// claurst-api: Anthropic API client with streaming SSE support for Claurst
// Rust port.
//
// Handles:
// - POST /v1/messages with streaming
// - SSE event parsing (message_start, content_block_start, content_block_delta,
//   content_block_stop, message_delta, message_stop, error)
// - Delta types: text_delta, input_json_delta, thinking_delta, signature_delta
// - Rate-limit (429) and overloaded (529) retry with exponential back-off
// - Authentication via API key from env or config

use claurst_core::constants::{ANTHROPIC_API_VERSION, ANTHROPIC_BETA_HEADER};
use claurst_core::error::ClaudeError;
use claurst_core::types::{ContentBlock, Message, MessageContent, Role, ToolDefinition, UsageInfo};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------
pub mod cch;
pub mod codex_adapter;

// Provider-agnostic unified types (Phase 1A).
pub mod provider_error;
pub mod provider_types;

// Provider abstraction traits (Phase 1B).
pub mod auth;
pub mod provider;
pub mod stream_parser;
pub mod transform;

// Provider registry (Phase 1C).
pub mod registry;

// Concrete provider adapters (Phase 1D).
pub mod providers;

// Model Registry (Phase 3).
pub mod model_registry;

// Provider-aware error handling (Phase 6).
pub mod error_handling;

// Message transform layer — concrete transformers (Phase 4).
pub mod transformers;

// ---------------------------------------------------------------------------
// Public re-exports
// ---------------------------------------------------------------------------
pub use client::AnthropicClient;
pub use streaming::{AnthropicStreamEvent, StreamHandler};
pub use types::*;

// Phase 1A re-exports — provider-agnostic layer.
pub use provider_error::ProviderError;
pub use provider_types::*;

// Phase 1B re-exports — provider abstraction traits.
pub use auth::{AuthProvider, LoginFlow};
pub use provider::{LlmProvider, ModelInfo};
pub use stream_parser::{JsonLinesStreamParser, SseStreamParser, StreamParser};
pub use transform::MessageTransformer;

// Phase 1C re-exports — provider registry.
pub use registry::ProviderRegistry;

// Phase 1D re-exports — concrete provider adapters.
pub use providers::AnthropicProvider;
pub use providers::GoogleProvider;
pub use providers::OpenAiProvider;

// Phase 3 re-exports — model registry.
pub use model_registry::{effective_model_for_config, ModelEntry, ModelRegistry};

// Phase 6 re-exports — provider-aware error handling.
pub use error_handling::{is_context_overflow, parse_error_response, RetryConfig};

// Phase 2E re-exports — Azure, Bedrock, and GitHub Copilot providers.
pub use providers::AzureProvider;
pub use providers::BedrockProvider;
pub use providers::CopilotProvider;

// Phase 2B re-exports — OpenAI-compatible generic adapter + common factories.
pub use providers::{
    deepseek, groq, lm_studio, mistral, ollama, openrouter, xai, OpenAiCompatProvider,
};

// Phase 2D re-exports — Cohere native provider.
pub use providers::CohereProvider;

// Phase 4 re-exports — concrete message transformers.
pub use transformers::{AnthropicTransformer, OpenAiChatTransformer};

// ---------------------------------------------------------------------------
// request / response types
// ---------------------------------------------------------------------------
pub mod types {
    use super::*;

    /// The request body sent to `POST /v1/messages`.
    #[derive(Debug, Clone, Serialize)]
    pub struct CreateMessageRequest {
        pub model: String,
        pub max_tokens: u32,
        pub messages: Vec<ApiMessage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub system: Option<SystemPrompt>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub tools: Option<Vec<ApiToolDefinition>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub temperature: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub top_p: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub top_k: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub stop_sequences: Option<Vec<String>>,
        pub stream: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub thinking: Option<ThinkingConfig>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ThinkingConfig {
        #[serde(rename = "type")]
        pub thinking_type: String,
        pub budget_tokens: u32,
    }

    impl ThinkingConfig {
        pub fn enabled(budget: u32) -> Self {
            Self {
                thinking_type: "enabled".to_string(),
                budget_tokens: budget,
            }
        }
    }

    /// System prompt - either a single string or structured blocks with cache.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(untagged)]
    pub enum SystemPrompt {
        Text(String),
        Blocks(Vec<SystemBlock>),
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SystemBlock {
        #[serde(rename = "type")]
        pub block_type: String,
        pub text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub cache_control: Option<CacheControl>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct CacheControl {
        #[serde(rename = "type")]
        pub control_type: String,
    }

    impl CacheControl {
        pub fn ephemeral() -> Self {
            Self {
                control_type: "ephemeral".to_string(),
            }
        }
    }

    /// Simplified message type for the API wire format.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ApiMessage {
        pub role: String,
        pub content: Value,
    }

    impl From<&Message> for ApiMessage {
        fn from(msg: &Message) -> Self {
            let role = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            let content = match &msg.content {
                MessageContent::Text(t) => Value::String(t.clone()),
                MessageContent::Blocks(blocks) => {
                    serde_json::to_value(blocks).unwrap_or(Value::Null)
                }
            };
            Self {
                role: role.to_string(),
                content,
            }
        }
    }

    /// Tool definition in the API wire format.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ApiToolDefinition {
        pub name: String,
        pub description: String,
        pub input_schema: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub cache_control: Option<CacheControl>,
    }

    impl From<&ToolDefinition> for ApiToolDefinition {
        fn from(td: &ToolDefinition) -> Self {
            Self {
                name: td.name.clone(),
                description: td.description.clone(),
                input_schema: td.input_schema.clone(),
                cache_control: None,
            }
        }
    }

    /// Non-streaming response from `POST /v1/messages`.
    #[derive(Debug, Clone, Deserialize)]
    pub struct CreateMessageResponse {
        pub id: String,
        #[serde(rename = "type")]
        pub response_type: String,
        pub role: String,
        pub content: Vec<Value>,
        pub model: String,
        pub stop_reason: Option<String>,
        pub stop_sequence: Option<String>,
        pub usage: UsageInfo,
    }

    /// Error body returned by the API.
    #[derive(Debug, Clone, Deserialize)]
    pub struct ApiErrorResponse {
        #[serde(rename = "type")]
        pub error_type: String,
        pub error: ApiErrorDetail,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct ApiErrorDetail {
        #[serde(rename = "type")]
        pub error_type: String,
        pub message: String,
    }
}

// ---------------------------------------------------------------------------
// SSE streaming types
// ---------------------------------------------------------------------------
pub mod streaming {
    use super::*;

    /// Events emitted by the Anthropic SSE streaming parser.
    #[derive(Debug, Clone)]
    pub enum AnthropicStreamEvent {
        /// The overall message has started; carries the message id and model.
        MessageStart {
            id: String,
            model: String,
            usage: UsageInfo,
        },
        /// A new content block has begun.
        ContentBlockStart {
            index: usize,
            content_block: ContentBlock,
        },
        /// Incremental delta for an existing content block.
        ContentBlockDelta { index: usize, delta: ContentDelta },
        /// A content block is finished.
        ContentBlockStop { index: usize },
        /// Final message-level delta (stop_reason, usage).
        MessageDelta {
            stop_reason: Option<String>,
            usage: Option<UsageInfo>,
        },
        /// The message is complete.
        MessageStop,
        /// An error occurred during streaming.
        Error { error_type: String, message: String },
        /// A ping/keep-alive event.
        Ping,
    }

    /// The delta payload inside a `content_block_delta` event.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum ContentDelta {
        TextDelta { text: String },
        InputJsonDelta { partial_json: String },
        ThinkingDelta { thinking: String },
        SignatureDelta { signature: String },
    }

    /// Trait for anything that wants to consume streaming events in real time.
    pub trait StreamHandler: Send + Sync {
        fn on_event(&self, event: &AnthropicStreamEvent);
    }

    /// A no-op handler useful for non-interactive / batch mode.
    pub struct NullStreamHandler;
    impl StreamHandler for NullStreamHandler {
        fn on_event(&self, _event: &AnthropicStreamEvent) {}
    }
}

// ---------------------------------------------------------------------------
// SSE line parser
// ---------------------------------------------------------------------------
mod sse_parser {
    /// Parsed SSE frame.
    #[derive(Debug)]
    pub struct SseFrame {
        pub event: Option<String>,
        pub data: String,
    }

    /// Incrementally accumulates raw bytes/lines and yields complete frames.
    pub struct SseLineParser {
        event_type: Option<String>,
        data_buf: String,
    }

    impl SseLineParser {
        pub fn new() -> Self {
            Self {
                event_type: None,
                data_buf: String::new(),
            }
        }

        /// Feed one line (without the trailing newline).  Returns `Some(frame)`
        /// when a blank line signals the end of an event.
        pub fn feed_line(&mut self, line: &str) -> Option<SseFrame> {
            if line.is_empty() {
                // Blank line = end of event
                if self.data_buf.is_empty() && self.event_type.is_none() {
                    return None; // spurious blank line
                }
                let frame = SseFrame {
                    event: self.event_type.take(),
                    data: std::mem::take(&mut self.data_buf),
                };
                return Some(frame);
            }

            if let Some(rest) = line.strip_prefix("event:") {
                self.event_type = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                if !self.data_buf.is_empty() {
                    self.data_buf.push('\n');
                }
                self.data_buf.push_str(rest.trim());
            } else if line.starts_with(':') {
                // SSE comment / keep-alive – ignore
            }

            None
        }
    }
}

// ---------------------------------------------------------------------------
// Models endpoint types (public)
// ---------------------------------------------------------------------------

/// A model entry returned by `GET /v1/models`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AvailableModel {
    pub id: String,
    pub display_name: Option<String>,
    /// Unix timestamp of when the model was created (seconds).
    pub created_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// Anthropic client
// ---------------------------------------------------------------------------
pub mod client {
    use super::*;

    /// Provider selection for API calls.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Provider {
        /// Use Anthropic's API
        Anthropic,
        /// Use OpenAI Codex via OAuth
        Codex,
    }

    impl Default for Provider {
        fn default() -> Self {
            Provider::Anthropic
        }
    }

    /// Configuration for the HTTP client.
    #[derive(Debug, Clone)]
    pub struct ClientConfig {
        pub api_key: String,
        pub api_base: String,
        pub api_version: String,
        pub beta_features: String,
        pub max_retries: u32,
        pub initial_retry_delay: Duration,
        pub max_retry_delay: Duration,
        pub request_timeout: Duration,
        /// When true, send `Authorization: Bearer <api_key>` instead of `x-api-key`.
        /// Used for Claude.ai subscription (OAuth user:inference scope) tokens.
        pub use_bearer_auth: bool,
        /// Which provider to use for API calls.
        pub provider: Provider,
    }

    impl Default for ClientConfig {
        fn default() -> Self {
            Self {
                api_key: String::new(),
                api_base: claurst_core::constants::ANTHROPIC_API_BASE.to_string(),
                api_version: ANTHROPIC_API_VERSION.to_string(),
                beta_features: ANTHROPIC_BETA_HEADER.to_string(),
                max_retries: 5,
                initial_retry_delay: Duration::from_secs(1),
                max_retry_delay: Duration::from_secs(60),
                request_timeout: Duration::from_secs(600),
                use_bearer_auth: false,
                provider: Provider::Anthropic,
            }
        }
    }

    /// The main Anthropic API client.
    pub struct AnthropicClient {
        http: reqwest::Client,
        config: ClientConfig,
    }

    impl AnthropicClient {
        pub fn with_provider(&self, provider: Provider) -> anyhow::Result<Self> {
            let mut config = self.config.clone();
            config.provider = provider;
            Self::new(config)
        }

        async fn resolve_codex_auth(&self) -> Result<(String, Option<String>), ClaudeError> {
            let mut tokens = claurst_core::oauth_config::get_codex_tokens();

            if let Some(current) = tokens.as_mut() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let expired = current
                    .expires_at
                    .map(|expires_at| now >= expires_at.saturating_sub(60))
                    .unwrap_or(false);

                if expired {
                    let refresh = current.refresh_token.clone().ok_or_else(|| {
                        ClaudeError::Auth(
                            "OpenAI Codex login expired. Run /connect and choose OpenAI Codex again."
                                .to_string(),
                        )
                    })?;

                    let body: Value = reqwest::Client::new()
                        .post(claurst_core::codex_oauth::CODEX_TOKEN_URL)
                        .form(&[
                            ("grant_type", "refresh_token"),
                            ("refresh_token", refresh.as_str()),
                            ("client_id", claurst_core::codex_oauth::CODEX_CLIENT_ID),
                        ])
                        .send()
                        .await
                        .map_err(|e| {
                            ClaudeError::Other(format!("Codex token refresh failed: {}", e))
                        })?
                        .error_for_status()
                        .map_err(|e| {
                            ClaudeError::Auth(format!("Codex token refresh failed: {}", e))
                        })?
                        .json()
                        .await
                        .map_err(|e| {
                            ClaudeError::Other(format!(
                                "Failed to parse Codex token refresh response: {}",
                                e
                            ))
                        })?;

                    current.access_token = body["access_token"].as_str().unwrap_or("").to_string();
                    if current.access_token.is_empty() {
                        return Err(ClaudeError::Auth(
                            "Codex token refresh returned no access token.".to_string(),
                        ));
                    }
                    if let Some(refresh_token) = body["refresh_token"].as_str() {
                        current.refresh_token = Some(refresh_token.to_string());
                    }
                    current.expires_at =
                        claurst_core::codex_oauth::expires_at_from_now(body["expires_in"].as_u64());
                    current.account_id = body["id_token"]
                        .as_str()
                        .and_then(claurst_core::codex_oauth::extract_account_id_from_jwt)
                        .or_else(|| {
                            claurst_core::codex_oauth::extract_account_id_from_jwt(
                                &current.access_token,
                            )
                        })
                        .or_else(|| current.account_id.clone());
                    claurst_core::oauth_config::save_codex_tokens(current).map_err(|e| {
                        ClaudeError::Other(format!("Failed to save refreshed Codex token: {}", e))
                    })?;
                }
            }

            let access_token = tokens
                .as_ref()
                .map(|tokens| tokens.access_token.as_str())
                .filter(|token| !token.is_empty())
                .or_else(|| {
                    (!self.config.api_key.is_empty()).then_some(self.config.api_key.as_str())
                })
                .ok_or_else(|| {
                    ClaudeError::Auth(
                        "No OpenAI Codex login found. Run /connect and choose OpenAI Codex."
                            .to_string(),
                    )
                })?
                .to_string();

            Ok((access_token, tokens.and_then(|tokens| tokens.account_id)))
        }

        /// Returns `true` when the client was constructed without an API key.
        ///
        /// The query loop checks this to know whether it should fall back to
        /// a runtime-built provider (e.g. from keys stored via `/connect`).
        pub fn api_key_is_empty(&self) -> bool {
            self.config.api_key.is_empty()
        }

        /// Build a new client.  Panics if `config.api_key` is empty.
        pub fn new(config: ClientConfig) -> anyhow::Result<Self> {
            // Allow empty key at construction — validation is deferred to
            // the first API call, so non-Anthropic provider setups can still
            // create this client without an Anthropic key configured.
            let http = reqwest::Client::builder()
                .timeout(config.request_timeout)
                .build()?;

            Ok(Self { http, config })
        }

        /// Convenience constructor that resolves the key from config/env.
        pub fn from_config(cfg: &claurst_core::config::Config) -> anyhow::Result<Self> {
            let api_key = cfg
                .resolve_api_key()
                .ok_or_else(|| anyhow::anyhow!("No API key found"))?;
            let api_base = cfg.resolve_api_base();

            Self::new(ClientConfig {
                api_key,
                api_base,
                ..Default::default()
            })
        }

        // ---- Non-streaming create message --------------------------------

        /// Send a non-streaming `POST /v1/messages` and return the full response.
        pub async fn create_message(
            &self,
            mut request: CreateMessageRequest,
        ) -> Result<CreateMessageResponse, ClaudeError> {
            // Deferred key validation — fail here rather than at construction
            // so that non-Anthropic provider setups don't crash on startup.
            if self.config.api_key.is_empty() && self.config.provider != Provider::Codex {
                // Check if this model might belong to another provider, giving
                // the user a more actionable error message.
                let model = &request.model;
                let hint = if model.starts_with("gemini") || model.starts_with("gemma") {
                    format!(
                        "Model '{}' is a Google model. Use `--provider google` or set GOOGLE_API_KEY.",
                        model
                    )
                } else if model.starts_with("gpt-")
                    || model.starts_with("o1")
                    || model.starts_with("o3")
                    || model.starts_with("o4")
                {
                    format!(
                        "Model '{}' is an OpenAI model. Use `--provider openai` or set OPENAI_API_KEY.",
                        model
                    )
                } else if model.starts_with("deepseek") {
                    format!(
                        "Model '{}' is a DeepSeek model. Use `--provider deepseek` or set DEEPSEEK_API_KEY.",
                        model
                    )
                } else if model.starts_with("grok") {
                    format!(
                        "Model '{}' is an xAI model. Use `--provider xai` or set XAI_API_KEY.",
                        model
                    )
                } else if model.starts_with("mistral") || model.starts_with("codestral") {
                    format!(
                        "Model '{}' is a Mistral model. Use `--provider mistral` or set MISTRAL_API_KEY.",
                        model
                    )
                } else if model.starts_with("command-") {
                    format!(
                        "Model '{}' is a Cohere model. Use `--provider cohere` or set COHERE_API_KEY.",
                        model
                    )
                } else if model.starts_with("llama") {
                    format!(
                        "Model '{}' looks like a Llama model. Use `--provider groq` (set GROQ_API_KEY) or `--provider ollama` for local.",
                        model
                    )
                } else {
                    "Set ANTHROPIC_API_KEY, run `claurst auth login`, \
                     or use --provider to select a different provider (e.g. --provider openai)."
                        .to_string()
                };
                return Err(ClaudeError::Auth(format!(
                    "No API key for the selected model. {}",
                    hint
                )));
            }
            // Route to Codex if configured
            if self.config.provider == Provider::Codex {
                return self.create_message_codex(&request).await;
            }

            request.stream = false;
            let body = serde_json::to_value(&request).map_err(ClaudeError::Json)?;

            let resp = self.send_with_retry(&body).await?;
            let status = resp.status();
            let text = resp.text().await.map_err(ClaudeError::Http)?;

            if !status.is_success() {
                return Err(self.parse_api_error(status.as_u16(), &text));
            }

            serde_json::from_str(&text).map_err(ClaudeError::Json)
        }

        /// Send a request to OpenAI Codex API instead of Anthropic.
        async fn create_message_codex(
            &self,
            request: &CreateMessageRequest,
        ) -> Result<CreateMessageResponse, ClaudeError> {
            let (access_token, account_id) = self.resolve_codex_auth().await?;
            let model = request
                .model
                .strip_prefix("openai-codex/")
                .unwrap_or(&request.model)
                .to_string();

            // Convert Anthropic format to OpenAI format
            let mut openai_req = codex_adapter::anthropic_to_openai_request(request);
            openai_req["model"] = Value::String(model.clone());

            // Send to Codex endpoint
            let client = reqwest::Client::new();
            let mut req = client
                .post(codex_adapter::CODEX_RESPONSES_ENDPOINT)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("Accept", "text/event-stream")
                .header("Content-Type", "application/json")
                .json(&openai_req);
            if let Some(account_id) = account_id
                .as_deref()
                .filter(|account_id| !account_id.is_empty())
            {
                req = req.header("ChatGPT-Account-Id", account_id);
            }
            let resp = req
                .timeout(self.config.request_timeout)
                .send()
                .await
                .map_err(|e| ClaudeError::Other(format!("Codex request failed: {}", e)))?;

            let status = resp.status();
            let text = resp.text().await.map_err(ClaudeError::Http)?;

            if !status.is_success() {
                return Err(self.parse_api_error(status.as_u16(), &text));
            }

            if text.contains("response.output_text.delta") || text.contains("response.completed") {
                return Self::parse_codex_stream_text(&text, &model);
            }

            // Parse OpenAI response and convert to Anthropic format
            let openai_resp: Value = serde_json::from_str(&text).map_err(ClaudeError::Json)?;
            let (content, stop_reason, input_tokens, output_tokens) =
                codex_adapter::parse_openai_response(&openai_resp);

            let response = codex_adapter::build_anthropic_response(
                &content,
                &stop_reason,
                input_tokens,
                output_tokens,
                &model,
            );

            Ok(response)
        }

        fn parse_codex_stream_text(
            text: &str,
            model: &str,
        ) -> Result<CreateMessageResponse, ClaudeError> {
            use sse_parser::SseLineParser;

            let mut parser = SseLineParser::new();
            let mut content = String::new();
            let mut stop_reason = "end_turn".to_string();
            let mut usage = UsageInfo::default();
            let mut response_id: Option<String> = None;

            for raw_line in text.lines() {
                let line = raw_line.trim_end_matches('\r');
                if let Some(frame) = parser.feed_line(line) {
                    if frame.data == "[DONE]" {
                        continue;
                    }

                    let value: Value = match serde_json::from_str(&frame.data) {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    let chunk_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

                    match chunk_type {
                        "response.created" => {
                            response_id = value
                                .get("response")
                                .and_then(|response| response.get("id"))
                                .and_then(|id| id.as_str())
                                .map(|id| id.to_string());
                        }
                        "response.output_text.delta" => {
                            if let Some(delta) = value.get("delta").and_then(|delta| delta.as_str())
                            {
                                content.push_str(delta);
                            }
                        }
                        "response.completed" | "response.incomplete" => {
                            stop_reason = codex_adapter::parse_openai_response(&value).1;
                            usage.input_tokens = value
                                .get("response")
                                .and_then(|response| response.get("usage"))
                                .and_then(|usage| usage.get("input_tokens"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            usage.output_tokens = value
                                .get("response")
                                .and_then(|response| response.get("usage"))
                                .and_then(|usage| usage.get("output_tokens"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            usage.cache_read_input_tokens = value
                                .get("response")
                                .and_then(|response| response.get("usage"))
                                .and_then(|usage| usage.get("input_tokens_details"))
                                .and_then(|details| details.get("cached_tokens"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                        }
                        "error" => {
                            let message = value
                                .get("message")
                                .and_then(|message| message.as_str())
                                .unwrap_or("Unknown Codex streaming error")
                                .to_string();
                            return Err(ClaudeError::Api(message));
                        }
                        _ => {}
                    }
                }
            }

            let mut response = codex_adapter::build_anthropic_response(
                &content,
                &stop_reason,
                usage.input_tokens,
                usage.output_tokens,
                model,
            );
            response.usage.cache_read_input_tokens = usage.cache_read_input_tokens;
            if let Some(id) = response_id {
                response.id = id;
            }
            Ok(response)
        }

        // ---- Streaming create message ------------------------------------

        /// Send a streaming `POST /v1/messages`.  Events are dispatched to the
        /// provided `handler` in real time, and also forwarded into the returned
        /// channel so the caller can drive a select loop.
        pub async fn create_message_stream(
            &self,
            mut request: CreateMessageRequest,
            handler: Arc<dyn StreamHandler>,
        ) -> Result<mpsc::Receiver<streaming::AnthropicStreamEvent>, ClaudeError> {
            // Deferred key validation
            if self.config.api_key.is_empty() && self.config.provider != Provider::Codex {
                let model = &request.model;
                let hint = if model.starts_with("gemini") || model.starts_with("gemma") {
                    format!(
                        "Model '{}' is a Google model. Use `--provider google` or set GOOGLE_API_KEY.",
                        model
                    )
                } else if model.starts_with("gpt-")
                    || model.starts_with("o1")
                    || model.starts_with("o3")
                    || model.starts_with("o4")
                {
                    format!(
                        "Model '{}' is an OpenAI model. Use `--provider openai` or set OPENAI_API_KEY.",
                        model
                    )
                } else if model.starts_with("deepseek") {
                    format!("Model '{}' is a DeepSeek model. Use `--provider deepseek` or set DEEPSEEK_API_KEY.", model)
                } else if model.starts_with("grok") {
                    format!(
                        "Model '{}' is an xAI model. Use `--provider xai` or set XAI_API_KEY.",
                        model
                    )
                } else if model.starts_with("mistral") || model.starts_with("codestral") {
                    format!("Model '{}' is a Mistral model. Use `--provider mistral` or set MISTRAL_API_KEY.", model)
                } else if model.starts_with("command-") {
                    format!("Model '{}' is a Cohere model. Use `--provider cohere` or set COHERE_API_KEY.", model)
                } else if model.starts_with("llama") {
                    format!("Model '{}' looks like a Llama model. Use `--provider groq` or `--provider ollama` for local.", model)
                } else {
                    "Set ANTHROPIC_API_KEY, run `claurst auth login`, \
                     or use --provider to select a different provider (e.g. --provider openai)."
                        .to_string()
                };
                return Err(ClaudeError::Auth(format!(
                    "No API key for the selected model. {}",
                    hint
                )));
            }
            // Codex uses a non-streaming endpoint. Synthesize a streaming event
            // sequence so the rest of the app can keep using the streaming path.
            if self.config.provider == Provider::Codex {
                request.stream = false;
                let response = self.create_message_codex(&request).await?;
                let stop_reason = response.stop_reason.clone();
                let usage = response.usage.clone();
                let text = response
                    .content
                    .iter()
                    .filter_map(|block| block.get("text").and_then(|text| text.as_str()))
                    .collect::<Vec<_>>()
                    .join("");
                let events = vec![
                    streaming::AnthropicStreamEvent::MessageStart {
                        id: response.id.clone(),
                        model: response.model.clone(),
                        usage: usage.clone(),
                    },
                    streaming::AnthropicStreamEvent::ContentBlockStart {
                        index: 0,
                        content_block: ContentBlock::Text {
                            text: String::new(),
                        },
                    },
                    streaming::AnthropicStreamEvent::ContentBlockDelta {
                        index: 0,
                        delta: streaming::ContentDelta::TextDelta { text },
                    },
                    streaming::AnthropicStreamEvent::ContentBlockStop { index: 0 },
                    streaming::AnthropicStreamEvent::MessageDelta {
                        stop_reason,
                        usage: Some(usage),
                    },
                    streaming::AnthropicStreamEvent::MessageStop,
                ];
                let (tx, rx) = mpsc::channel(16);

                tokio::spawn(async move {
                    for event in events {
                        handler.on_event(&event);
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                });

                return Ok(rx);
            }

            request.stream = true;
            let body = serde_json::to_value(&request).map_err(ClaudeError::Json)?;

            let resp = self.send_with_retry(&body).await?;
            let status = resp.status();

            if !status.is_success() {
                let text = resp.text().await.map_err(ClaudeError::Http)?;
                return Err(self.parse_api_error(status.as_u16(), &text));
            }

            let (tx, rx) = mpsc::channel(256);

            // Spawn a task that reads the SSE byte stream and emits events.
            tokio::spawn(async move {
                if let Err(e) = Self::process_sse_stream(resp, handler, tx.clone()).await {
                    let _ = tx
                        .send(streaming::AnthropicStreamEvent::Error {
                            error_type: "stream_error".into(),
                            message: e.to_string(),
                        })
                        .await;
                }
            });

            Ok(rx)
        }

        // ---- Models list ------------------------------------------------

        /// Fetch available models from `GET /v1/models`.
        ///
        /// Returns a list of models the current API key has access to.
        /// Falls back gracefully: returns an empty `Vec` on any error so
        /// callers can fall back to the hardcoded default list instead of
        /// surfacing an error.
        pub async fn fetch_available_models(&self) -> anyhow::Result<Vec<crate::AvailableModel>> {
            let url = format!("{}/v1/models", self.config.api_base);

            let mut req = self
                .http
                .get(&url)
                .header("anthropic-version", &self.config.api_version)
                .header("content-type", "application/json");
            req = if self.config.use_bearer_auth {
                req.header("Authorization", format!("Bearer {}", &self.config.api_key))
            } else {
                req.header("x-api-key", &self.config.api_key)
            };

            let resp = req.send().await?;

            if !resp.status().is_success() {
                anyhow::bail!("models endpoint returned {}", resp.status());
            }

            #[derive(serde::Deserialize)]
            struct ModelsResponse {
                data: Vec<crate::AvailableModel>,
            }

            let body: ModelsResponse = resp.json().await?;
            Ok(body.data)
        }

        // ---- Internal helpers --------------------------------------------

        /// Build the common request and execute with retry logic.
        async fn send_with_retry(&self, body: &Value) -> Result<reqwest::Response, ClaudeError> {
            let url = format!("{}/v1/messages", self.config.api_base);
            let mut attempts = 0u32;
            let mut delay = self.config.initial_retry_delay;

            // Serialize body to JSON string for CCH signing
            let body_str = serde_json::to_string(body)
                .map_err(|e| ClaudeError::Api(format!("Failed to serialize request: {}", e)))?;
            let body_bytes = body_str.as_bytes();

            loop {
                attempts += 1;

                // Compute CCH hash and build billing header
                let cch_hash = cch::compute_cch(body_bytes);
                let billing_header = format!(
                    "cc_version=0.1; cc_entrypoint=claude_code; {}; cc_workload=claude_code;",
                    cch_hash
                );

                // Use Bearer auth for Claude.ai OAuth tokens; x-api-key for regular keys.
                let mut req = self
                    .http
                    .post(&url)
                    .header("anthropic-version", &self.config.api_version)
                    .header("anthropic-beta", &self.config.beta_features)
                    .header("content-type", "application/json")
                    .header("accept", "text/event-stream")
                    .header("x-anthropic-billing-header", billing_header);
                req = if self.config.use_bearer_auth {
                    req.header("Authorization", format!("Bearer {}", &self.config.api_key))
                } else {
                    req.header("x-api-key", &self.config.api_key)
                };
                let req = req.body(body_str.clone());

                let resp = req.send().await.map_err(ClaudeError::Http)?;
                let status = resp.status().as_u16();

                // 200-299: success
                if resp.status().is_success() {
                    return Ok(resp);
                }

                // 429 (rate limit) or 529 (overloaded): retry
                if (status == 429 || status == 529) && attempts <= self.config.max_retries {
                    // Honour Retry-After header if present
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(Duration::from_secs);

                    let wait = retry_after.unwrap_or(delay);
                    warn!(
                        status,
                        attempt = attempts,
                        wait_secs = wait.as_secs(),
                        "Retryable API error, backing off"
                    );
                    tokio::time::sleep(wait).await;
                    delay = (delay * 2).min(self.config.max_retry_delay);
                    continue;
                }

                // Non-retryable error – return immediately
                let text = resp.text().await.unwrap_or_default();
                return Err(self.parse_api_error(status, &text));
            }
        }

        /// Parse an API error body into a typed `ClaudeError`.
        fn parse_api_error(&self, status: u16, body: &str) -> ClaudeError {
            if let Ok(err) = serde_json::from_str::<ApiErrorResponse>(body) {
                match status {
                    401 => ClaudeError::Auth(err.error.message),
                    429 => ClaudeError::RateLimit,
                    529 => ClaudeError::ApiStatus {
                        status,
                        message: format!("Overloaded: {}", err.error.message),
                    },
                    _ => ClaudeError::ApiStatus {
                        status,
                        message: err.error.message,
                    },
                }
            } else {
                ClaudeError::ApiStatus {
                    status,
                    message: body.to_string(),
                }
            }
        }

        /// Read an SSE byte stream, parse frames, and emit `AnthropicStreamEvent`s.
        async fn process_sse_stream(
            resp: reqwest::Response,
            handler: Arc<dyn StreamHandler>,
            tx: mpsc::Sender<streaming::AnthropicStreamEvent>,
        ) -> Result<(), ClaudeError> {
            use sse_parser::SseLineParser;

            let mut parser = SseLineParser::new();
            let mut byte_stream = resp.bytes_stream();
            let mut leftover = String::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = chunk_result.map_err(ClaudeError::Http)?;
                let text = String::from_utf8_lossy(&chunk);

                // Prepend any leftover from the previous chunk
                let combined = if leftover.is_empty() {
                    text.to_string()
                } else {
                    let mut s = std::mem::take(&mut leftover);
                    s.push_str(&text);
                    s
                };

                // Split into lines.  If the chunk doesn't end with a newline
                // the last piece is an incomplete line – stash it.
                let mut lines: Vec<&str> = combined.split('\n').collect();
                if !combined.ends_with('\n') {
                    leftover = lines.pop().unwrap_or("").to_string();
                }

                for line in lines {
                    let line = line.trim_end_matches('\r');
                    if let Some(frame) = parser.feed_line(line) {
                        if let Some(event) = Self::frame_to_event(&frame.event, &frame.data) {
                            handler.on_event(&event);
                            if tx.send(event).await.is_err() {
                                // Receiver dropped – stop reading.
                                return Ok(());
                            }
                        }
                    }
                }
            }

            Ok(())
        }

        /// Convert a parsed SSE frame into a typed `AnthropicStreamEvent`.
        fn frame_to_event(
            event_type: &Option<String>,
            data: &str,
        ) -> Option<streaming::AnthropicStreamEvent> {
            let event_name = event_type.as_deref().unwrap_or("");

            match event_name {
                "ping" => Some(streaming::AnthropicStreamEvent::Ping),

                "message_start" => {
                    let v: Value = serde_json::from_str(data).ok()?;
                    let msg = v.get("message")?;
                    let id = msg.get("id")?.as_str()?.to_string();
                    let model = msg.get("model")?.as_str()?.to_string();
                    let usage = msg
                        .get("usage")
                        .and_then(|u| serde_json::from_value::<UsageInfo>(u.clone()).ok())
                        .unwrap_or_default();

                    Some(streaming::AnthropicStreamEvent::MessageStart { id, model, usage })
                }

                "content_block_start" => {
                    let v: Value = serde_json::from_str(data).ok()?;
                    let index = v.get("index")?.as_u64()? as usize;
                    let block_value = v.get("content_block")?;
                    let content_block: ContentBlock =
                        serde_json::from_value(block_value.clone()).ok()?;
                    Some(streaming::AnthropicStreamEvent::ContentBlockStart {
                        index,
                        content_block,
                    })
                }

                "content_block_delta" => {
                    let v: Value = serde_json::from_str(data).ok()?;
                    let index = v.get("index")?.as_u64()? as usize;
                    let delta_value = v.get("delta")?;
                    let delta: streaming::ContentDelta =
                        serde_json::from_value(delta_value.clone()).ok()?;
                    Some(streaming::AnthropicStreamEvent::ContentBlockDelta { index, delta })
                }

                "content_block_stop" => {
                    let v: Value = serde_json::from_str(data).ok()?;
                    let index = v.get("index")?.as_u64()? as usize;
                    Some(streaming::AnthropicStreamEvent::ContentBlockStop { index })
                }

                "message_delta" => {
                    let v: Value = serde_json::from_str(data).ok()?;
                    let delta = v.get("delta")?;
                    let stop_reason = delta
                        .get("stop_reason")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string());
                    let usage = v
                        .get("usage")
                        .and_then(|u| serde_json::from_value::<UsageInfo>(u.clone()).ok());
                    Some(streaming::AnthropicStreamEvent::MessageDelta { stop_reason, usage })
                }

                "message_stop" => Some(streaming::AnthropicStreamEvent::MessageStop),

                "error" => {
                    let v: Value = serde_json::from_str(data).ok()?;
                    let error = v.get("error")?;
                    let error_type = error
                        .get("type")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let message = error
                        .get("message")
                        .and_then(|s| s.as_str())
                        .unwrap_or("Unknown error")
                        .to_string();
                    Some(streaming::AnthropicStreamEvent::Error {
                        error_type,
                        message,
                    })
                }

                _ => {
                    debug!(event = event_name, "Unhandled SSE event type");
                    None
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience builder for CreateMessageRequest
// ---------------------------------------------------------------------------

impl CreateMessageRequest {
    /// Create a minimal request builder.
    pub fn builder(model: impl Into<String>, max_tokens: u32) -> CreateMessageRequestBuilder {
        CreateMessageRequestBuilder {
            model: model.into(),
            max_tokens,
            messages: vec![],
            system: None,
            tools: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
        }
    }
}

pub struct CreateMessageRequestBuilder {
    model: String,
    max_tokens: u32,
    messages: Vec<ApiMessage>,
    system: Option<SystemPrompt>,
    tools: Option<Vec<ApiToolDefinition>>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    top_k: Option<u32>,
    stop_sequences: Option<Vec<String>>,
    thinking: Option<ThinkingConfig>,
}

impl CreateMessageRequestBuilder {
    pub fn messages(mut self, msgs: Vec<ApiMessage>) -> Self {
        self.messages = msgs;
        self
    }

    pub fn add_message(mut self, msg: ApiMessage) -> Self {
        self.messages.push(msg);
        self
    }

    pub fn system(mut self, s: SystemPrompt) -> Self {
        self.system = Some(s);
        self
    }

    pub fn system_text(mut self, text: impl Into<String>) -> Self {
        self.system = Some(SystemPrompt::Text(text.into()));
        self
    }

    pub fn tools(mut self, tools: Vec<ApiToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    pub fn top_p(mut self, p: f32) -> Self {
        self.top_p = Some(p);
        self
    }

    pub fn top_k(mut self, k: u32) -> Self {
        self.top_k = Some(k);
        self
    }

    pub fn stop_sequences(mut self, seqs: Vec<String>) -> Self {
        self.stop_sequences = Some(seqs);
        self
    }

    pub fn thinking(mut self, config: ThinkingConfig) -> Self {
        self.thinking = Some(config);
        self
    }

    pub fn build(self) -> CreateMessageRequest {
        CreateMessageRequest {
            model: self.model,
            max_tokens: self.max_tokens,
            messages: self.messages,
            system: self.system,
            tools: self.tools,
            temperature: self.temperature,
            top_p: self.top_p,
            top_k: self.top_k,
            stop_sequences: self.stop_sequences,
            stream: true,
            thinking: self.thinking,
        }
    }
}

// ---------------------------------------------------------------------------
// Accumulated message builder – reconstructs a full Message from stream events
// ---------------------------------------------------------------------------

/// Collects streaming events and produces a finished `Message` plus usage info.
pub struct StreamAccumulator {
    id: Option<String>,
    model: Option<String>,
    content_blocks: Vec<ContentBlock>,
    /// Partial accumulators keyed by block index.
    partials: std::collections::HashMap<usize, PartialBlock>,
    stop_reason: Option<String>,
    usage: UsageInfo,
}

#[derive(Debug)]
enum PartialBlock {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        json_buf: String,
    },
    Thinking {
        thinking_buf: String,
        signature_buf: String,
    },
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self {
            id: None,
            model: None,
            content_blocks: vec![],
            partials: Default::default(),
            stop_reason: None,
            usage: UsageInfo::default(),
        }
    }

    /// Feed a stream event. Call this for every event received from the stream.
    pub fn on_event(&mut self, event: &streaming::AnthropicStreamEvent) {
        use streaming::AnthropicStreamEvent;
        match event {
            AnthropicStreamEvent::MessageStart { id, model, usage } => {
                self.id = Some(id.clone());
                self.model = Some(model.clone());
                self.usage = usage.clone();
            }

            AnthropicStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                let partial = match content_block {
                    ContentBlock::Text { text } => PartialBlock::Text(text.clone()),
                    ContentBlock::ToolUse { id, name, .. } => PartialBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        json_buf: String::new(),
                    },
                    ContentBlock::Thinking {
                        thinking,
                        signature,
                    } => PartialBlock::Thinking {
                        thinking_buf: thinking.clone(),
                        signature_buf: signature.clone(),
                    },
                    _ => return,
                };
                self.partials.insert(*index, partial);
            }

            AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                if let Some(partial) = self.partials.get_mut(index) {
                    match (partial, delta) {
                        (PartialBlock::Text(buf), streaming::ContentDelta::TextDelta { text }) => {
                            buf.push_str(text);
                        }
                        (
                            PartialBlock::ToolUse { json_buf, .. },
                            streaming::ContentDelta::InputJsonDelta { partial_json },
                        ) => {
                            json_buf.push_str(partial_json);
                        }
                        (
                            PartialBlock::Thinking { thinking_buf, .. },
                            streaming::ContentDelta::ThinkingDelta { thinking },
                        ) => {
                            thinking_buf.push_str(thinking);
                        }
                        (
                            PartialBlock::Thinking { signature_buf, .. },
                            streaming::ContentDelta::SignatureDelta { signature },
                        ) => {
                            signature_buf.push_str(signature);
                        }
                        _ => {}
                    }
                }
            }

            AnthropicStreamEvent::ContentBlockStop { index } => {
                if let Some(partial) = self.partials.remove(index) {
                    let block = match partial {
                        PartialBlock::Text(text) => ContentBlock::Text { text },
                        PartialBlock::ToolUse { id, name, json_buf } => {
                            let input = serde_json::from_str(&json_buf)
                                .unwrap_or(Value::Object(Default::default()));
                            ContentBlock::ToolUse { id, name, input }
                        }
                        PartialBlock::Thinking {
                            thinking_buf,
                            signature_buf,
                        } => ContentBlock::Thinking {
                            thinking: thinking_buf,
                            signature: signature_buf,
                        },
                    };
                    self.content_blocks.push(block);
                }
            }

            AnthropicStreamEvent::MessageDelta { stop_reason, usage } => {
                if let Some(sr) = stop_reason {
                    self.stop_reason = Some(sr.clone());
                }
                if let Some(u) = usage {
                    // The delta usage usually only has output_tokens;
                    // add them to the running total.
                    self.usage.output_tokens += u.output_tokens;
                }
            }

            AnthropicStreamEvent::MessageStop => {}
            AnthropicStreamEvent::Ping => {}
            AnthropicStreamEvent::Error { .. } => {}
        }
    }

    /// Finalize and produce the accumulated `Message`.
    pub fn finish(self) -> (Message, UsageInfo, Option<String>) {
        let msg = Message::assistant_blocks(self.content_blocks);
        (msg, self.usage, self.stop_reason)
    }

    pub fn stop_reason(&self) -> Option<&str> {
        self.stop_reason.as_deref()
    }

    pub fn usage(&self) -> &UsageInfo {
        &self.usage
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_parser_basic() {
        let mut parser = sse_parser::SseLineParser::new();
        assert!(parser.feed_line("event: message_start").is_none());
        assert!(parser
            .feed_line(r#"data: {"message":{"id":"m1","model":"claude","usage":{"input_tokens":0,"output_tokens":0}}}"#)
            .is_none());
        let frame = parser.feed_line("").expect("should produce frame");
        assert_eq!(frame.event.as_deref(), Some("message_start"));
        assert!(frame.data.contains("m1"));
    }

    #[test]
    fn test_create_message_request_builder() {
        let req = CreateMessageRequest::builder("claude-opus-4-6", 4096)
            .system_text("You are helpful.")
            .temperature(0.7)
            .build();
        assert_eq!(req.model, "claude-opus-4-6");
        assert_eq!(req.max_tokens, 4096);
        assert!(req.stream);
    }

    #[test]
    fn test_stream_accumulator_text() {
        let mut acc = StreamAccumulator::new();
        acc.on_event(&streaming::AnthropicStreamEvent::MessageStart {
            id: "m1".into(),
            model: "claude".into(),
            usage: UsageInfo::default(),
        });
        acc.on_event(&streaming::AnthropicStreamEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::Text {
                text: String::new(),
            },
        });
        acc.on_event(&streaming::AnthropicStreamEvent::ContentBlockDelta {
            index: 0,
            delta: streaming::ContentDelta::TextDelta {
                text: "Hello ".into(),
            },
        });
        acc.on_event(&streaming::AnthropicStreamEvent::ContentBlockDelta {
            index: 0,
            delta: streaming::ContentDelta::TextDelta {
                text: "world!".into(),
            },
        });
        acc.on_event(&streaming::AnthropicStreamEvent::ContentBlockStop { index: 0 });
        acc.on_event(&streaming::AnthropicStreamEvent::MessageDelta {
            stop_reason: Some("end_turn".into()),
            usage: None,
        });
        acc.on_event(&streaming::AnthropicStreamEvent::MessageStop);

        let (msg, _usage, stop) = acc.finish();
        assert_eq!(msg.get_text(), Some("Hello world!"));
        assert_eq!(stop.as_deref(), Some("end_turn"));
    }
}
