// providers/openai.rs — OpenAI Chat Completions provider adapter.
//
// Implements LlmProvider for the OpenAI Chat Completions API (POST
// /v1/chat/completions).  Works equally well for any OpenAI-compatible
// endpoint (e.g. Azure OpenAI, local Ollama, Together AI) by configuring
// `base_url`.
//
// Phase 2A implementation covers:
//  - Request transformation (Anthropic internal types → OpenAI wire format)
//  - Streaming via Server-Sent Events (data: {...}\n\n lines)
//  - Non-streaming JSON response parsing
//  - Tool-call support (request and response)
//  - Model listing via GET /v1/models
//  - Health check
//  - ProviderCapabilities

use async_stream::stream;
use async_trait::async_trait;
use claurst_core::provider_id::{ModelId, ProviderId};
use claurst_core::types::{
    ContentBlock, ImageSource, MessageContent, Role, ToolResultContent, UsageInfo,
};
use futures::Stream;
use serde_json::{json, Value};
use std::pin::Pin;
use tracing::debug;

use crate::error_handling::parse_error_response;
use crate::provider::{LlmProvider, ModelInfo};
use crate::provider_error::ProviderError;
use crate::provider_types::SystemPrompt;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StopReason,
    StreamEvent, SystemPromptStyle,
};

use super::request_options::merge_openai_compatible_options;

// ---------------------------------------------------------------------------
// OpenAiProvider
// ---------------------------------------------------------------------------

pub struct OpenAiProvider {
    id: ProviderId,
    name: String,
    base_url: String,
    api_key: String,
    http_client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(api_key: String) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("failed to build reqwest client");

        Self {
            id: ProviderId::new(ProviderId::OPENAI),
            name: "OpenAI".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key,
            http_client,
        }
    }

    /// Override the API base URL (e.g. for Azure, Ollama, or other compatible
    /// endpoints).
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Returns `true` if the model should use the Responses API instead of
    /// Chat Completions (gpt-5+, o3, o4-mini).
    fn use_responses_api(model: &str) -> bool {
        model.starts_with("o3") || model.starts_with("o4") || model.starts_with("gpt-5")
    }

    // -----------------------------------------------------------------------
    // Request transformation helpers
    // -----------------------------------------------------------------------

    /// Public wrapper for Azure/Copilot providers that share the OpenAI wire format.
    pub fn to_openai_messages_pub(
        messages: &[claurst_core::types::Message],
        system_prompt: Option<&SystemPrompt>,
    ) -> Vec<Value> {
        Self::to_openai_messages(messages, system_prompt)
    }

    /// Public wrapper for tool conversion used by Azure/Copilot providers.
    pub fn to_openai_tools_pub(tools: &[claurst_core::types::ToolDefinition]) -> Vec<Value> {
        Self::to_openai_tools(tools)
    }

    /// Public wrapper for finish-reason mapping.
    pub fn map_finish_reason_pub(reason: &str) -> StopReason {
        Self::map_finish_reason(reason)
    }

    /// Public wrapper for usage parsing.
    pub fn parse_usage_pub(usage: Option<&Value>) -> UsageInfo {
        Self::parse_usage(usage)
    }

    /// Public wrapper for non-streaming response parsing.
    pub fn parse_non_streaming_response_pub(
        json: &Value,
        provider_id: &claurst_core::provider_id::ProviderId,
    ) -> Result<crate::provider_types::ProviderResponse, crate::provider_error::ProviderError> {
        Self::parse_non_streaming_response(json, provider_id)
    }

    /// Convert a provider-agnostic [`ProviderRequest`] into the OpenAI Chat
    /// Completions `messages` array.
    fn to_openai_messages(
        messages: &[claurst_core::types::Message],
        system_prompt: Option<&SystemPrompt>,
    ) -> Vec<Value> {
        let mut result: Vec<Value> = Vec::new();

        // System prompt goes first as a `system` role message.
        if let Some(sys) = system_prompt {
            let sys_text = match sys {
                SystemPrompt::Text(t) => t.clone(),
                SystemPrompt::Blocks(blocks) => blocks
                    .iter()
                    .map(|b| b.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            result.push(json!({ "role": "system", "content": sys_text }));
        }

        for msg in messages {
            match msg.role {
                Role::User => {
                    Self::append_user_messages(&mut result, &msg.content);
                }
                Role::Assistant => {
                    let (text_content, tool_calls) =
                        Self::assistant_content_to_openai(&msg.content);
                    let mut obj = serde_json::Map::new();
                    obj.insert("role".into(), json!("assistant"));
                    if let Some(tc) = text_content {
                        obj.insert("content".into(), json!(tc));
                    } else {
                        obj.insert("content".into(), Value::Null);
                    }
                    if !tool_calls.is_empty() {
                        obj.insert("tool_calls".into(), json!(tool_calls));
                    }
                    result.push(Value::Object(obj));

                    // ToolResult blocks in an assistant message need to be
                    // emitted as separate `role: tool` messages.
                    let tool_results = Self::extract_tool_results(&msg.content);
                    result.extend(tool_results);
                }
            }
        }

        result
    }

    fn append_user_messages(result: &mut Vec<Value>, content: &MessageContent) {
        match content {
            MessageContent::Text(text) => {
                result.push(json!({ "role": "user", "content": text }));
            }
            MessageContent::Blocks(blocks) => {
                let mut user_parts: Vec<Value> = Vec::new();
                let flush_user_parts = |result: &mut Vec<Value>, parts: &mut Vec<Value>| {
                    if !parts.is_empty() {
                        result.push(json!({
                            "role": "user",
                            "content": std::mem::take(parts),
                        }));
                    }
                };

                for block in blocks {
                    if let Some(tool_result) = Self::tool_result_to_openai_message(block) {
                        flush_user_parts(result, &mut user_parts);
                        result.push(tool_result);
                    } else if let Some(part) = Self::user_block_to_openai_part(block) {
                        user_parts.push(part);
                    }
                }

                flush_user_parts(result, &mut user_parts);
            }
        }
    }

    fn user_block_to_openai_part(block: &ContentBlock) -> Option<Value> {
        match block {
            ContentBlock::Text { text } => Some(json!({ "type": "text", "text": text })),
            ContentBlock::Image { source } => {
                let url = Self::image_source_to_url(source);
                Some(json!({
                    "type": "image_url",
                    "image_url": { "url": url }
                }))
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                // Tool results become separate `role: tool` messages at the
                // conversation level — handled in append_user_messages.
                let _ = (tool_use_id, content, is_error);
                None
            }
            // Thinking, RedactedThinking, etc. are not supported by OpenAI.
            _ => None,
        }
    }

    fn image_source_to_url(source: &ImageSource) -> String {
        if let Some(url) = &source.url {
            return url.clone();
        }
        // base64-encoded image
        let media_type = source.media_type.as_deref().unwrap_or("image/png");
        let data = source.data.as_deref().unwrap_or("");
        format!("data:{};base64,{}", media_type, data)
    }

    /// Split assistant content blocks into (text_string, tool_calls_array).
    fn assistant_content_to_openai(content: &MessageContent) -> (Option<String>, Vec<Value>) {
        let blocks = match content {
            MessageContent::Text(t) => return (Some(t.clone()), vec![]),
            MessageContent::Blocks(b) => b,
        };

        let mut text_parts: Vec<&str> = Vec::new();
        let mut tool_calls: Vec<Value> = Vec::new();

        for block in blocks {
            match block {
                ContentBlock::Text { text } => {
                    text_parts.push(text.as_str());
                }
                ContentBlock::ToolUse { id, name, input } => {
                    let args = serde_json::to_string(input).unwrap_or_default();
                    tool_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": args
                        }
                    }));
                }
                // Thinking is dropped — not supported by OpenAI.
                _ => {}
            }
        }

        let text_content = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        };

        (text_content, tool_calls)
    }

    /// Collect any ToolResult blocks and emit them as `role: tool` messages.
    fn extract_tool_results(content: &MessageContent) -> Vec<Value> {
        let blocks = match content {
            MessageContent::Text(_) => return vec![],
            MessageContent::Blocks(b) => b,
        };

        blocks
            .iter()
            .filter_map(Self::tool_result_to_openai_message)
            .collect()
    }

    fn tool_result_to_openai_message(block: &ContentBlock) -> Option<Value> {
        let ContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } = block
        else {
            return None;
        };

        let text = match content {
            ToolResultContent::Text(t) => t.clone(),
            ToolResultContent::Blocks(inner) => inner
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        };

        Some(json!({
            "role": "tool",
            "tool_call_id": tool_use_id,
            "content": text,
        }))
    }

    /// Convert tool definitions to the OpenAI `tools` array format.
    fn to_openai_tools(tools: &[claurst_core::types::ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|td| {
                json!({
                    "type": "function",
                    "function": {
                        "name": td.name,
                        "description": td.description,
                        "parameters": td.input_schema
                    }
                })
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // HTTP helpers
    // -----------------------------------------------------------------------

    fn auth_header(&self) -> (&'static str, String) {
        ("Authorization", format!("Bearer {}", self.api_key))
    }

    fn map_http_error(&self, status: u16, body: &str) -> ProviderError {
        parse_error_response(status, body, &self.id)
    }

    // -----------------------------------------------------------------------
    // Non-streaming create_message
    // -----------------------------------------------------------------------

    async fn create_message_non_streaming(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        let messages = Self::to_openai_messages(&request.messages, request.system_prompt.as_ref());
        let tools = Self::to_openai_tools(&request.tools);

        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
            "stream": false,
            "store": false,
        });

        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(p) = request.top_p {
            body["top_p"] = json!(p);
        }
        if !request.stop_sequences.is_empty() {
            body["stop"] = json!(request.stop_sequences);
        }
        merge_openai_compatible_options(&mut body, &request.provider_options);

        let (auth_key, auth_val) = self.auth_header();
        let url = format!("{}/v1/chat/completions", self.base_url);

        let resp = self
            .http_client
            .post(&url)
            .header(auth_key, auth_val)
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

        Self::parse_non_streaming_response(&json, &self.id)
    }

    fn parse_non_streaming_response(
        json: &Value,
        provider_id: &ProviderId,
    ) -> Result<ProviderResponse, ProviderError> {
        let id = json
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let model = json
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let choice = json
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| ProviderError::Other {
                provider: provider_id.clone(),
                message: "No choices in response".to_string(),
                status: None,
                body: None,
            })?;

        let message = choice.get("message").ok_or_else(|| ProviderError::Other {
            provider: provider_id.clone(),
            message: "No message in choice".to_string(),
            status: None,
            body: None,
        })?;

        let mut content_blocks: Vec<ContentBlock> = Vec::new();

        // Text content
        if let Some(text) = message.get("content").and_then(|c| c.as_str()) {
            if !text.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: text.to_string(),
                });
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
                let args_str = tc
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                let input: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
                content_blocks.push(ContentBlock::ToolUse { id, name, input });
            }
        }

        let finish_reason = choice
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("stop");
        let stop_reason = Self::map_finish_reason(finish_reason);

        let usage = Self::parse_usage(json.get("usage"));

        Ok(ProviderResponse {
            id,
            content: content_blocks,
            stop_reason,
            usage,
            model,
        })
    }

    // -----------------------------------------------------------------------
    // Streaming helpers
    // -----------------------------------------------------------------------

    fn map_finish_reason(reason: &str) -> StopReason {
        match reason {
            "stop" => StopReason::EndTurn,
            "length" => StopReason::MaxTokens,
            "tool_calls" | "function_call" => StopReason::ToolUse,
            "content_filter" => StopReason::ContentFiltered,
            other => StopReason::Other(other.to_string()),
        }
    }

    fn parse_usage(usage: Option<&Value>) -> UsageInfo {
        let u = match usage {
            Some(v) => v,
            None => return UsageInfo::default(),
        };
        UsageInfo {
            input_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            output_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Streaming create_message_stream
    // -----------------------------------------------------------------------

    async fn do_streaming(
        &self,
        request: &ProviderRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let messages = Self::to_openai_messages(&request.messages, request.system_prompt.as_ref());
        let tools = Self::to_openai_tools(&request.tools);

        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
            "store": false,
        });

        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(p) = request.top_p {
            body["top_p"] = json!(p);
        }
        if !request.stop_sequences.is_empty() {
            body["stop"] = json!(request.stop_sequences);
        }
        merge_openai_compatible_options(&mut body, &request.provider_options);

        let (auth_key, auth_val) = self.auth_header();
        let url = format!("{}/v1/chat/completions", self.base_url);

        let resp = self
            .http_client
            .post(&url)
            .header(auth_key, auth_val)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
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

#[cfg(test)]
mod tests {
    use super::*;
    use claurst_core::types::Message;

    #[test]
    fn user_tool_results_become_tool_messages() {
        let messages = vec![
            Message::assistant_blocks(vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "search".to_string(),
                input: json!({ "q": "test" }),
            }]),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: ToolResultContent::Text("done".to_string()),
                is_error: Some(false),
            }]),
        ];

        let wire = OpenAiProvider::to_openai_messages(&messages, None);
        assert_eq!(wire.len(), 2);
        assert_eq!(
            wire[0].get("role").and_then(|v| v.as_str()),
            Some("assistant")
        );
        assert_eq!(wire[1].get("role").and_then(|v| v.as_str()), Some("tool"));
        assert_eq!(
            wire[1].get("tool_call_id").and_then(|v| v.as_str()),
            Some("call_1")
        );
        assert_eq!(
            wire[1].get("content").and_then(|v| v.as_str()),
            Some("done")
        );
    }

    #[test]
    fn mixed_user_content_flushes_before_tool_result() {
        let messages = vec![Message::user_blocks(vec![
            ContentBlock::Text {
                text: "preface".to_string(),
            },
            ContentBlock::ToolResult {
                tool_use_id: "call_2".to_string(),
                content: ToolResultContent::Text("ok".to_string()),
                is_error: Some(false),
            },
        ])];

        let wire = OpenAiProvider::to_openai_messages(&messages, None);
        assert_eq!(wire.len(), 2);
        assert_eq!(wire[0].get("role").and_then(|v| v.as_str()), Some("user"));
        assert_eq!(wire[1].get("role").and_then(|v| v.as_str()), Some("tool"));
        assert_eq!(
            wire[1].get("tool_call_id").and_then(|v| v.as_str()),
            Some("call_2")
        );
    }
}

// ---------------------------------------------------------------------------
// LlmProvider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl LlmProvider for OpenAiProvider {
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
        if Self::use_responses_api(&request.model) {
            return Err(ProviderError::InvalidRequest {
                provider: self.id.clone(),
                message: format!(
                    "Model '{}' requires the OpenAI Responses API which is not yet fully \
                     implemented. Use gpt-4o or gpt-4o-mini for now, or set \
                     OPENAI_BASE_URL to a compatible endpoint.",
                    request.model
                ),
            });
        }
        self.create_message_non_streaming(&request).await
    }

    async fn create_message_stream(
        &self,
        request: ProviderRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        if Self::use_responses_api(&request.model) {
            return Err(ProviderError::InvalidRequest {
                provider: self.id.clone(),
                message: format!(
                    "Model '{}' requires the OpenAI Responses API which is not yet fully \
                     implemented. Use gpt-4o or gpt-4o-mini for now, or set \
                     OPENAI_BASE_URL to a compatible endpoint.",
                    request.model
                ),
            });
        }
        let resp = self.do_streaming(&request).await?;
        let provider_id = self.id.clone();

        // We need the message ID to emit MessageStart.  We'll generate one on
        // the first chunk that carries it.
        let s = stream! {
            use futures::StreamExt;

            let mut byte_stream = resp.bytes_stream();
            let mut leftover = String::new();

            // State carried across chunks
            let mut message_started = false;
            let mut message_id = String::from("unknown");
            let mut model_name = String::new();
            // Track accumulating tool call argument buffers: index -> (id, name, buf)
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

                    // Skip SSE comment lines and blank lines that are not data.
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
                            debug!("Failed to parse OpenAI SSE chunk: {}: {}", e, data);
                            continue;
                        }
                    };

                    // Extract message id and model on first chunk.
                    if !message_started {
                        if let Some(id) = chunk_json.get("id").and_then(|v| v.as_str()) {
                            message_id = id.to_string();
                        }
                        if let Some(m) = chunk_json.get("model").and_then(|v| v.as_str()) {
                            model_name = m.to_string();
                        }
                        // Emit MessageStart — usage will be filled in later from
                        // the final chunk; emit zeros for now.
                        yield Ok(StreamEvent::MessageStart {
                            id: message_id.clone(),
                            model: model_name.clone(),
                            usage: UsageInfo::default(),
                        });
                        // Emit ContentBlockStart for the text block (index 0).
                        yield Ok(StreamEvent::ContentBlockStart {
                            index: 0,
                            content_block: ContentBlock::Text { text: String::new() },
                        });
                        message_started = true;
                    }

                    let choices = match chunk_json.get("choices").and_then(|c| c.as_array()) {
                        Some(c) => c,
                        None => {
                            // May be a usage-only chunk (the final one).
                            if let Some(usage_val) = chunk_json.get("usage") {
                                let usage = OpenAiProvider::parse_usage(Some(usage_val));
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
                            // OpenAI sends id/name only on the first chunk for each tool call.
                            if let Some(tc_id) =
                                tc.get("id").and_then(|v| v.as_str())
                            {
                                let name = tc
                                    .get("function")
                                    .and_then(|f| f.get("name"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                // OpenAI tool calls sit after the text block.
                                // Use index 1 + tc_index.
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
                            // Argument fragment
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

                    // finish_reason signals end of message.
                    if let Some(finish_reason) =
                        choice.get("finish_reason").and_then(|v| v.as_str())
                    {
                        if !finish_reason.is_empty() && finish_reason != "null" {
                            // Close the text content block.
                            yield Ok(StreamEvent::ContentBlockStop { index: 0 });
                            // Close any open tool call blocks.
                            let mut tc_indices: Vec<usize> =
                                tool_call_buffers.keys().cloned().collect();
                            tc_indices.sort();
                            for idx in tc_indices {
                                yield Ok(StreamEvent::ContentBlockStop { index: idx });
                            }

                            let stop_reason =
                                OpenAiProvider::map_finish_reason(finish_reason);

                            // Usage might come in the same chunk or a later one.
                            let usage_val = chunk_json.get("usage");
                            let usage = usage_val.map(|u| OpenAiProvider::parse_usage(Some(u)));

                            yield Ok(StreamEvent::MessageDelta {
                                stop_reason: Some(stop_reason),
                                usage,
                            });
                        }
                    }
                }
            }

            // If we consumed all bytes without seeing [DONE], emit stop.
            if message_started {
                yield Ok(StreamEvent::MessageStop);
            }
        };

        Ok(Box::pin(s))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let (auth_key, auth_val) = self.auth_header();
        let url = format!("{}/v1/models", self.base_url);

        let resp = self
            .http_client
            .get(&url)
            .header(auth_key, auth_val)
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
                // Only return GPT, O3, O4 family models.
                if !id.starts_with("gpt-")
                    && !id.starts_with("o3")
                    && !id.starts_with("o4")
                    && !id.starts_with("o1")
                {
                    return None;
                }
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
        let (auth_key, auth_val) = self.auth_header();
        let url = format!("{}/v1/models", self.base_url);

        let resp = self
            .http_client
            .get(&url)
            .header(auth_key, auth_val)
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
