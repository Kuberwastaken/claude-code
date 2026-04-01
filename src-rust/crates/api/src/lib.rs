// cc-api: Anthropic API client with streaming SSE support for the Claude Code
// Rust port.
//
// Handles:
// - POST /v1/messages with streaming
// - SSE event parsing (message_start, content_block_start, content_block_delta,
//   content_block_stop, message_delta, message_stop, error)
// - Delta types: text_delta, input_json_delta, thinking_delta, signature_delta
// - Rate-limit (429) and overloaded (529) retry with exponential back-off
// - Authentication via API key from env or config

use cc_core::constants::{ANTHROPIC_API_VERSION, ANTHROPIC_BETA_HEADER};
use cc_core::error::ClaudeError;
use cc_core::types::{ContentBlock, Message, MessageContent, Role, ToolDefinition, UsageInfo};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Public re-exports
// ---------------------------------------------------------------------------
pub use client::AnthropicClient;
pub use openai_compat::OpenAiCompatClient;
pub use streaming::{StreamEvent, StreamHandler};
pub use types::*;

#[async_trait::async_trait]
pub trait LlmClient: Send + Sync {
    async fn create_message_stream(
        &self,
        request: CreateMessageRequest,
        handler: Arc<dyn StreamHandler>,
    ) -> Result<mpsc::Receiver<StreamEvent>, ClaudeError>;
}

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

    /// Events emitted by the streaming SSE parser.
    #[derive(Debug, Clone)]
    pub enum StreamEvent {
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
        ContentBlockDelta {
            index: usize,
            delta: ContentDelta,
        },
        /// A content block is finished.
        ContentBlockStop {
            index: usize,
        },
        /// Final message-level delta (stop_reason, usage).
        MessageDelta {
            stop_reason: Option<String>,
            usage: Option<UsageInfo>,
        },
        /// The message is complete.
        MessageStop,
        /// An error occurred during streaming.
        Error {
            error_type: String,
            message: String,
        },
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
        fn on_event(&self, event: &StreamEvent);
    }

    /// A no-op handler useful for non-interactive / batch mode.
    pub struct NullStreamHandler;
    impl StreamHandler for NullStreamHandler {
        fn on_event(&self, _event: &StreamEvent) {}
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
// Anthropic client
// ---------------------------------------------------------------------------
pub mod client {
    use super::*;

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
    }

    impl Default for ClientConfig {
        fn default() -> Self {
            Self {
                api_key: String::new(),
                api_base: cc_core::constants::ANTHROPIC_API_BASE.to_string(),
                api_version: ANTHROPIC_API_VERSION.to_string(),
                beta_features: ANTHROPIC_BETA_HEADER.to_string(),
                max_retries: 5,
                initial_retry_delay: Duration::from_secs(1),
                max_retry_delay: Duration::from_secs(60),
                request_timeout: Duration::from_secs(600),
                use_bearer_auth: false,
            }
        }
    }

    /// The main Anthropic API client.
    pub struct AnthropicClient {
        http: reqwest::Client,
        config: ClientConfig,
    }

    impl AnthropicClient {
        /// Build a new client.  Panics if `config.api_key` is empty.
        pub fn new(config: ClientConfig) -> anyhow::Result<Self> {
            if config.api_key.is_empty() {
                return Err(anyhow::anyhow!(
                    "Anthropic API key is required. Set ANTHROPIC_API_KEY or pass --api-key."
                ));
            }

            let http = reqwest::Client::builder()
                .timeout(config.request_timeout)
                .build()?;

            Ok(Self { http, config })
        }

        /// Convenience constructor that resolves the key from config/env.
        pub fn from_config(cfg: &cc_core::config::Config) -> anyhow::Result<Self> {
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

        // ---- Streaming create message ------------------------------------

        /// Send a streaming `POST /v1/messages`.  Events are dispatched to the
        /// provided `handler` in real time, and also forwarded into the returned
        /// channel so the caller can drive a select loop.
        pub async fn create_message_stream(
            &self,
            mut request: CreateMessageRequest,
            handler: Arc<dyn StreamHandler>,
        ) -> Result<mpsc::Receiver<StreamEvent>, ClaudeError> {
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
                        .send(StreamEvent::Error {
                            error_type: "stream_error".into(),
                            message: e.to_string(),
                        })
                        .await;
                }
            });

            Ok(rx)
        }

        // ---- Internal helpers --------------------------------------------

        /// Build the common request and execute with retry logic.
        async fn send_with_retry(
            &self,
            body: &Value,
        ) -> Result<reqwest::Response, ClaudeError> {
            let url = format!("{}/v1/messages", self.config.api_base);
            let mut attempts = 0u32;
            let mut delay = self.config.initial_retry_delay;

            loop {
                attempts += 1;

                // Use Bearer auth for Claude.ai OAuth tokens; x-api-key for regular keys.
                let mut req = self
                    .http
                    .post(&url)
                    .header("anthropic-version", &self.config.api_version)
                    .header("anthropic-beta", &self.config.beta_features)
                    .header("content-type", "application/json")
                    .header("accept", "text/event-stream");
                req = if self.config.use_bearer_auth {
                    req.header("Authorization", format!("Bearer {}", &self.config.api_key))
                } else {
                    req.header("x-api-key", &self.config.api_key)
                };
                let req = req.json(body);

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

        /// Read an SSE byte stream, parse frames, and emit `StreamEvent`s.
        async fn process_sse_stream(
            resp: reqwest::Response,
            handler: Arc<dyn StreamHandler>,
            tx: mpsc::Sender<StreamEvent>,
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
                        if let Some(event) =
                            Self::frame_to_event(&frame.event, &frame.data)
                        {
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

        /// Convert a parsed SSE frame into a typed `StreamEvent`.
        fn frame_to_event(
            event_type: &Option<String>,
            data: &str,
        ) -> Option<StreamEvent> {
            let event_name = event_type.as_deref().unwrap_or("");

            match event_name {
                "ping" => Some(StreamEvent::Ping),

                "message_start" => {
                    let v: Value = serde_json::from_str(data).ok()?;
                    let msg = v.get("message")?;
                    let id = msg.get("id")?.as_str()?.to_string();
                    let model = msg.get("model")?.as_str()?.to_string();
                    let usage = msg
                        .get("usage")
                        .and_then(|u| serde_json::from_value::<UsageInfo>(u.clone()).ok())
                        .unwrap_or_default();

                    Some(StreamEvent::MessageStart { id, model, usage })
                }

                "content_block_start" => {
                    let v: Value = serde_json::from_str(data).ok()?;
                    let index = v.get("index")?.as_u64()? as usize;
                    let block_value = v.get("content_block")?;
                    let content_block: ContentBlock =
                        serde_json::from_value(block_value.clone()).ok()?;
                    Some(StreamEvent::ContentBlockStart {
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
                    Some(StreamEvent::ContentBlockDelta { index, delta })
                }

                "content_block_stop" => {
                    let v: Value = serde_json::from_str(data).ok()?;
                    let index = v.get("index")?.as_u64()? as usize;
                    Some(StreamEvent::ContentBlockStop { index })
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
                    Some(StreamEvent::MessageDelta { stop_reason, usage })
                }

                "message_stop" => Some(StreamEvent::MessageStop),

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
                    Some(StreamEvent::Error {
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

#[async_trait::async_trait]
impl LlmClient for client::AnthropicClient {
    async fn create_message_stream(
        &self,
        request: CreateMessageRequest,
        handler: Arc<dyn StreamHandler>,
    ) -> Result<mpsc::Receiver<StreamEvent>, ClaudeError> {
        client::AnthropicClient::create_message_stream(self, request, handler).await
    }
}

pub mod openai_compat {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    /// OpenAI-compatible chat/completions streaming client.
    pub struct OpenAiCompatClient {
        http: reqwest::Client,
        config: client::ClientConfig,
    }

    impl OpenAiCompatClient {
        pub fn new(config: client::ClientConfig) -> anyhow::Result<Self> {
            if config.api_key.is_empty() {
                return Err(anyhow::anyhow!(
                    "API key is required for openai_compat provider."
                ));
            }
            let http = reqwest::Client::builder()
                .timeout(config.request_timeout)
                .build()?;
            Ok(Self { http, config })
        }

        fn normalize_base(base: &str) -> String {
            base.trim_end_matches('/').to_string()
        }

        fn system_to_string(system: Option<SystemPrompt>) -> Option<String> {
            match system {
                None => None,
                Some(SystemPrompt::Text(t)) => Some(t),
                Some(SystemPrompt::Blocks(blocks)) => {
                    let mut s = String::new();
                    for b in blocks {
                        if !s.is_empty() {
                            s.push_str("\n\n");
                        }
                        s.push_str(&b.text);
                    }
                    if s.trim().is_empty() { None } else { Some(s) }
                }
            }
        }

        fn map_messages(request: &CreateMessageRequest) -> Vec<Value> {
            let mut out = Vec::new();
            if let Some(system_text) = Self::system_to_string(request.system.clone()) {
                out.push(serde_json::json!({ "role": "system", "content": system_text }));
            }
            for m in &request.messages {
                let content = if let Some(s) = m.content.as_str() {
                    Value::String(s.to_string())
                } else {
                    // Keep compatibility for non-text content blocks by passing JSON.
                    Value::String(m.content.to_string())
                };
                out.push(serde_json::json!({
                    "role": m.role,
                    "content": content
                }));
            }
            out
        }

        fn map_tools(request: &CreateMessageRequest) -> Option<Value> {
            let tools = request.tools.as_ref()?;
            let mapped: Vec<Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema
                        }
                    })
                })
                .collect();
            Some(Value::Array(mapped))
        }

        fn map_finish_reason(reason: Option<&str>) -> Option<String> {
            match reason {
                Some("tool_calls") => Some("tool_use".to_string()),
                Some("length") => Some("max_tokens".to_string()),
                Some("stop") => Some("end_turn".to_string()),
                Some(v) => Some(v.to_string()),
                None => None,
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for OpenAiCompatClient {
        async fn create_message_stream(
            &self,
            request: CreateMessageRequest,
            handler: Arc<dyn StreamHandler>,
        ) -> Result<mpsc::Receiver<StreamEvent>, ClaudeError> {
            let url = format!(
                "{}/v1/chat/completions",
                Self::normalize_base(&self.config.api_base)
            );

            let mut body = serde_json::json!({
                "model": request.model,
                "messages": Self::map_messages(&request),
                "stream": true,
                "max_tokens": request.max_tokens,
            });
            if let Some(temp) = request.temperature {
                body["temperature"] = serde_json::json!(temp);
            }
            if let Some(top_p) = request.top_p {
                body["top_p"] = serde_json::json!(top_p);
            }
            if let Some(stop) = request.stop_sequences.clone() {
                body["stop"] = serde_json::json!(stop);
            }
            if let Some(tools) = Self::map_tools(&request) {
                body["tools"] = tools;
            }

            let resp = self
                .http
                .post(&url)
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .bearer_auth(&self.config.api_key)
                .json(&body)
                .send()
                .await
                .map_err(ClaudeError::Http)?;

            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                if status == 401 || status == 403 {
                    return Err(ClaudeError::Auth(text));
                }
                if status == 429 {
                    return Err(ClaudeError::RateLimit);
                }
                return Err(ClaudeError::ApiStatus {
                    status,
                    message: text,
                });
            }

            let (tx, rx) = mpsc::channel(256);
            tokio::spawn(async move {
                use sse_parser::SseLineParser;

                let mut parser = SseLineParser::new();
                let mut byte_stream = resp.bytes_stream();
                let mut leftover = String::new();
                let mut started = false;
                let mut text_block_index: Option<usize> = None;
                let mut next_index: usize = 0;
                let mut tool_blocks: HashMap<u64, (usize, String, String)> = HashMap::new();
                let mut stop_reason: Option<String> = None;

                while let Some(chunk_result) = byte_stream.next().await {
                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.send(StreamEvent::Error {
                                error_type: "stream_error".to_string(),
                                message: e.to_string(),
                            }).await;
                            return;
                        }
                    };
                    let text = String::from_utf8_lossy(&chunk);
                    let combined = if leftover.is_empty() {
                        text.to_string()
                    } else {
                        let mut s = std::mem::take(&mut leftover);
                        s.push_str(&text);
                        s
                    };
                    let mut lines: Vec<&str> = combined.split('\n').collect();
                    if !combined.ends_with('\n') {
                        leftover = lines.pop().unwrap_or("").to_string();
                    }
                    for line in lines {
                        let line = line.trim_end_matches('\r');
                        let Some(frame) = parser.feed_line(line) else { continue; };
                        if frame.data.trim() == "[DONE]" {
                            break;
                        }

                        let Ok(v) = serde_json::from_str::<Value>(&frame.data) else {
                            continue;
                        };
                        if let Some(err) = v.get("error") {
                            let error_type = err
                                .get("type")
                                .and_then(|x| x.as_str())
                                .unwrap_or("api_error")
                                .to_string();
                            let message = err
                                .get("message")
                                .and_then(|x| x.as_str())
                                .unwrap_or("Unknown provider error")
                                .to_string();
                            let evt = StreamEvent::Error { error_type, message };
                            handler.on_event(&evt);
                            let _ = tx.send(evt).await;
                            let stop_evt = StreamEvent::MessageStop;
                            handler.on_event(&stop_evt);
                            let _ = tx.send(stop_evt).await;
                            return;
                        }
                        if !started {
                            let message_id = v.get("id").and_then(|x| x.as_str()).map(str::to_string);
                            let message_model = v.get("model").and_then(|x| x.as_str()).map(str::to_string);
                            let evt = StreamEvent::MessageStart {
                                id: message_id.clone().unwrap_or_else(|| "openai-chat".to_string()),
                                model: message_model.clone().unwrap_or_else(|| "openai".to_string()),
                                usage: UsageInfo::default(),
                            };
                            handler.on_event(&evt);
                            if tx.send(evt).await.is_err() { return; }
                            started = true;
                        }

                        let choice = v.get("choices")
                            .and_then(|c| c.as_array())
                            .and_then(|arr| arr.first());
                        let Some(choice) = choice else { continue; };
                        let mut should_terminate = false;

                        if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                            stop_reason = Self::map_finish_reason(Some(reason));
                            should_terminate = true;
                        }
                        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);

                        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                            if !content.is_empty() {
                                if text_block_index.is_none() {
                                    let idx = next_index;
                                    next_index += 1;
                                    text_block_index = Some(idx);
                                    let start_evt = StreamEvent::ContentBlockStart {
                                        index: idx,
                                        content_block: ContentBlock::Text { text: String::new() },
                                    };
                                    handler.on_event(&start_evt);
                                    if tx.send(start_evt).await.is_err() { return; }
                                }
                                let idx = text_block_index.unwrap_or(0);
                                let delta_evt = StreamEvent::ContentBlockDelta {
                                    index: idx,
                                    delta: streaming::ContentDelta::TextDelta { text: content.to_string() },
                                };
                                handler.on_event(&delta_evt);
                                if tx.send(delta_evt).await.is_err() { return; }
                            }
                        }

                        if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                            for tc in tool_calls {
                                let tool_idx = tc.get("index").and_then(|x| x.as_u64()).unwrap_or(0);
                                if !tool_blocks.contains_key(&tool_idx) {
                                    let idx = next_index;
                                    next_index += 1;
                                    let id = tc.get("id")
                                        .and_then(|x| x.as_str())
                                        .map(str::to_string)
                                        .unwrap_or_else(|| format!("tool_call_{}", tool_idx));
                                    let name = tc.get("function")
                                        .and_then(|f| f.get("name"))
                                        .and_then(|x| x.as_str())
                                        .map(str::to_string)
                                        .unwrap_or_else(|| "tool".to_string());
                                    let start_evt = StreamEvent::ContentBlockStart {
                                        index: idx,
                                        content_block: ContentBlock::ToolUse {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input: Value::Object(Default::default()),
                                        },
                                    };
                                    handler.on_event(&start_evt);
                                    if tx.send(start_evt).await.is_err() { return; }
                                    tool_blocks.insert(tool_idx, (idx, id, name));
                                }
                                let entry = match tool_blocks.get_mut(&tool_idx) {
                                    Some(e) => e,
                                    None => continue,
                                };

                                if let Some(name) = tc.get("function")
                                    .and_then(|f| f.get("name"))
                                    .and_then(|x| x.as_str()) {
                                    entry.2 = name.to_string();
                                }
                                if let Some(args) = tc.get("function")
                                    .and_then(|f| f.get("arguments"))
                                    .and_then(|x| x.as_str()) {
                                    if !args.is_empty() {
                                        let delta_evt = StreamEvent::ContentBlockDelta {
                                            index: entry.0,
                                            delta: streaming::ContentDelta::InputJsonDelta {
                                                partial_json: args.to_string(),
                                            },
                                        };
                                        handler.on_event(&delta_evt);
                                        if tx.send(delta_evt).await.is_err() { return; }
                                    }
                                }
                            }
                        }
                        if should_terminate {
                            break;
                        }
                    }
                }

                if let Some(idx) = text_block_index {
                    let stop_evt = StreamEvent::ContentBlockStop { index: idx };
                    handler.on_event(&stop_evt);
                    if tx.send(stop_evt).await.is_err() { return; }
                }

                let mut ordered_tool_blocks: BTreeMap<usize, ()> = BTreeMap::new();
                for (idx, _, _) in tool_blocks.values() {
                    ordered_tool_blocks.insert(*idx, ());
                }
                for idx in ordered_tool_blocks.keys() {
                    let stop_evt = StreamEvent::ContentBlockStop { index: *idx };
                    handler.on_event(&stop_evt);
                    if tx.send(stop_evt).await.is_err() { return; }
                }

                let delta_evt = StreamEvent::MessageDelta {
                    stop_reason: stop_reason.or_else(|| Some("end_turn".to_string())),
                    usage: None,
                };
                handler.on_event(&delta_evt);
                if tx.send(delta_evt).await.is_err() { return; }

                let stop_evt = StreamEvent::MessageStop;
                handler.on_event(&stop_evt);
                let _ = tx.send(stop_evt).await;
            });

            Ok(rx)
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
    pub fn on_event(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::MessageStart { id, model, usage } => {
                self.id = Some(id.clone());
                self.model = Some(model.clone());
                self.usage = usage.clone();
            }

            StreamEvent::ContentBlockStart {
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
                    ContentBlock::Thinking { thinking, signature } => PartialBlock::Thinking {
                        thinking_buf: thinking.clone(),
                        signature_buf: signature.clone(),
                    },
                    _ => return,
                };
                self.partials.insert(*index, partial);
            }

            StreamEvent::ContentBlockDelta { index, delta } => {
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

            StreamEvent::ContentBlockStop { index } => {
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

            StreamEvent::MessageDelta { stop_reason, usage } => {
                if let Some(sr) = stop_reason {
                    self.stop_reason = Some(sr.clone());
                }
                if let Some(u) = usage {
                    // The delta usage usually only has output_tokens;
                    // add them to the running total.
                    self.usage.output_tokens += u.output_tokens;
                }
            }

            StreamEvent::MessageStop => {}
            StreamEvent::Ping => {}
            StreamEvent::Error { .. } => {}
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
        acc.on_event(&StreamEvent::MessageStart {
            id: "m1".into(),
            model: "claude".into(),
            usage: UsageInfo::default(),
        });
        acc.on_event(&StreamEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::Text {
                text: String::new(),
            },
        });
        acc.on_event(&StreamEvent::ContentBlockDelta {
            index: 0,
            delta: streaming::ContentDelta::TextDelta {
                text: "Hello ".into(),
            },
        });
        acc.on_event(&StreamEvent::ContentBlockDelta {
            index: 0,
            delta: streaming::ContentDelta::TextDelta {
                text: "world!".into(),
            },
        });
        acc.on_event(&StreamEvent::ContentBlockStop { index: 0 });
        acc.on_event(&StreamEvent::MessageDelta {
            stop_reason: Some("end_turn".into()),
            usage: None,
        });
        acc.on_event(&StreamEvent::MessageStop);

        let (msg, _usage, stop) = acc.finish();
        assert_eq!(msg.get_text(), Some("Hello world!"));
        assert_eq!(stop.as_deref(), Some("end_turn"));
    }
}
