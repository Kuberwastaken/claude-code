// providers/openai_compat.rs — OpenAI-Compatible generic provider adapter.
//
// A configurable OpenAI Chat Completions adapter that can target any
// provider exposing an OpenAI-compatible API.  Configure base URL, auth,
// extra headers, and per-provider behavioural quirks via the builder API.

use std::pin::Pin;

use async_stream::stream;
use async_trait::async_trait;
use claurst_core::provider_id::{ModelId, ProviderId};
use claurst_core::types::{ContentBlock, UsageInfo};
use futures::Stream;
use serde_json::{json, Value};
use tracing::debug;

use crate::error_handling::parse_error_response;
use crate::provider::{LlmProvider, ModelInfo};
use crate::provider_error::ProviderError;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StreamEvent,
    SystemPromptStyle,
};

// Re-use the message transformation helpers from openai.rs.
use super::openai::OpenAiProvider;
use super::request_options::merge_openai_compatible_options;

// ---------------------------------------------------------------------------
// ProviderQuirks
// ---------------------------------------------------------------------------

/// Provider-specific behavioural quirks that alter how the generic adapter
/// builds and interprets requests/responses.
#[derive(Debug, Clone, Default)]
pub struct ProviderQuirks {
    /// Truncate tool call IDs to at most this many characters before sending.
    /// For example, Mistral requires tool IDs of at most 9 characters.
    pub tool_id_max_len: Option<usize>,

    /// If `true`, strip all non-alphanumeric characters from tool IDs.
    pub tool_id_alphanumeric_only: bool,

    /// Extra error-message substrings (or regex-like patterns) that indicate
    /// the request exceeded the model's context window.
    pub overflow_patterns: Vec<String>,

    /// Whether to send `{"stream_options": {"include_usage": true}}` when
    /// streaming.  Required by some providers to receive token counts.
    pub include_usage_in_stream: bool,

    /// Override the sampling temperature when the request does not specify one.
    pub default_temperature: Option<f64>,

    /// Some providers (e.g. older Mistral releases) reject a message sequence
    /// that goes …tool_result → user… without an intervening assistant turn.
    /// When `true`, an `{"role":"assistant","content":"Done."}` message is
    /// inserted between any `role: tool` message and a following `role: user`
    /// message.
    pub fix_tool_user_sequence: bool,

    /// Name of the JSON field in the assistant message that carries extended
    /// reasoning / thinking text.  `None` means the provider does not expose
    /// reasoning output.  Example: `Some("reasoning_content")` for DeepSeek.
    pub reasoning_field: Option<String>,
}

// ---------------------------------------------------------------------------
// OpenAiCompatProvider
// ---------------------------------------------------------------------------

pub struct OpenAiCompatProvider {
    id: ProviderId,
    name: String,
    base_url: String,
    api_key: Option<String>,
    extra_headers: Vec<(String, String)>,
    quirks: ProviderQuirks,
    http_client: reqwest::Client,
}

impl OpenAiCompatProvider {
    /// Create a new compat provider.  `base_url` should already include any
    /// path prefix (e.g. `"https://api.groq.com/openai/v1"`).
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("failed to build reqwest client");

        Self {
            id: ProviderId::new(id),
            name: name.into(),
            base_url: base_url.into(),
            api_key: None,
            extra_headers: Vec::new(),
            quirks: ProviderQuirks::default(),
            http_client,
        }
    }

    /// Set an API key that will be sent as `Authorization: Bearer <key>`.
    pub fn with_api_key(mut self, key: String) -> Self {
        self.api_key = if key.is_empty() { None } else { Some(key) };
        self
    }

    /// Append a custom header sent on every request.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((name.into(), value.into()));
        self
    }

    /// Apply provider-specific quirks.
    pub fn with_quirks(mut self, quirks: ProviderQuirks) -> Self {
        self.quirks = quirks;
        self
    }

    /// Override the base URL (e.g. from a user-supplied --api-base flag).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Returns `true` when the provider has no usable API key.
    fn has_no_key(&self) -> bool {
        self.api_key.is_none()
    }

    /// Scrub a tool-call ID according to the configured quirks.
    fn scrub_tool_id(&self, id: &str) -> String {
        let mut s = id.to_string();
        if self.quirks.tool_id_alphanumeric_only {
            s = s.chars().filter(|c| c.is_alphanumeric()).collect();
        }
        if let Some(max_len) = self.quirks.tool_id_max_len {
            let truncated: String = s.chars().take(max_len).collect();
            s = format!("{:0<width$}", truncated, width = max_len);
        }
        s
    }

    /// Apply `scrub_tool_id` to every tool-call id/tool_call_id in a messages
    /// array that was already built by `OpenAiProvider::to_openai_messages`.
    fn apply_tool_id_quirks(&self, messages: &mut Vec<Value>) {
        if self.quirks.tool_id_max_len.is_none() && !self.quirks.tool_id_alphanumeric_only {
            return;
        }
        for msg in messages.iter_mut() {
            // assistant message tool_calls[].id
            if let Some(tcs) = msg.get_mut("tool_calls").and_then(|v| v.as_array_mut()) {
                for tc in tcs.iter_mut() {
                    if let Some(id_val) = tc.get("id").and_then(|v| v.as_str()) {
                        let scrubbed = self.scrub_tool_id(id_val);
                        if let Some(obj) = tc.as_object_mut() {
                            obj.insert("id".to_string(), json!(scrubbed));
                        }
                    }
                }
            }
            // tool message tool_call_id
            if let Some(id_val) = msg.get("tool_call_id").and_then(|v| v.as_str()) {
                let scrubbed = self.scrub_tool_id(id_val);
                if let Some(obj) = msg.as_object_mut() {
                    obj.insert("tool_call_id".to_string(), json!(scrubbed));
                }
            }
        }
    }

    /// Insert `{"role":"assistant","content":"Done."}` between any
    /// `role: tool` message that is immediately followed by a `role: user`
    /// message.
    fn apply_fix_tool_user_sequence(messages: &mut Vec<Value>) {
        let mut i = 0;
        while i + 1 < messages.len() {
            let current_is_tool = messages[i].get("role").and_then(|v| v.as_str()) == Some("tool");
            let next_is_user = messages[i + 1].get("role").and_then(|v| v.as_str()) == Some("user");

            if current_is_tool && next_is_user {
                messages.insert(i + 1, json!({ "role": "assistant", "content": "Done." }));
                i += 2; // skip past the inserted message and the user message
            } else {
                i += 1;
            }
        }
    }

    /// Build the full messages array, applying all quirks.
    fn build_messages(&self, request: &ProviderRequest) -> Vec<Value> {
        let mut messages = OpenAiProvider::to_openai_messages_pub(
            &request.messages,
            request.system_prompt.as_ref(),
        );

        self.apply_tool_id_quirks(&mut messages);

        if self.quirks.fix_tool_user_sequence {
            Self::apply_fix_tool_user_sequence(&mut messages);
        }

        messages
    }

    /// Resolve the temperature to use: request value takes priority, then
    /// the quirk default, then nothing (let the API default apply).
    fn resolve_temperature(&self, request: &ProviderRequest) -> Option<f64> {
        request.temperature.or(self.quirks.default_temperature)
    }

    /// Attach the authorization header if an API key is configured.
    fn apply_auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(key) = &self.api_key {
            builder.header("Authorization", format!("Bearer {}", key))
        } else {
            builder
        }
    }

    /// Attach all configured extra headers.
    fn apply_extra_headers(&self, mut builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in &self.extra_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        builder
    }

    fn map_http_error(&self, status: u16, body: &str) -> ProviderError {
        parse_error_response(status, body, &self.id)
    }

    // -----------------------------------------------------------------------
    // Non-streaming
    // -----------------------------------------------------------------------

    async fn create_message_non_streaming(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        let messages = self.build_messages(request);
        let tools = OpenAiProvider::to_openai_tools_pub(&request.tools);

        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
            "stream": false,
        });

        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if let Some(t) = self.resolve_temperature(request) {
            body["temperature"] = json!(t);
        }
        if let Some(p) = request.top_p {
            body["top_p"] = json!(p);
        }
        if !request.stop_sequences.is_empty() {
            body["stop"] = json!(request.stop_sequences);
        }
        merge_openai_compatible_options(&mut body, &request.provider_options);

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let builder = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json");
        let builder = self.apply_auth(builder);
        let builder = self.apply_extra_headers(builder);

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
            return Err(self.map_http_error(status, &text));
        }

        let json: Value = serde_json::from_str(&text).map_err(|e| ProviderError::Other {
            provider: self.id.clone(),
            message: format!("Failed to parse response JSON: {}", e),
            status: Some(status),
            body: Some(text.clone()),
        })?;

        OpenAiProvider::parse_non_streaming_response_pub(&json, &self.id)
    }

    // -----------------------------------------------------------------------
    // Streaming
    // -----------------------------------------------------------------------

    async fn do_streaming(
        &self,
        request: &ProviderRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let messages = self.build_messages(request);
        let tools = OpenAiProvider::to_openai_tools_pub(&request.tools);

        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
            "stream": true,
        });

        if self.quirks.include_usage_in_stream {
            body["stream_options"] = json!({ "include_usage": true });
        }

        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if let Some(t) = self.resolve_temperature(request) {
            body["temperature"] = json!(t);
        }
        if let Some(p) = request.top_p {
            body["top_p"] = json!(p);
        }
        if !request.stop_sequences.is_empty() {
            body["stop"] = json!(request.stop_sequences);
        }
        merge_openai_compatible_options(&mut body, &request.provider_options);

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let builder = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");
        let builder = self.apply_auth(builder);
        let builder = self.apply_extra_headers(builder);

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
            let text = resp.text().await.unwrap_or_default();
            return Err(self.map_http_error(status, &text));
        }

        Ok(resp)
    }
}

// ---------------------------------------------------------------------------
// LlmProvider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn create_message(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        if self.has_no_key() {
            // Providers that have no key set are considered unconfigured.
            // We allow the call to proceed in case the provider genuinely needs
            // no auth (e.g. Ollama), but callers that gate on health_check()
            // will see Unavailable first.
        }
        self.create_message_non_streaming(&request).await
    }

    async fn create_message_stream(
        &self,
        request: ProviderRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        let resp = self.do_streaming(&request).await?;
        let provider_id = self.id.clone();
        let reasoning_field = self.quirks.reasoning_field.clone();

        let s = stream! {
            use futures::StreamExt;

            let mut byte_stream = resp.bytes_stream();
            let mut leftover = String::new();

            let mut message_started = false;
            let mut message_id = String::from("unknown");
            let mut model_name = String::new();
            let mut tool_call_buffers: std::collections::HashMap<
                usize,
                (String, String, String),
            > = std::collections::HashMap::new();

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

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    let data = if let Some(rest) = line.strip_prefix("data:") {
                        rest.trim()
                    } else {
                        continue;
                    };

                    if data == "[DONE]" {
                        yield Ok(StreamEvent::MessageStop);
                        return;
                    }

                    let chunk_json: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(e) => {
                            debug!("Failed to parse SSE chunk: {}: {}", e, data);
                            continue;
                        }
                    };

                    if !message_started {
                        if let Some(id) = chunk_json.get("id").and_then(|v| v.as_str()) {
                            message_id = id.to_string();
                        }
                        if let Some(m) = chunk_json.get("model").and_then(|v| v.as_str()) {
                            model_name = m.to_string();
                        }
                        yield Ok(StreamEvent::MessageStart {
                            id: message_id.clone(),
                            model: model_name.clone(),
                            usage: UsageInfo::default(),
                        });
                        yield Ok(StreamEvent::ContentBlockStart {
                            index: 0,
                            content_block: ContentBlock::Text { text: String::new() },
                        });
                        message_started = true;
                    }

                    let choices = match chunk_json.get("choices").and_then(|c| c.as_array()) {
                        Some(c) => c,
                        None => {
                            if let Some(usage_val) = chunk_json.get("usage") {
                                let usage = OpenAiProvider::parse_usage_pub(Some(usage_val));
                                yield Ok(StreamEvent::MessageDelta {
                                    stop_reason: None,
                                    usage: Some(usage),
                                });
                            }
                            continue;
                        }
                    };

                    let choice = match choices.first() {
                        Some(c) => c,
                        None => continue,
                    };

                    let delta = match choice.get("delta") {
                        Some(d) => d,
                        None => continue,
                    };

                    // Reasoning / thinking extraction.
                    // Check the provider-specific field first (e.g. DeepSeek's
                    // "reasoning_content"), then fall back to common field names
                    // used by other providers (Copilot "reasoning_text", generic
                    // "reasoning", etc.).  This allows reasoning traces to show
                    // for any provider that emits them without needing explicit
                    // per-provider configuration.
                    {
                        const COMMON_REASONING_FIELDS: &[&str] = &[
                            "reasoning_content",  // DeepSeek
                            "reasoning_text",     // GitHub Copilot
                            "reasoning",          // Generic / future
                        ];
                        let fields_to_check: Vec<&str> = if let Some(ref f) = reasoning_field {
                            // Provider-specific field first, then common ones
                            let mut v = vec![f.as_str()];
                            for common in COMMON_REASONING_FIELDS {
                                if *common != f.as_str() {
                                    v.push(common);
                                }
                            }
                            v
                        } else {
                            COMMON_REASONING_FIELDS.to_vec()
                        };
                        for field in &fields_to_check {
                            if let Some(reasoning) = delta.get(*field).and_then(|v| v.as_str()) {
                                if !reasoning.is_empty() {
                                    yield Ok(StreamEvent::ReasoningDelta {
                                        index: 0,
                                        reasoning: reasoning.to_string(),
                                    });
                                    break;
                                }
                            }
                        }
                    }

                    // Text content delta
                    if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                        if !content.is_empty() {
                            yield Ok(StreamEvent::TextDelta {
                                index: 0,
                                text: content.to_string(),
                            });
                        }
                    }

                    // Tool call deltas
                    if let Some(tool_calls) =
                        delta.get("tool_calls").and_then(|t| t.as_array())
                    {
                        for tc in tool_calls {
                            let tc_index = tc
                                .get("index")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as usize;
                            if let Some(tc_id) =
                                tc.get("id").and_then(|v| v.as_str())
                            {
                                let name = tc
                                    .get("function")
                                    .and_then(|f| f.get("name"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let block_index = 1 + tc_index;
                                tool_call_buffers.insert(
                                    block_index,
                                    (tc_id.to_string(), name.clone(), String::new()),
                                );
                                yield Ok(StreamEvent::ContentBlockStart {
                                    index: block_index,
                                    content_block: ContentBlock::ToolUse {
                                        id: tc_id.to_string(),
                                        name,
                                        input: json!({}),
                                    },
                                });
                            }
                            if let Some(args_frag) = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|v| v.as_str())
                            {
                                if !args_frag.is_empty() {
                                    let block_index = 1 + tc_index;
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
                    }

                    // finish_reason
                    if let Some(finish_reason) =
                        choice.get("finish_reason").and_then(|v| v.as_str())
                    {
                        if !finish_reason.is_empty() && finish_reason != "null" {
                            yield Ok(StreamEvent::ContentBlockStop { index: 0 });
                            let mut tc_indices: Vec<usize> =
                                tool_call_buffers.keys().cloned().collect();
                            tc_indices.sort();
                            for idx in tc_indices {
                                yield Ok(StreamEvent::ContentBlockStop { index: idx });
                            }

                            let stop_reason =
                                OpenAiProvider::map_finish_reason_pub(finish_reason);

                            let usage_val = chunk_json.get("usage");
                            let usage = usage_val.map(|u| OpenAiProvider::parse_usage_pub(Some(u)));

                            yield Ok(StreamEvent::MessageDelta {
                                stop_reason: Some(stop_reason),
                                usage,
                            });
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
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let builder = self.http_client.get(&url);
        let builder = self.apply_auth(builder);
        let builder = self.apply_extra_headers(builder);

        let resp = builder.send().await.map_err(|e| ProviderError::Other {
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
            message: format!("Failed to parse models JSON: {}", e),
            status: Some(status),
            body: Some(text),
        })?;

        let data = match json.get("data").and_then(|d| d.as_array()) {
            Some(d) => d,
            None => return Ok(vec![]),
        };

        let provider_id = self.id.clone();
        let models: Vec<ModelInfo> = data
            .iter()
            .filter_map(|m| {
                let id = m.get("id").and_then(|v| v.as_str())?;
                Some(ModelInfo {
                    id: ModelId::new(id),
                    provider_id: provider_id.clone(),
                    name: id.to_string(),
                    context_window: 128_000,
                    max_output_tokens: 16_384,
                })
            })
            .collect();

        Ok(models)
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        // Providers that need an API key but have none configured are
        // immediately unavailable without making a network call.
        if self.has_no_key() {
            // Local providers (Ollama, LM Studio, llama.cpp) have no key by
            // design.  For remote providers the key will be None only if the
            // env var was missing or empty; report that clearly.
            //
            // We distinguish by whether the base_url is a localhost address.
            let is_local = self.base_url.contains("localhost")
                || self.base_url.contains("127.0.0.1")
                || self.base_url.contains("::1");

            if !is_local {
                return Ok(ProviderStatus::Unavailable {
                    reason: "No API key configured".to_string(),
                });
            }
        }

        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let builder = self.http_client.get(&url);
        let builder = self.apply_auth(builder);
        let builder = self.apply_extra_headers(builder);

        match builder.send().await {
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
            thinking: self.quirks.reasoning_field.is_some(),
            image_input: true,
            pdf_input: false,
            audio_input: false,
            video_input: false,
            caching: false,
            structured_output: true,
            system_prompt_style: SystemPromptStyle::SystemMessage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mistral_tool_ids_match_opencode_style() {
        let provider = OpenAiCompatProvider::new("mistral", "Mistral", "https://example.com")
            .with_quirks(ProviderQuirks {
                tool_id_max_len: Some(9),
                tool_id_alphanumeric_only: true,
                ..Default::default()
            });

        assert_eq!(provider.scrub_tool_id("call-123456789abc"), "call12345");
        assert_eq!(provider.scrub_tool_id("x"), "x00000000");
    }

    #[test]
    fn fix_tool_user_sequence_inserts_done_between_tool_and_user() {
        let mut messages = vec![
            json!({"role": "tool", "tool_call_id": "call_1", "content": "ok"}),
            json!({"role": "user", "content": "continue"}),
        ];

        OpenAiCompatProvider::apply_fix_tool_user_sequence(&mut messages);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["role"], json!("assistant"));
        assert_eq!(messages[1]["content"], json!("Done."));
    }
}
