// providers/cohere.rs — Cohere provider adapter (Command R / Command R+).
//
// Cohere exposes a custom v2 chat API that is structurally similar to the
// OpenAI Chat Completions wire format but uses its own streaming event
// envelope.  This adapter maps the provider-agnostic ProviderRequest /
// ProviderResponse types onto the Cohere v2 wire format and parses the
// streaming JSON objects back into StreamEvents.

use std::pin::Pin;

use async_stream::stream;
use async_trait::async_trait;
use claurst_core::provider_id::{ModelId, ProviderId};
use claurst_core::types::{ContentBlock, UsageInfo};
use futures::Stream;
use serde_json::{json, Value};
use tracing::debug;

use crate::provider::{LlmProvider, ModelInfo};
use crate::provider_error::ProviderError;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StopReason,
    StreamEvent, SystemPromptStyle,
};

// Re-use OpenAI message transformation helpers since Cohere v2 uses the same
// messages array shape (role/content/tool_calls/tool_call_id).
use super::openai::OpenAiProvider;
use super::request_options::merge_root_options;

// ---------------------------------------------------------------------------
// CohereProvider
// ---------------------------------------------------------------------------

pub struct CohereProvider {
    id: ProviderId,
    api_key: String,
    http_client: reqwest::Client,
}

impl CohereProvider {
    /// Create a new CohereProvider with the given API key.
    pub fn new(api_key: String) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("failed to build reqwest client");

        Self {
            id: ProviderId::new(ProviderId::COHERE),
            api_key,
            http_client,
        }
    }

    /// Construct from the `COHERE_API_KEY` environment variable.
    /// Returns `None` if the variable is absent or empty.
    pub fn from_env() -> Option<Self> {
        std::env::var("COHERE_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .map(Self::new)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Build the Cohere v2 messages array from the provider-agnostic request.
    /// Cohere v2 uses the same shape as OpenAI Chat Completions, so we reuse
    /// the OpenAI transformation helper.
    fn build_messages(&self, request: &ProviderRequest) -> Vec<Value> {
        OpenAiProvider::to_openai_messages_pub(&request.messages, request.system_prompt.as_ref())
    }

    /// Build the Cohere v2 tools array.  Same shape as OpenAI function tools.
    fn build_tools(&self, request: &ProviderRequest) -> Vec<Value> {
        OpenAiProvider::to_openai_tools_pub(&request.tools)
    }

    /// Map an HTTP error response to a typed ProviderError.
    fn map_http_error(&self, status: u16, body: &str) -> ProviderError {
        // Cohere error format: {"message": "..."}
        let message = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| body.to_string());

        match status {
            401 | 403 => ProviderError::AuthFailed {
                provider: self.id.clone(),
                message,
            },
            404 => ProviderError::ModelNotFound {
                provider: self.id.clone(),
                model: message,
                suggestions: vec![],
            },
            429 => ProviderError::RateLimited {
                provider: self.id.clone(),
                retry_after: None,
            },
            400 => ProviderError::InvalidRequest {
                provider: self.id.clone(),
                message,
            },
            _ => ProviderError::ServerError {
                provider: self.id.clone(),
                status: Some(status),
                message,
                is_retryable: status >= 500,
            },
        }
    }

    // -----------------------------------------------------------------------
    // Non-streaming
    // -----------------------------------------------------------------------

    async fn create_message_non_streaming(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        let messages = self.build_messages(request);
        let tools = self.build_tools(request);

        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
            "stream": false,
        });

        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(p) = request.top_p {
            body["p"] = json!(p);
        }
        if !request.stop_sequences.is_empty() {
            body["stop_sequences"] = json!(request.stop_sequences);
        }
        merge_root_options(&mut body, &request.provider_options);

        let resp = self
            .http_client
            .post("https://api.cohere.ai/v2/chat")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
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
            return Err(self.map_http_error(status, &text));
        }

        let json: Value = serde_json::from_str(&text).map_err(|e| ProviderError::Other {
            provider: self.id.clone(),
            message: format!("Failed to parse response JSON: {}", e),
            status: Some(status),
            body: Some(text.clone()),
        })?;

        // Cohere v2 non-streaming response shape:
        // { "id": "...", "message": { "role": "assistant", "content": [...], "tool_calls": [...] },
        //   "finish_reason": "COMPLETE", "usage": { "tokens": { "input_tokens": N, "output_tokens": N } } }
        let resp_id = json
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let finish_reason = json
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("COMPLETE");
        let stop_reason = map_finish_reason(finish_reason);

        let usage = parse_cohere_usage(json.get("usage"));

        let mut content_blocks: Vec<ContentBlock> = Vec::new();

        if let Some(message) = json.get("message") {
            // Text content
            if let Some(content_arr) = message.get("content").and_then(|c| c.as_array()) {
                for item in content_arr {
                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            content_blocks.push(ContentBlock::Text {
                                text: text.to_string(),
                            });
                        }
                    }
                }
            }

            // Tool calls
            if let Some(tool_calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tool_calls {
                    let id = tc
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = tc
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input_str = tc
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}");
                    let input: Value =
                        serde_json::from_str(input_str).unwrap_or_else(|_| json!({}));
                    content_blocks.push(ContentBlock::ToolUse { id, name, input });
                }
            }
        }

        if content_blocks.is_empty() {
            content_blocks.push(ContentBlock::Text {
                text: String::new(),
            });
        }

        Ok(ProviderResponse {
            id: resp_id,
            content: content_blocks,
            stop_reason,
            usage,
            model: request.model.clone(),
        })
    }

    // -----------------------------------------------------------------------
    // Streaming
    // -----------------------------------------------------------------------

    async fn do_streaming(
        &self,
        request: &ProviderRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let messages = self.build_messages(request);
        let tools = self.build_tools(request);

        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
            "stream": true,
        });

        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(p) = request.top_p {
            body["p"] = json!(p);
        }
        if !request.stop_sequences.is_empty() {
            body["stop_sequences"] = json!(request.stop_sequences);
        }
        merge_root_options(&mut body, &request.provider_options);

        let resp = self
            .http_client
            .post("https://api.cohere.ai/v2/chat")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
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
            let text = resp.text().await.unwrap_or_default();
            return Err(self.map_http_error(status, &text));
        }

        Ok(resp)
    }
}

// ---------------------------------------------------------------------------
// Helpers (module-private)
// ---------------------------------------------------------------------------

/// Map a Cohere finish_reason string to the provider-agnostic StopReason.
fn map_finish_reason(reason: &str) -> StopReason {
    match reason {
        "COMPLETE" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        "STOP_SEQUENCE" => StopReason::StopSequence,
        "TOOL_CALL" => StopReason::ToolUse,
        "ERROR" | "ERROR_TOXIC" | "USER_CANCEL" => StopReason::Other(reason.to_string()),
        other => StopReason::Other(other.to_string()),
    }
}

/// Parse Cohere v2 usage object into the provider-agnostic UsageInfo.
///
/// Cohere v2 streaming shape:
///   `{"billed_units": {...}, "tokens": {"input_tokens": N, "output_tokens": N}}`
///
/// Cohere v2 non-streaming shape (inside the response root):
///   `{"billed_units": {...}, "tokens": {"input_tokens": N, "output_tokens": N}}`
fn parse_cohere_usage(usage: Option<&Value>) -> UsageInfo {
    let Some(u) = usage else {
        return UsageInfo::default();
    };

    // Try the "tokens" sub-object first (present in both streaming delta and
    // the non-streaming response body).
    let tokens = u.get("tokens").unwrap_or(u);

    let input = tokens
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = tokens
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    UsageInfo {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
    }
}

// ---------------------------------------------------------------------------
// LlmProvider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl LlmProvider for CohereProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        "Cohere"
    }

    async fn create_message(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        self.create_message_non_streaming(&request).await
    }

    async fn create_message_stream(
        &self,
        request: ProviderRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        let resp = self.do_streaming(&request).await?;
        let provider_id = self.id.clone();
        let model_name = request.model.clone();

        let s = stream! {
            use futures::StreamExt;

            let mut byte_stream = resp.bytes_stream();
            let mut leftover = String::new();

            let mut message_started = false;
            let mut tool_call_buffers: std::collections::HashMap<
                usize,
                (String, String, String),
            > = std::collections::HashMap::new();

            // Cohere streams newline-delimited JSON objects (not SSE data: lines).
            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
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

                for line in lines {
                    let line = line.trim_end_matches('\r').trim();
                    if line.is_empty() {
                        continue;
                    }

                    // Cohere may also send SSE-formatted lines.
                    let data = if let Some(rest) = line.strip_prefix("data:") {
                        rest.trim()
                    } else {
                        line
                    };

                    if data == "[DONE]" {
                        yield Ok(StreamEvent::MessageStop);
                        return;
                    }

                    let event: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(e) => {
                            debug!("Failed to parse Cohere stream chunk: {}: {}", e, data);
                            continue;
                        }
                    };

                    let event_type = event
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    match event_type {
                        "message-start" => {
                            if !message_started {
                                let msg_id = event
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown")
                                    .to_string();
                                yield Ok(StreamEvent::MessageStart {
                                    id: msg_id,
                                    model: model_name.clone(),
                                    usage: UsageInfo::default(),
                                });
                                yield Ok(StreamEvent::ContentBlockStart {
                                    index: 0,
                                    content_block: ContentBlock::Text { text: String::new() },
                                });
                                message_started = true;
                            }
                        }

                        "content-start" => {
                            // A new content block is beginning — already handled
                            // by message-start for text.  For tool calls a
                            // separate tool-call-start event carries the metadata.
                        }

                        "content-delta" => {
                            // Text delta:
                            // {"type":"content-delta","index":N,"delta":{"message":{"content":{"type":"text","text":"..."}}}}
                            if !message_started {
                                yield Ok(StreamEvent::MessageStart {
                                    id: "unknown".to_string(),
                                    model: model_name.clone(),
                                    usage: UsageInfo::default(),
                                });
                                yield Ok(StreamEvent::ContentBlockStart {
                                    index: 0,
                                    content_block: ContentBlock::Text { text: String::new() },
                                });
                                message_started = true;
                            }

                            if let Some(text) = event
                                .get("delta")
                                .and_then(|d| d.get("message"))
                                .and_then(|m| m.get("content"))
                                .and_then(|c| c.get("text"))
                                .and_then(|t| t.as_str())
                            {
                                if !text.is_empty() {
                                    yield Ok(StreamEvent::TextDelta {
                                        index: 0,
                                        text: text.to_string(),
                                    });
                                }
                            }
                        }

                        "tool-call-start" => {
                            // {"type":"tool-call-start","index":N,"delta":{"message":{"tool_calls":{"id":"...","function":{"name":"..."}}}}}
                            if !message_started {
                                yield Ok(StreamEvent::MessageStart {
                                    id: "unknown".to_string(),
                                    model: model_name.clone(),
                                    usage: UsageInfo::default(),
                                });
                                yield Ok(StreamEvent::ContentBlockStart {
                                    index: 0,
                                    content_block: ContentBlock::Text { text: String::new() },
                                });
                                message_started = true;
                            }

                            let tc_index = event
                                .get("index")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as usize;
                            let block_index = 1 + tc_index;

                            if let Some(tc) = event
                                .get("delta")
                                .and_then(|d| d.get("message"))
                                .and_then(|m| m.get("tool_calls"))
                            {
                                let tc_id = tc
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let tc_name = tc
                                    .get("function")
                                    .and_then(|f| f.get("name"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                tool_call_buffers.insert(
                                    block_index,
                                    (tc_id.clone(), tc_name.clone(), String::new()),
                                );
                                yield Ok(StreamEvent::ContentBlockStart {
                                    index: block_index,
                                    content_block: ContentBlock::ToolUse {
                                        id: tc_id,
                                        name: tc_name,
                                        input: json!({}),
                                    },
                                });
                            }
                        }

                        "tool-call-delta" => {
                            // {"type":"tool-call-delta","index":N,"delta":{"message":{"tool_calls":{"function":{"arguments":"..."}}}}}
                            let tc_index = event
                                .get("index")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as usize;
                            let block_index = 1 + tc_index;

                            if let Some(args_frag) = event
                                .get("delta")
                                .and_then(|d| d.get("message"))
                                .and_then(|m| m.get("tool_calls"))
                                .and_then(|tc| tc.get("function"))
                                .and_then(|f| f.get("arguments"))
                                .and_then(|v| v.as_str())
                            {
                                if !args_frag.is_empty() {
                                    if let Some((_, _, buf)) =
                                        tool_call_buffers.get_mut(&block_index)
                                    {
                                        buf.push_str(args_frag);
                                    }
                                    yield Ok(StreamEvent::InputJsonDelta {
                                        index: block_index,
                                        partial_json: args_frag.to_string(),
                                    });
                                }
                            }
                        }

                        "content-end" | "tool-call-end" => {
                            // Individual block ended — nothing to emit; handled at
                            // message-end.
                        }

                        "message-end" => {
                            // {"type":"message-end","finish_reason":"COMPLETE","delta":{"finish_reason":"COMPLETE","usage":{...}}}
                            let finish_reason = event
                                .get("delta")
                                .and_then(|d| d.get("finish_reason"))
                                .and_then(|v| v.as_str())
                                .or_else(|| {
                                    event.get("finish_reason").and_then(|v| v.as_str())
                                })
                                .unwrap_or("COMPLETE");

                            let stop_reason = map_finish_reason(finish_reason);

                            // Close all open content blocks.
                            yield Ok(StreamEvent::ContentBlockStop { index: 0 });
                            let mut tc_indices: Vec<usize> =
                                tool_call_buffers.keys().cloned().collect();
                            tc_indices.sort();
                            for idx in tc_indices {
                                yield Ok(StreamEvent::ContentBlockStop { index: idx });
                            }

                            let usage = event
                                .get("delta")
                                .and_then(|d| d.get("usage"))
                                .map(|u| parse_cohere_usage(Some(u)));

                            yield Ok(StreamEvent::MessageDelta {
                                stop_reason: Some(stop_reason),
                                usage,
                            });
                            yield Ok(StreamEvent::MessageStop);
                            return;
                        }

                        other => {
                            debug!("Unhandled Cohere stream event type: {}", other);
                        }
                    }
                }
            }

            if message_started {
                yield Ok(StreamEvent::MessageStop);
            }
        };

        Ok(Box::pin(s))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![
            ModelInfo {
                id: ModelId::new("command-r-plus"),
                provider_id: self.id.clone(),
                name: "Command R+".to_string(),
                context_window: 128_000,
                max_output_tokens: 4_000,
            },
            ModelInfo {
                id: ModelId::new("command-r"),
                provider_id: self.id.clone(),
                name: "Command R".to_string(),
                context_window: 128_000,
                max_output_tokens: 4_000,
            },
        ])
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        if self.api_key.is_empty() {
            return Ok(ProviderStatus::Unavailable {
                reason: "No API key configured".to_string(),
            });
        }

        // Lightweight check: list models endpoint.
        let resp = self
            .http_client
            .get("https://api.cohere.ai/v2/models")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => Ok(ProviderStatus::Healthy),
            Ok(r) => Ok(ProviderStatus::Unavailable {
                reason: format!("models endpoint returned {}", r.status()),
            }),
            Err(e) => Ok(ProviderStatus::Unavailable {
                reason: e.to_string(),
            }),
        }
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
