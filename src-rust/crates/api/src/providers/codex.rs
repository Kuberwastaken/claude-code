// providers/codex.rs — OpenAI Codex provider (OAuth-authenticated).
//
// Codex uses OpenAI's Responses API at:
//   https://chatgpt.com/backend-api/codex/responses
//
// Auth: Bearer token obtained via the Codex OAuth flow stored in
//   ~/.claurst/codex_tokens.json (`CodexTokens` struct).
//
// Token refresh: if `expires_at` is in the past we POST to the OpenAI token
//   endpoint with `grant_type=refresh_token` before making the request.
//
// Model list: static — the Codex endpoint does not expose a /models route,
//   so we use the `CODEX_MODELS` constant from `claurst-core`.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_stream::stream;
use async_trait::async_trait;
use claurst_core::codex_oauth::{
    CODEX_API_ENDPOINT, CODEX_MODELS, CODEX_TOKEN_URL, DEFAULT_CODEX_MODEL,
};
use claurst_core::oauth_config::{get_codex_tokens, save_codex_tokens, CodexTokens};
use claurst_core::provider_id::{ModelId, ProviderId};
use claurst_core::types::UsageInfo;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use tracing::{debug, warn};

use crate::error_handling::parse_error_response;
use crate::provider::{LlmProvider, ModelInfo};
use crate::provider_error::ProviderError;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StopReason,
    StreamEvent, SystemPromptStyle,
};

// Re-use Copilot's message translation helpers via the public Copilot type.
use crate::providers::copilot::CopilotProvider;

// ---------------------------------------------------------------------------
// CodexProvider
// ---------------------------------------------------------------------------

pub struct CodexProvider {
    id: ProviderId,
    http_client: reqwest::Client,
    /// Mutable token cache: updated in-place when a refresh succeeds.
    tokens: Arc<Mutex<CodexTokens>>,
}

impl CodexProvider {
    pub fn new(tokens: CodexTokens) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("failed to build reqwest client");

        Self {
            id: ProviderId::new(ProviderId::CODEX),
            http_client,
            tokens: Arc::new(Mutex::new(tokens)),
        }
    }

    /// Construct from stored tokens; returns `None` if no tokens are saved.
    pub fn from_stored() -> Option<Self> {
        let tokens = get_codex_tokens()?;
        if tokens.access_token.is_empty() {
            return None;
        }
        Some(Self::new(tokens))
    }

    // -----------------------------------------------------------------------
    // Token management
    // -----------------------------------------------------------------------

    fn is_expired(tokens: &CodexTokens) -> bool {
        let Some(expires_at) = tokens.expires_at else {
            return false; // No expiry info — assume still valid.
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Treat as expired 60 s early to avoid races.
        now + 60 >= expires_at
    }

    /// Return the current access token, refreshing first if it is expired.
    async fn access_token(&self) -> Result<String, ProviderError> {
        // Check expiry under the lock; clone what we need; release.
        let (token, needs_refresh, refresh_token) = {
            let guard = self.tokens.lock().unwrap();
            let expired = Self::is_expired(&guard);
            (
                guard.access_token.clone(),
                expired,
                guard.refresh_token.clone(),
            )
        };

        if !needs_refresh {
            return Ok(token);
        }

        let Some(refresh) = refresh_token else {
            // No refresh token — return what we have and hope for the best.
            warn!("Codex access token is expired and no refresh token is available");
            return Ok(token);
        };

        debug!("Codex access token expired — refreshing");
        self.refresh_token(&refresh).await
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<String, ProviderError> {
        let body = json!({
            "grant_type": "refresh_token",
            "client_id": claurst_core::codex_oauth::CODEX_CLIENT_ID,
            "refresh_token": refresh_token,
        });

        let resp = self
            .http_client
            .post(CODEX_TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Other {
                provider: self.id.clone(),
                message: format!("Token refresh request failed: {}", e),
                status: None,
                body: None,
            })?;

        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| ProviderError::Other {
            provider: self.id.clone(),
            message: format!("Failed to read token refresh response: {}", e),
            status: Some(status),
            body: None,
        })?;

        if !(200..300).contains(&(status as usize)) {
            return Err(ProviderError::Other {
                provider: self.id.clone(),
                message: format!("Token refresh failed (HTTP {})", status),
                status: Some(status),
                body: Some(text),
            });
        }

        let json_val: Value = serde_json::from_str(&text).map_err(|e| ProviderError::Other {
            provider: self.id.clone(),
            message: format!("Failed to parse token refresh response: {}", e),
            status: Some(status),
            body: Some(text.clone()),
        })?;

        let new_access = json_val
            .get("access_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if new_access.is_empty() {
            return Err(ProviderError::Other {
                provider: self.id.clone(),
                message: "Token refresh response missing access_token".to_string(),
                status: Some(status),
                body: Some(text),
            });
        }

        let new_refresh = json_val
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let expires_in = json_val.get("expires_in").and_then(|v| v.as_u64());

        let new_expires_at = expires_in.map(|secs| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                + secs
        });

        // Persist and cache the refreshed tokens.
        let mut updated = {
            let guard = self.tokens.lock().unwrap();
            guard.clone()
        };
        updated.access_token = new_access.clone();
        if let Some(r) = new_refresh {
            updated.refresh_token = Some(r);
        }
        updated.expires_at = new_expires_at;

        if let Err(e) = save_codex_tokens(&updated) {
            warn!("Failed to persist refreshed Codex tokens: {}", e);
        }

        {
            let mut guard = self.tokens.lock().unwrap();
            *guard = updated;
        }

        Ok(new_access)
    }

    // -----------------------------------------------------------------------
    // Request helpers
    // -----------------------------------------------------------------------

    fn codex_headers(
        &self,
        builder: reqwest::RequestBuilder,
        token: &str,
        account_id: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let builder = builder
            .bearer_auth(token)
            .header("User-Agent", concat!("claurst/", env!("CARGO_PKG_VERSION")));

        if let Some(id) = account_id {
            builder.header("ChatGPT-Account-Id", id)
        } else {
            builder
        }
    }

    fn account_id(&self) -> Option<String> {
        self.tokens.lock().unwrap().account_id.clone()
    }

    fn system_prompt_to_text(request: &ProviderRequest) -> String {
        match request.system_prompt.as_ref() {
            Some(crate::provider_types::SystemPrompt::Text(text)) => text.clone(),
            Some(crate::provider_types::SystemPrompt::Blocks(blocks)) => blocks
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            None => String::new(),
        }
    }

    /// Build the Responses-API request body for Codex.
    fn build_responses_body(request: &ProviderRequest) -> Value {
        // Re-use the same message translation that the Copilot provider uses.
        let input = CopilotProvider::to_responses_input_pub(request);
        let instructions = Self::system_prompt_to_text(request);

        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                    "strict": false,
                })
            })
            .collect();

        let mut body = json!({
            "model": request.model,
            "input": input,
            "instructions": instructions,
            "store": false,
        });

        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }

        body
    }

    fn extract_stream_error_message(json_val: &Value) -> String {
        json_val
            .pointer("/error/message")
            .and_then(|value| value.as_str())
            .or_else(|| {
                json_val
                    .pointer("/response/error/message")
                    .and_then(|value| value.as_str())
            })
            .or_else(|| {
                json_val
                    .pointer("/message")
                    .and_then(|value| value.as_str())
            })
            .map(|value| value.to_string())
            .unwrap_or_else(|| json_val.to_string())
    }

    // -----------------------------------------------------------------------
    // HTTP call
    // -----------------------------------------------------------------------

    async fn send_responses_request(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        let token = self.access_token().await?;
        let account_id = self.account_id();

        let body = Self::build_responses_body(request);

        let builder = self
            .http_client
            .post(CODEX_API_ENDPOINT)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
        let builder = self.codex_headers(builder, &token, account_id.as_deref());

        let resp = builder
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Other {
                provider: self.id.clone(),
                message: format!("HTTP request failed: {}", e),
                status: None,
                body: None,
            })?;

        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| ProviderError::Other {
            provider: self.id.clone(),
            message: format!("Failed to read response body: {}", e),
            status: Some(status),
            body: None,
        })?;

        if !(200..300).contains(&(status as usize)) {
            return Err(parse_error_response(status, &text, &self.id));
        }

        let json_val: Value = serde_json::from_str(&text).map_err(|e| ProviderError::Other {
            provider: self.id.clone(),
            message: format!("Failed to parse response JSON: {}", e),
            status: Some(status),
            body: Some(text.clone()),
        })?;

        Self::parse_responses_response(&self.id, &json_val)
    }

    async fn send_responses_streaming_request(
        &self,
        request: &ProviderRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let token = self.access_token().await?;
        let account_id = self.account_id();

        let mut body = Self::build_responses_body(request);
        body["stream"] = json!(true);

        let builder = self
            .http_client
            .post(CODEX_API_ENDPOINT)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");
        let builder = self.codex_headers(builder, &token, account_id.as_deref());

        let resp = builder
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Other {
                provider: self.id.clone(),
                message: format!("HTTP request failed: {}", e),
                status: None,
                body: None,
            })?;

        let status = resp.status().as_u16();
        if !(200..300).contains(&(status as usize)) {
            let text = resp.text().await.map_err(|e| ProviderError::Other {
                provider: self.id.clone(),
                message: format!("Failed to read response body: {}", e),
                status: Some(status),
                body: None,
            })?;
            return Err(parse_error_response(status, &text, &self.id));
        }

        Ok(resp)
    }

    // -----------------------------------------------------------------------
    // Response parsing  (mirrors CopilotProvider::parse_responses_response)
    // -----------------------------------------------------------------------

    fn parse_responses_response(
        provider_id: &ProviderId,
        json_val: &Value,
    ) -> Result<ProviderResponse, ProviderError> {
        use claurst_core::types::ContentBlock;

        let id = json_val
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let model = json_val
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_CODEX_MODEL)
            .to_string();

        let output = json_val
            .get("output")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ProviderError::Other {
                provider: provider_id.clone(),
                message: "No output in Codex Responses API response".to_string(),
                status: None,
                body: Some(json_val.to_string()),
            })?;

        let mut content: Vec<ContentBlock> = Vec::new();
        let mut has_tool_call = false;

        for item in output {
            match item.get("type").and_then(|v| v.as_str()) {
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(|v| v.as_array()) {
                        for part in parts {
                            match part.get("type").and_then(|v| v.as_str()) {
                                Some("output_text") | Some("text") => {
                                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                        if !text.is_empty() {
                                            content.push(ContentBlock::Text {
                                                text: text.to_string(),
                                            });
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Some("reasoning") => {
                    if let Some(summaries) = item.get("summary").and_then(|v| v.as_array()) {
                        let reasoning: String = summaries
                            .iter()
                            .filter_map(|s| s.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("");
                        if !reasoning.is_empty() {
                            content.push(ContentBlock::Thinking {
                                thinking: reasoning,
                                signature: String::new(),
                            });
                        }
                    }
                }
                Some("function_call") => {
                    has_tool_call = true;
                    let id = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = item
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}");
                    let input = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                    content.push(ContentBlock::ToolUse { id, name, input });
                }
                _ => {}
            }
        }

        let stop_reason = if has_tool_call {
            StopReason::ToolUse
        } else {
            match json_val
                .get("incomplete_details")
                .and_then(|v| v.get("reason"))
                .and_then(|v| v.as_str())
            {
                Some("max_output_tokens") => StopReason::MaxTokens,
                Some("content_filter") => StopReason::ContentFiltered,
                Some(other) if !other.is_empty() => StopReason::Other(other.to_string()),
                _ => StopReason::EndTurn,
            }
        };

        let usage = {
            let u = json_val.get("usage");
            UsageInfo {
                input_tokens: u
                    .and_then(|v| v.get("input_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                output_tokens: u
                    .and_then(|v| v.get("output_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            }
        };

        Ok(ProviderResponse {
            id,
            content,
            stop_reason,
            usage,
            model,
        })
    }

}

// ---------------------------------------------------------------------------
// LlmProvider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl LlmProvider for CodexProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        "OpenAI Codex"
    }

    async fn create_message(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        self.send_responses_request(&request).await
    }

    async fn create_message_stream(
        &self,
        request: ProviderRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        let resp = self.send_responses_streaming_request(&request).await?;
        let provider_id = self.id.clone();

        let s = stream! {
            let mut byte_stream = resp.bytes_stream();
            let mut leftover = String::new();
            let mut current_event = String::new();
            let mut current_data: Vec<String> = Vec::new();
            let mut message_started = false;
            let mut message_id = String::from("unknown");
            let mut model_name = String::from(DEFAULT_CODEX_MODEL);
            let mut saw_tool_call = false;
            let mut open_blocks: std::collections::HashSet<usize> = std::collections::HashSet::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(chunk) => chunk,
                    Err(e) => {
                        yield Err(ProviderError::StreamError {
                            provider: provider_id.clone(),
                            message: format!("Stream read error: {}", e),
                            partial_response: None,
                        });
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

                for raw_line in lines {
                    let line = raw_line.trim_end_matches('\r');

                    if line.is_empty() {
                        if current_data.is_empty() {
                            current_event.clear();
                            continue;
                        }

                        let data = current_data.join("\n");
                        current_data.clear();
                        let trimmed = data.trim();
                        if trimmed.is_empty() || trimmed == "[DONE]" {
                            current_event.clear();
                            continue;
                        }

                        let json_val: Value = match serde_json::from_str(trimmed) {
                            Ok(value) => value,
                            Err(e) => {
                                yield Err(ProviderError::StreamError {
                                    provider: provider_id.clone(),
                                    message: format!("Failed to parse Codex stream JSON: {}", e),
                                    partial_response: Some(trimmed.to_string()),
                                });
                                return;
                            }
                        };

                        let event_name = if current_event.is_empty() {
                            json_val
                                .get("type")
                                .and_then(|value| value.as_str())
                                .unwrap_or("")
                                .to_string()
                        } else {
                            current_event.clone()
                        };

                        match event_name.as_str() {
                            "response.created" | "response.in_progress" => {
                                if let Some(response) = json_val.get("response") {
                                    if let Some(id) = response.get("id").and_then(|value| value.as_str()) {
                                        message_id = id.to_string();
                                    }
                                    if let Some(model) = response.get("model").and_then(|value| value.as_str()) {
                                        model_name = model.to_string();
                                    }
                                    if !message_started {
                                        yield Ok(StreamEvent::MessageStart {
                                            id: message_id.clone(),
                                            model: model_name.clone(),
                                            usage: UsageInfo::default(),
                                        });
                                        message_started = true;
                                    }
                                }
                            }
                            "response.output_item.added" => {
                                let output_index = json_val
                                    .get("output_index")
                                    .and_then(|value| value.as_u64())
                                    .unwrap_or(0) as usize;
                                if let Some(item) = json_val.get("item") {
                                    match item.get("type").and_then(|value| value.as_str()) {
                                        Some("message") => {
                                            if let Some(id) = item.get("id").and_then(|value| value.as_str()) {
                                                message_id = id.to_string();
                                            }
                                            if !message_started {
                                                yield Ok(StreamEvent::MessageStart {
                                                    id: message_id.clone(),
                                                    model: model_name.clone(),
                                                    usage: UsageInfo::default(),
                                                });
                                                message_started = true;
                                            }
                                        }
                                        Some("function_call") => {
                                            saw_tool_call = true;
                                            let call_id = item
                                                .get("call_id")
                                                .or_else(|| item.get("id"))
                                                .and_then(|value| value.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let name = item
                                                .get("name")
                                                .and_then(|value| value.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            if open_blocks.insert(output_index) {
                                                yield Ok(StreamEvent::ContentBlockStart {
                                                    index: output_index,
                                                    content_block: claurst_core::types::ContentBlock::ToolUse {
                                                        id: call_id,
                                                        name,
                                                        input: json!({}),
                                                    },
                                                });
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            "response.content_part.added" => {
                                let output_index = json_val
                                    .get("output_index")
                                    .and_then(|value| value.as_u64())
                                    .unwrap_or(0) as usize;
                                if let Some(part) = json_val.get("part") {
                                    if matches!(part.get("type").and_then(|value| value.as_str()), Some("output_text") | Some("text")) {
                                        if !message_started {
                                            yield Ok(StreamEvent::MessageStart {
                                                id: message_id.clone(),
                                                model: model_name.clone(),
                                                usage: UsageInfo::default(),
                                            });
                                            message_started = true;
                                        }
                                        if open_blocks.insert(output_index) {
                                            yield Ok(StreamEvent::ContentBlockStart {
                                                index: output_index,
                                                content_block: claurst_core::types::ContentBlock::Text {
                                                    text: String::new(),
                                                },
                                            });
                                        }
                                    }
                                }
                            }
                            "response.output_text.delta" => {
                                let output_index = json_val
                                    .get("output_index")
                                    .and_then(|value| value.as_u64())
                                    .unwrap_or(0) as usize;
                                let delta = json_val
                                    .get("delta")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("");
                                if !message_started {
                                    yield Ok(StreamEvent::MessageStart {
                                        id: message_id.clone(),
                                        model: model_name.clone(),
                                        usage: UsageInfo::default(),
                                    });
                                    message_started = true;
                                }
                                if open_blocks.insert(output_index) {
                                    yield Ok(StreamEvent::ContentBlockStart {
                                        index: output_index,
                                        content_block: claurst_core::types::ContentBlock::Text {
                                            text: String::new(),
                                        },
                                    });
                                }
                                if !delta.is_empty() {
                                    yield Ok(StreamEvent::TextDelta {
                                        index: output_index,
                                        text: delta.to_string(),
                                    });
                                }
                            }
                            "response.function_call_arguments.delta" => {
                                let output_index = json_val
                                    .get("output_index")
                                    .and_then(|value| value.as_u64())
                                    .unwrap_or(0) as usize;
                                let delta = json_val
                                    .get("delta")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("");
                                if !delta.is_empty() {
                                    yield Ok(StreamEvent::InputJsonDelta {
                                        index: output_index,
                                        partial_json: delta.to_string(),
                                    });
                                }
                            }
                            "response.output_item.done" => {
                                let output_index = json_val
                                    .get("output_index")
                                    .and_then(|value| value.as_u64())
                                    .unwrap_or(0) as usize;
                                if open_blocks.remove(&output_index) {
                                    yield Ok(StreamEvent::ContentBlockStop { index: output_index });
                                }
                            }
                            "response.completed" => {
                                if let Some(response) = json_val.get("response") {
                                    if let Some(id) = response.get("id").and_then(|value| value.as_str()) {
                                        message_id = id.to_string();
                                    }
                                    if let Some(model) = response.get("model").and_then(|value| value.as_str()) {
                                        model_name = model.to_string();
                                    }

                                    if !message_started {
                                        yield Ok(StreamEvent::MessageStart {
                                            id: message_id.clone(),
                                            model: model_name.clone(),
                                            usage: UsageInfo::default(),
                                        });
                                        // No reassignment: this branch is the
                                        // stream-completion handler — execution
                                        // never re-enters the guard below.
                                    }

                                    let mut remaining: Vec<usize> = open_blocks.drain().collect();
                                    remaining.sort_unstable();
                                    for index in remaining {
                                        yield Ok(StreamEvent::ContentBlockStop { index });
                                    }

                                    let usage_json = response.get("usage");
                                    let usage = UsageInfo {
                                        input_tokens: usage_json
                                            .and_then(|value| value.get("input_tokens"))
                                            .and_then(|value| value.as_u64())
                                            .unwrap_or(0),
                                        output_tokens: usage_json
                                            .and_then(|value| value.get("output_tokens"))
                                            .and_then(|value| value.as_u64())
                                            .unwrap_or(0),
                                        cache_creation_input_tokens: 0,
                                        cache_read_input_tokens: usage_json
                                            .and_then(|value| value.get("input_tokens_details"))
                                            .and_then(|value| value.get("cached_tokens"))
                                            .and_then(|value| value.as_u64())
                                            .unwrap_or(0),
                                    };

                                    let stop_reason = if saw_tool_call {
                                        StopReason::ToolUse
                                    } else {
                                        match response
                                            .get("incomplete_details")
                                            .and_then(|value| value.get("reason"))
                                            .and_then(|value| value.as_str())
                                        {
                                            Some("max_output_tokens") => StopReason::MaxTokens,
                                            Some("content_filter") => StopReason::ContentFiltered,
                                            Some(other) if !other.is_empty() => StopReason::Other(other.to_string()),
                                            _ => StopReason::EndTurn,
                                        }
                                    };

                                    yield Ok(StreamEvent::MessageDelta {
                                        stop_reason: Some(stop_reason),
                                        usage: Some(usage),
                                    });
                                    yield Ok(StreamEvent::MessageStop);
                                    return;
                                }
                            }
                            "response.failed" | "response.error" | "error" => {
                                yield Err(ProviderError::StreamError {
                                    provider: provider_id.clone(),
                                    message: Self::extract_stream_error_message(&json_val),
                                    partial_response: Some(trimmed.to_string()),
                                });
                                return;
                            }
                            _ => {}
                        }

                        current_event.clear();
                        continue;
                    }

                    if let Some(rest) = line.strip_prefix("event:") {
                        current_event = rest.trim().to_string();
                        continue;
                    }

                    if let Some(rest) = line.strip_prefix("data:") {
                        current_data.push(rest.trim_start().to_string());
                    }
                }
            }

            yield Err(ProviderError::StreamError {
                provider: provider_id,
                message: "Codex stream ended before response.completed".to_string(),
                partial_response: None,
            });
        };

        Ok(Box::pin(s))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let models = CODEX_MODELS
            .iter()
            .map(|(id, name)| ModelInfo {
                id: ModelId::new(*id),
                provider_id: self.id.clone(),
                name: name.to_string(),
                context_window: match *id {
                    "gpt-5.4" | "gpt-5-codex" | "gpt-5-mini" => 400_000,
                    "gpt-4.1" => 128_000,
                    "o4-mini" => 200_000,
                    _ => 128_000,
                },
                max_output_tokens: 32_768,
            })
            .collect();
        Ok(models)
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        // Validate that a non-expired token exists without making a network call.
        let guard = self.tokens.lock().unwrap();
        if guard.access_token.is_empty() {
            return Ok(ProviderStatus::Unavailable {
                reason: "no Codex access token — run /connect to authenticate".to_string(),
            });
        }
        if Self::is_expired(&guard) && guard.refresh_token.is_none() {
            return Ok(ProviderStatus::Unavailable {
                reason: "Codex access token expired and no refresh token — re-run /connect"
                    .to_string(),
            });
        }
        Ok(ProviderStatus::Healthy)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            thinking: false,
            image_input: false,
            pdf_input: false,
            audio_input: false,
            video_input: false,
            caching: false,
            structured_output: false,
            system_prompt_style: SystemPromptStyle::SystemMessage,
        }
    }
}
