// providers/google.rs — GoogleProvider: implements LlmProvider for the
// Google Gemini API (generativelanguage.googleapis.com).
//
// Supports:
// - Non-streaming: POST .../generateContent?key={api_key}
// - Streaming SSE: POST .../streamGenerateContent?alt=sse&key={api_key}
// - Tool/function calling via functionDeclarations
// - System prompts via systemInstruction field
// - Thinking config for Gemini 2.5+ and 3.0+ models
// - Image/video inputs via inlineData parts
// - list_models via GET /v1beta/models

use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use claurst_core::provider_id::{ModelId, ProviderId};
use claurst_core::types::{
    ContentBlock, Message, MessageContent, Role, ToolResultContent, UsageInfo,
};
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use tracing::{debug, warn};

use crate::error_handling::parse_error_response as parse_http_error;
use crate::provider::{LlmProvider, ModelInfo};
use crate::provider_error::ProviderError;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StopReason,
    StreamEvent, SystemPrompt, SystemPromptStyle,
};

use super::request_options::merge_google_options;

// ---------------------------------------------------------------------------
// GoogleProvider
// ---------------------------------------------------------------------------

pub struct GoogleProvider {
    id: ProviderId,
    api_key: String,
    base_url: String,
    http_client: reqwest::Client,
}

impl GoogleProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            id: ProviderId::new(ProviderId::GOOGLE),
            api_key,
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            http_client: reqwest::Client::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Returns true if the model supports thinking config (Gemini 2.5+ / 3.0+).
    fn supports_thinking(model: &str) -> bool {
        model.contains("2.5")
            || model.contains("3.0")
            || model.contains("3.1")
            || model.contains("gemini-3")
    }

    /// Build the full generateContent URL for non-streaming.
    fn generate_url(&self, model: &str) -> String {
        format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.base_url, model, self.api_key
        )
    }

    /// Build the full streamGenerateContent URL for streaming.
    fn stream_url(&self, model: &str) -> String {
        format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url, model, self.api_key
        )
    }

    fn tool_use_id_for_name(name: &str, occurrence: usize) -> String {
        let sanitized: String = name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        let base = if sanitized.is_empty() {
            "tool"
        } else {
            sanitized.as_str()
        };
        if occurrence == 0 {
            format!("call_{}", base)
        } else {
            format!("call_{}_{}", base, occurrence + 1)
        }
    }

    fn tool_name_by_id(messages: &[Message]) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        for message in messages {
            let MessageContent::Blocks(blocks) = &message.content else {
                continue;
            };
            for block in blocks {
                if let ContentBlock::ToolUse { id, name, .. } = block {
                    map.insert(id.clone(), name.clone());
                }
            }
        }
        map
    }

    fn infer_tool_name_from_id(tool_use_id: &str) -> Option<String> {
        let raw = tool_use_id.strip_prefix("call_")?;
        let trimmed = if let Some((candidate, suffix)) = raw.rsplit_once('_') {
            if !candidate.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
                candidate
            } else {
                raw
            }
        } else {
            raw
        };

        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    /// Convert a single ContentBlock to a Gemini "part" Value.
    /// Returns None for blocks that should be dropped (e.g. Thinking).
    fn content_block_to_part(block: &ContentBlock) -> Option<Value> {
        match block {
            ContentBlock::Text { text } => Some(json!({ "text": text })),

            ContentBlock::Image { source } => {
                // Prefer base64 inline data; fall back to URL if available.
                if let (Some(data), Some(mime)) = (&source.data, &source.media_type) {
                    Some(json!({
                        "inlineData": {
                            "data": data,
                            "mimeType": mime
                        }
                    }))
                } else if let Some(url) = &source.url {
                    Some(json!({
                        "fileData": {
                            "fileUri": url,
                            "mimeType": source.media_type.as_deref().unwrap_or("image/jpeg")
                        }
                    }))
                } else {
                    None
                }
            }

            ContentBlock::ToolUse { name, input, .. } => Some(json!({
                "functionCall": {
                    "name": name,
                    "args": input
                }
            })),

            // Thinking blocks are not supported by Gemini — drop silently.
            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => None,

            // Document blocks: treat as file data when URL is available,
            // otherwise as inline base64.
            ContentBlock::Document { source, .. } => {
                if let (Some(data), Some(mime)) = (&source.data, &source.media_type) {
                    Some(json!({
                        "inlineData": {
                            "data": data,
                            "mimeType": mime
                        }
                    }))
                } else if let Some(url) = &source.url {
                    Some(json!({
                        "fileData": {
                            "fileUri": url,
                            "mimeType": source.media_type.as_deref().unwrap_or("application/pdf")
                        }
                    }))
                } else {
                    None
                }
            }

            // Render UI-only / metadata blocks as text so context is not lost.
            ContentBlock::UserLocalCommandOutput { command, output } => Some(json!({
                "text": format!("$ {}\n{}", command, output)
            })),
            ContentBlock::UserCommand { name, args } => Some(json!({
                "text": format!("/{} {}", name, args)
            })),
            ContentBlock::UserMemoryInput { key, value } => Some(json!({
                "text": format!("[memory] {}: {}", key, value)
            })),
            ContentBlock::SystemAPIError { message, .. } => Some(json!({
                "text": format!("[error] {}", message)
            })),
            ContentBlock::CollapsedReadSearch {
                tool_name, paths, ..
            } => Some(json!({
                "text": format!("[{}] {}", tool_name, paths.join(", "))
            })),
            ContentBlock::TaskAssignment {
                id,
                subject,
                description,
            } => Some(json!({
                "text": format!("[task:{}] {}: {}", id, subject, description)
            })),

            // ToolResult is handled specially in message conversion.
            ContentBlock::ToolResult { .. } => None,
        }
    }

    /// Convert a ToolResult block to a "functionResponse" part Value.
    fn tool_result_to_part(tool_name: &str, content: &ToolResultContent) -> Value {
        let response_content = match content {
            ToolResultContent::Text(t) => json!({ "content": t }),
            ToolResultContent::Blocks(blocks) => {
                // Concatenate all text blocks for the response payload.
                let text: String = blocks
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Text { text } = b {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                json!({ "content": text })
            }
        };
        json!({
            "functionResponse": {
                "name": tool_name,
                "response": response_content
            }
        })
    }

    /// Sanitize a JSON Schema object for Google's stricter requirements:
    /// - Integer enums → string enums
    /// - `required` must only list fields actually in `properties`
    /// - Non-object types must not have `properties`/`required`
    /// - Array `items` must have a `type` field
    fn sanitize_schema(schema: Value) -> Value {
        match schema {
            Value::Object(mut map) => {
                // Recurse into nested schemas first.
                let schema_type = map
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                // Convert integer enums to string enums.
                if let Some(Value::Array(enum_vals)) = map.get("enum") {
                    if enum_vals.iter().any(|v| v.is_number()) {
                        let string_enums: Vec<Value> = enum_vals
                            .iter()
                            .map(|v| Value::String(v.to_string()))
                            .collect();
                        map.insert("enum".to_string(), Value::Array(string_enums));
                        // Upgrade type to string when converting number enums.
                        map.insert("type".to_string(), Value::String("string".to_string()));
                    }
                }

                // For object types: sanitize properties recursively and fix required.
                if schema_type.as_deref() == Some("object") {
                    if let Some(Value::Object(props)) = map.get_mut("properties") {
                        let sanitized_props: serde_json::Map<String, Value> = props
                            .iter()
                            .map(|(k, v)| (k.clone(), Self::sanitize_schema(v.clone())))
                            .collect();
                        *props = sanitized_props;
                    }

                    // Filter required to only include keys present in properties.
                    if let Some(required) = map.get("required").cloned() {
                        if let Value::Array(req_arr) = required {
                            let prop_keys: std::collections::HashSet<String> = map
                                .get("properties")
                                .and_then(|p| p.as_object())
                                .map(|o| o.keys().cloned().collect())
                                .unwrap_or_default();

                            let filtered: Vec<Value> = req_arr
                                .into_iter()
                                .filter(|v| {
                                    v.as_str().map(|s| prop_keys.contains(s)).unwrap_or(false)
                                })
                                .collect();
                            map.insert("required".to_string(), Value::Array(filtered));
                        }
                    }
                } else {
                    // Non-object types must not carry properties/required.
                    map.remove("properties");
                    map.remove("required");
                }

                // Array items: ensure a type field is present.
                if schema_type.as_deref() == Some("array") {
                    if let Some(items) = map.get_mut("items") {
                        if let Value::Object(ref mut items_map) = items {
                            if !items_map.contains_key("type") {
                                items_map.insert(
                                    "type".to_string(),
                                    Value::String("string".to_string()),
                                );
                            }
                            // Recurse sanitize into items.
                            let sanitized = Self::sanitize_schema(Value::Object(items_map.clone()));
                            *items = sanitized;
                        }
                    }
                }

                Value::Object(map)
            }
            other => other,
        }
    }

    /// Build the full request body JSON for the Gemini API.
    fn build_request_body(&self, request: &ProviderRequest) -> Value {
        // ---- Convert messages ----
        // Google requires a flat list of content objects.
        // ToolResult blocks must become separate user-role messages.
        let mut contents: Vec<Value> = Vec::new();
        let tool_name_by_id = Self::tool_name_by_id(&request.messages);

        for msg in &request.messages {
            let role = match msg.role {
                Role::User => "user",
                Role::Assistant => "model",
            };

            let blocks = msg.content_blocks();

            let mut regular_parts: Vec<Value> = Vec::new();
            let mut tool_result_parts: Vec<Value> = Vec::new();
            let flush_regular_parts = |contents: &mut Vec<Value>, parts: &mut Vec<Value>| {
                if !parts.is_empty() {
                    contents.push(json!({
                        "role": role,
                        "parts": std::mem::take(parts)
                    }));
                }
            };
            let flush_tool_result_parts = |contents: &mut Vec<Value>, parts: &mut Vec<Value>| {
                if !parts.is_empty() {
                    contents.push(json!({
                        "role": "user",
                        "parts": std::mem::take(parts)
                    }));
                }
            };

            for block in &blocks {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                {
                    flush_regular_parts(&mut contents, &mut regular_parts);
                    let tool_name = tool_name_by_id
                        .get(tool_use_id)
                        .cloned()
                        .or_else(|| Self::infer_tool_name_from_id(tool_use_id))
                        .unwrap_or_else(|| tool_use_id.clone());
                    tool_result_parts.push(Self::tool_result_to_part(&tool_name, content));
                } else if let Some(part) = Self::content_block_to_part(block) {
                    flush_tool_result_parts(&mut contents, &mut tool_result_parts);
                    regular_parts.push(part);
                }
            }

            flush_regular_parts(&mut contents, &mut regular_parts);
            flush_tool_result_parts(&mut contents, &mut tool_result_parts);
        }

        // ---- System instruction ----
        let system_instruction: Option<Value> = request.system_prompt.as_ref().map(|sp| {
            let text = match sp {
                SystemPrompt::Text(t) => t.clone(),
                SystemPrompt::Blocks(blocks) => blocks
                    .iter()
                    .map(|b| b.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            json!({ "parts": [{ "text": text }] })
        });

        // ---- Tool declarations ----
        let tools_value: Option<Value> = if request.tools.is_empty() {
            None
        } else {
            let declarations: Vec<Value> = request
                .tools
                .iter()
                .map(|td| {
                    json!({
                        "name": td.name,
                        "description": td.description,
                        "parameters": Self::sanitize_schema(td.input_schema.clone())
                    })
                })
                .collect();
            Some(json!([{ "functionDeclarations": declarations }]))
        };

        // ---- Generation config ----
        let mut gen_config = serde_json::Map::new();
        gen_config.insert("maxOutputTokens".to_string(), json!(request.max_tokens));
        if let Some(temp) = request.temperature {
            gen_config.insert("temperature".to_string(), json!(temp));
        }
        if !request.stop_sequences.is_empty() {
            gen_config.insert("stopSequences".to_string(), json!(request.stop_sequences));
        }
        if let Some(top_p) = request.top_p {
            gen_config.insert("topP".to_string(), json!(top_p));
        }
        if let Some(top_k) = request.top_k {
            gen_config.insert("topK".to_string(), json!(top_k));
        }

        // Thinking config for supported models.
        if Self::supports_thinking(&request.model) && request.thinking.is_some() {
            let budget = request
                .thinking
                .as_ref()
                .map(|t| t.budget_tokens)
                .unwrap_or(8192);
            gen_config.insert(
                "thinkingConfig".to_string(),
                json!({
                    "includeThoughts": true,
                    "thinkingBudget": budget
                }),
            );
        }

        // ---- Assemble body ----
        let mut body = serde_json::Map::new();
        body.insert("contents".to_string(), Value::Array(contents));
        body.insert("generationConfig".to_string(), Value::Object(gen_config));
        if let Some(si) = system_instruction {
            body.insert("systemInstruction".to_string(), si);
        }
        if let Some(tools) = tools_value {
            body.insert("tools".to_string(), tools);
        }

        let mut value = Value::Object(body);
        merge_google_options(&mut value, &request.provider_options);
        value
    }

    /// Parse a Google error JSON body and return the appropriate ProviderError.
    fn parse_error_response(&self, status: u16, body: &str) -> ProviderError {
        parse_http_error(status, body, &self.id)
    }

    /// Extract content blocks and usage from a completed Gemini response body.
    fn parse_response_body(
        &self,
        body: &Value,
        model: &str,
    ) -> Result<ProviderResponse, ProviderError> {
        let candidates = body
            .get("candidates")
            .and_then(|c| c.as_array())
            .ok_or_else(|| ProviderError::Other {
                provider: self.id.clone(),
                message: "Missing 'candidates' in response".to_string(),
                status: None,
                body: Some(body.to_string()),
            })?;

        let candidate = candidates.first().ok_or_else(|| ProviderError::Other {
            provider: self.id.clone(),
            message: "Empty 'candidates' array in response".to_string(),
            status: None,
            body: Some(body.to_string()),
        })?;

        let finish_reason = candidate
            .get("finishReason")
            .and_then(|r| r.as_str())
            .unwrap_or("STOP");

        let stop_reason = match finish_reason {
            "STOP" => StopReason::EndTurn,
            "MAX_TOKENS" => StopReason::MaxTokens,
            "SAFETY" => StopReason::ContentFiltered,
            "RECITATION" => StopReason::ContentFiltered,
            "TOOL_CODE" | "FUNCTION_CALL" => StopReason::ToolUse,
            other => StopReason::Other(other.to_string()),
        };

        let parts = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array());

        let mut content_blocks: Vec<ContentBlock> = Vec::new();
        let mut tool_name_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        if let Some(parts) = parts {
            for part in parts {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    content_blocks.push(ContentBlock::Text {
                        text: text.to_string(),
                    });
                } else if let Some(fc) = part.get("functionCall") {
                    let name = fc
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = fc.get("args").cloned().unwrap_or(json!({}));
                    let occurrence = tool_name_counts
                        .entry(name.clone())
                        .and_modify(|count| *count += 1)
                        .or_insert(0);
                    let id = Self::tool_use_id_for_name(&name, *occurrence);
                    content_blocks.push(ContentBlock::ToolUse {
                        id,
                        name,
                        input: args,
                    });
                }
            }
        }

        // Extract usage metadata.
        let usage = self.extract_usage(body);

        Ok(ProviderResponse {
            id: format!("gemini-{}", uuid_v4_simple()),
            content: content_blocks,
            stop_reason,
            usage,
            model: model.to_string(),
        })
    }

    /// Extract UsageInfo from a response body's usageMetadata field.
    fn extract_usage(&self, body: &Value) -> UsageInfo {
        let meta = body.get("usageMetadata");
        UsageInfo {
            input_tokens: meta
                .and_then(|m| m.get("promptTokenCount"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            output_tokens: meta
                .and_then(|m| m.get("candidatesTokenCount"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// LlmProvider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl LlmProvider for GoogleProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        "Google"
    }

    async fn create_message(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        let url = self.generate_url(&request.model);
        let model = request.model.clone();
        let body = self.build_request_body(&request);

        debug!("Google create_message: POST {}", url);

        let resp = self
            .http_client
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ServerError {
                provider: self.id.clone(),
                status: None,
                message: e.to_string(),
                is_retryable: true,
            })?;

        let status = resp.status().as_u16();
        let resp_body = resp.text().await.map_err(|e| ProviderError::ServerError {
            provider: self.id.clone(),
            status: Some(status),
            message: e.to_string(),
            is_retryable: true,
        })?;

        if status >= 400 {
            return Err(self.parse_error_response(status, &resp_body));
        }

        let json_body: Value =
            serde_json::from_str(&resp_body).map_err(|e| ProviderError::Other {
                provider: self.id.clone(),
                message: format!("Failed to parse response JSON: {}", e),
                status: Some(status),
                body: Some(resp_body.clone()),
            })?;

        self.parse_response_body(&json_body, &model)
    }

    async fn create_message_stream(
        &self,
        request: ProviderRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        let url = self.stream_url(&request.model);
        let model = request.model.clone();
        let body = self.build_request_body(&request);

        debug!("Google create_message_stream: POST {}", url);

        let resp = self
            .http_client
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::ServerError {
                provider: self.id.clone(),
                status: None,
                message: e.to_string(),
                is_retryable: true,
            })?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let resp_body = resp
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable>".to_string());
            return Err(self.parse_error_response(status, &resp_body));
        }

        // Wrap the byte stream in a line-based SSE parser.
        let provider_id_for_stream = self.id.clone();
        let model_clone = model.clone();
        let byte_stream = resp.bytes_stream();

        let stream = async_stream::stream! {
            let mut byte_stream = byte_stream;
            let text_block_index: usize = 0;
            let mut tool_block_index: usize = 1000;
            let mut open_tool_calls: std::collections::HashMap<usize, (usize, String, String)> =
                std::collections::HashMap::new();
            let mut emitted_message_start = false;
            let message_id = format!("gemini-{}", uuid_v4_simple());
            let mut line_buf = String::new();
            let mut tool_name_counts: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk: Bytes = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        yield Err(ProviderError::StreamError {
                            provider: provider_id_for_stream.clone(),
                            message: e.to_string(),
                            partial_response: None,
                        });
                        return;
                    }
                };

                let chunk_str = match std::str::from_utf8(&chunk) {
                    Ok(s) => s,
                    Err(_) => {
                        warn!("Google SSE: non-UTF8 chunk, skipping");
                        continue;
                    }
                };

                line_buf.push_str(chunk_str);

                // Process complete lines.
                while let Some(newline_pos) = line_buf.find('\n') {
                    let line = line_buf[..newline_pos].trim_end_matches('\r').to_string();
                    line_buf = line_buf[newline_pos + 1..].to_string();

                    if let Some(data) = line.strip_prefix("data: ") {
                        let data = data.trim();
                        if data.is_empty() || data == "[DONE]" {
                            continue;
                        }

                        // Parse the JSON payload and emit events.
                        let parsed: Value = match serde_json::from_str(data) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!("Google SSE: JSON parse error: {}: {}", e, data);
                                continue;
                            }
                        };

                        // Check for stream-level error.
                        if let Some(err) = parsed.get("error") {
                            let msg = err
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown error")
                                .to_string();
                            yield Err(ProviderError::StreamError {
                                provider: provider_id_for_stream.clone(),
                                message: msg,
                                partial_response: None,
                            });
                            return;
                        }

                        // Emit MessageStart on first chunk.
                        if !emitted_message_start {
                            emitted_message_start = true;
                            let meta = parsed.get("usageMetadata");
                            let usage = UsageInfo {
                                input_tokens: meta
                                    .and_then(|m| m.get("promptTokenCount"))
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0),
                                output_tokens: meta
                                    .and_then(|m| m.get("candidatesTokenCount"))
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0),
                                cache_creation_input_tokens: 0,
                                cache_read_input_tokens: 0,
                            };
                            yield Ok(StreamEvent::MessageStart {
                                id: message_id.clone(),
                                model: model_clone.clone(),
                                usage,
                            });
                        }

                        let candidates = parsed
                            .get("candidates")
                            .and_then(|c| c.as_array());

                        let Some(candidates) = candidates else { continue };

                        for candidate in candidates {
                            let parts = candidate
                                .get("content")
                                .and_then(|c| c.get("parts"))
                                .and_then(|p| p.as_array());

                            if let Some(parts) = parts {
                                for (part_idx, part) in parts.iter().enumerate() {
                                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                        yield Ok(StreamEvent::TextDelta {
                                            index: text_block_index,
                                            text: text.to_string(),
                                        });
                                    } else if let Some(fc) = part.get("functionCall") {
                                        let name = fc
                                            .get("name")
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let args_str = fc
                                            .get("args")
                                            .map(|a| a.to_string())
                                            .unwrap_or_else(|| "{}".to_string());

                                        let idx = if let Some((existing_idx, _, _)) = open_tool_calls.get(&part_idx) {
                                            *existing_idx
                                        } else {
                                            let occurrence = tool_name_counts
                                                .entry(name.clone())
                                                .and_modify(|count| *count += 1)
                                                .or_insert(0);
                                            let id = Self::tool_use_id_for_name(&name, *occurrence);
                                            let idx = tool_block_index;
                                            tool_block_index += 1;
                                            open_tool_calls.insert(part_idx, (idx, id.clone(), name.clone()));
                                            yield Ok(StreamEvent::ContentBlockStart {
                                                index: idx,
                                                content_block: ContentBlock::ToolUse {
                                                    id,
                                                    name: name.clone(),
                                                    input: json!({}),
                                                },
                                            });
                                            idx
                                        };

                                        yield Ok(StreamEvent::InputJsonDelta {
                                            index: idx,
                                            partial_json: args_str,
                                        });
                                    }
                                }
                            }

                            // Handle finish reason.
                            let finish_reason = candidate
                                .get("finishReason")
                                .and_then(|r| r.as_str())
                                .unwrap_or("");

                            if !finish_reason.is_empty()
                                && finish_reason != "FINISH_REASON_UNSPECIFIED"
                            {
                                // Close text block.
                                yield Ok(StreamEvent::ContentBlockStop {
                                    index: text_block_index,
                                });

                                // Close tool call blocks.
                                let mut tool_indices: Vec<usize> =
                                    open_tool_calls
                                        .values()
                                        .map(|(idx, _, _)| *idx)
                                        .collect();
                                tool_indices.sort_unstable();
                                for idx in tool_indices {
                                    yield Ok(StreamEvent::ContentBlockStop { index: idx });
                                }
                                open_tool_calls.clear();

                                let stop_reason = match finish_reason {
                                    "STOP" => Some(StopReason::EndTurn),
                                    "MAX_TOKENS" => Some(StopReason::MaxTokens),
                                    "SAFETY" | "RECITATION" => Some(StopReason::ContentFiltered),
                                    "TOOL_CODE" | "FUNCTION_CALL" => Some(StopReason::ToolUse),
                                    other => Some(StopReason::Other(other.to_string())),
                                };

                                let meta = parsed.get("usageMetadata");
                                let final_usage = UsageInfo {
                                    input_tokens: meta
                                        .and_then(|m| m.get("promptTokenCount"))
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0),
                                    output_tokens: meta
                                        .and_then(|m| m.get("candidatesTokenCount"))
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0),
                                    cache_creation_input_tokens: 0,
                                    cache_read_input_tokens: 0,
                                };

                                yield Ok(StreamEvent::MessageDelta {
                                    stop_reason,
                                    usage: Some(final_usage),
                                });
                                yield Ok(StreamEvent::MessageStop);
                            }
                        }
                    }
                    // SSE comment lines (": ...") and blank lines are ignored.
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = format!("{}/v1beta/models?key={}", self.base_url, self.api_key);

        let resp = self
            .http_client
            .get(&url)
            .header("x-goog-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| ProviderError::ServerError {
                provider: self.id.clone(),
                status: None,
                message: e.to_string(),
                is_retryable: true,
            })?;

        let status = resp.status().as_u16();
        let body_text = resp.text().await.map_err(|e| ProviderError::ServerError {
            provider: self.id.clone(),
            status: Some(status),
            message: e.to_string(),
            is_retryable: true,
        })?;

        if status >= 400 {
            return Err(self.parse_error_response(status, &body_text));
        }

        let body: Value = serde_json::from_str(&body_text).map_err(|e| ProviderError::Other {
            provider: self.id.clone(),
            message: format!("Failed to parse models list JSON: {}", e),
            status: Some(status),
            body: Some(body_text.clone()),
        })?;

        let models_array = body
            .get("models")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();

        let provider_id = self.id.clone();
        let models: Vec<ModelInfo> = models_array
            .iter()
            .filter_map(|m| {
                let name = m.get("name").and_then(|n| n.as_str())?;
                // Only include Gemini models (filter out palm, embedding, etc.)
                if !name.starts_with("models/gemini-") {
                    return None;
                }
                // Strip the "models/" prefix for the model ID.
                let model_id = name.strip_prefix("models/").unwrap_or(name);
                let display = m
                    .get("displayName")
                    .and_then(|d| d.as_str())
                    .unwrap_or(model_id)
                    .to_string();
                let input_limit = m
                    .get("inputTokenLimit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(32_768) as u32;
                let output_limit = m
                    .get("outputTokenLimit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(8_192) as u32;

                Some(ModelInfo {
                    id: ModelId::new(model_id),
                    provider_id: provider_id.clone(),
                    name: display,
                    context_window: input_limit,
                    max_output_tokens: output_limit,
                })
            })
            .collect();

        Ok(models)
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        // Use list_models as a lightweight liveness check.
        match self.list_models().await {
            Ok(models) if !models.is_empty() => Ok(ProviderStatus::Healthy),
            Ok(_) => Ok(ProviderStatus::Degraded {
                reason: "No Gemini models returned".to_string(),
            }),
            Err(ProviderError::AuthFailed { message, .. }) => Err(ProviderError::AuthFailed {
                provider: self.id.clone(),
                message,
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
            thinking: true,
            image_input: true,
            pdf_input: true,
            audio_input: false,
            video_input: true,
            caching: false,
            structured_output: true,
            system_prompt_style: SystemPromptStyle::SystemInstruction,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a simple pseudo-random hex ID without pulling in the uuid crate.
/// Uses a combination of the current time and a thread-local counter.
fn uuid_v4_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Simple hash mix to spread bits.
    let a = t ^ (t >> 17) ^ (t << 13);
    let b = a.wrapping_mul(0x517cc1b727220a95);
    format!("{:032x}", b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use claurst_core::types::Message;
    use serde_json::json;

    fn test_request(messages: Vec<Message>) -> ProviderRequest {
        ProviderRequest {
            model: "gemini-3-flash-preview".to_string(),
            messages,
            system_prompt: None,
            tools: vec![],
            max_tokens: 512,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: None,
            provider_options: json!({}),
        }
    }

    #[test]
    fn build_request_body_uses_function_names_for_tool_results() {
        let provider = GoogleProvider::new("test".to_string());
        let request = test_request(vec![
            Message::assistant_blocks(vec![ContentBlock::ToolUse {
                id: "call_search_2".to_string(),
                name: "search".to_string(),
                input: json!({"q": "cats"}),
            }]),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "call_search_2".to_string(),
                content: ToolResultContent::Text("ok".to_string()),
                is_error: Some(false),
            }]),
        ]);

        let body = provider.build_request_body(&request);
        let contents = body["contents"].as_array().expect("contents array");
        assert_eq!(contents.len(), 2);
        assert_eq!(
            contents[1]["parts"][0]["functionResponse"]["name"],
            json!("search")
        );
    }

    #[test]
    fn build_request_body_preserves_tool_result_order() {
        let provider = GoogleProvider::new("test".to_string());
        let request = test_request(vec![Message::user_blocks(vec![
            ContentBlock::Text {
                text: "before".to_string(),
            },
            ContentBlock::ToolResult {
                tool_use_id: "call_search".to_string(),
                content: ToolResultContent::Text("done".to_string()),
                is_error: Some(false),
            },
            ContentBlock::Text {
                text: "after".to_string(),
            },
        ])]);

        let body = provider.build_request_body(&request);
        let contents = body["contents"].as_array().expect("contents array");
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], json!("user"));
        assert_eq!(contents[0]["parts"][0]["text"], json!("before"));
        assert_eq!(
            contents[1]["parts"][0]["functionResponse"]["name"],
            json!("search")
        );
        assert_eq!(contents[2]["parts"][0]["text"], json!("after"));
    }

    #[test]
    fn parse_response_body_assigns_unique_ids_for_duplicate_tool_names() {
        let provider = GoogleProvider::new("test".to_string());
        let response = json!({
            "candidates": [{
                "finishReason": "FUNCTION_CALL",
                "content": {
                    "parts": [
                        { "functionCall": { "name": "search", "args": { "q": "a" } } },
                        { "functionCall": { "name": "search", "args": { "q": "b" } } }
                    ]
                }
            }],
            "usageMetadata": {}
        });

        let parsed = provider
            .parse_response_body(&response, "gemini-3-flash-preview")
            .expect("parsed response");

        assert!(matches!(
            &parsed.content[0],
            ContentBlock::ToolUse { id, .. } if id == "call_search"
        ));
        assert!(matches!(
            &parsed.content[1],
            ContentBlock::ToolUse { id, .. } if id == "call_search_2"
        ));
    }
}
