// providers/copilot.rs — GitHub Copilot provider adapter.
//
// GitHub Copilot exposes an OpenAI-compatible API with special auth and
// routing headers.
//
// Chat Completions: POST https://api.githubcopilot.com/chat/completions
//
// GPT-5-class Copilot models require the Responses API while the rest still use
// Chat Completions. OpenCode makes the same split, so this adapter now follows
// the same routing rule instead of forcing every model through
// /chat/completions.
//
// Required headers on model/chat requests:
//   Authorization: Bearer <github_token>
//   User-Agent: claurst/0.0.9
//   Openai-Intent: conversation-edits
//   x-initiator: user | agent
//
// Env: GITHUB_TOKEN

use std::pin::Pin;

use async_stream::stream;
use async_trait::async_trait;
use claurst_core::provider_id::{ModelId, ProviderId};
use claurst_core::types::{
    ContentBlock, ImageSource, MessageContent, Role, ToolResultContent, UsageInfo,
};
use futures::Stream;
use serde_json::{json, Value};
use tracing::debug;

use crate::error_handling::parse_error_response;
use crate::provider::{LlmProvider, ModelInfo};
use crate::provider_error::ProviderError;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StopReason,
    StreamEvent, SystemPromptStyle,
};
use crate::providers::openai::OpenAiProvider;

// ---------------------------------------------------------------------------
// CopilotProvider
// ---------------------------------------------------------------------------

pub struct CopilotProvider {
    id: ProviderId,
    token: String,
    http_client: reqwest::Client,
}

impl CopilotProvider {
    pub fn new(token: String) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("failed to build reqwest client");

        Self {
            id: ProviderId::new(ProviderId::GITHUB_COPILOT),
            token,
            http_client,
        }
    }

    pub fn from_env() -> Option<Self> {
        std::env::var("GITHUB_TOKEN").ok().map(|t| Self::new(t))
    }

    fn base_url() -> &'static str {
        "https://api.githubcopilot.com"
    }

    fn block_has_image(block: &ContentBlock) -> bool {
        match block {
            ContentBlock::Image { .. } => true,
            ContentBlock::ToolResult { content, .. } => match content {
                ToolResultContent::Text(_) => false,
                ToolResultContent::Blocks(blocks) => blocks.iter().any(Self::block_has_image),
            },
            _ => false,
        }
    }

    fn message_has_image(content: &MessageContent) -> bool {
        match content {
            MessageContent::Text(_) => false,
            MessageContent::Blocks(blocks) => blocks.iter().any(Self::block_has_image),
        }
    }

    fn request_has_image(request: &ProviderRequest) -> bool {
        request
            .messages
            .iter()
            .any(|message| Self::message_has_image(&message.content))
    }

    fn image_source_to_url(source: &ImageSource) -> String {
        if let Some(url) = &source.url {
            return url.clone();
        }
        let media_type = source.media_type.as_deref().unwrap_or("image/png");
        let data = source.data.as_deref().unwrap_or("");
        format!("data:{};base64,{}", media_type, data)
    }

    fn request_initiator(request: &ProviderRequest) -> &'static str {
        match request.messages.last() {
            Some(message) if message.role == Role::User => "user",
            _ => "agent",
        }
    }

    fn copilot_headers(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        builder
            .bearer_auth(&self.token)
            .header("User-Agent", "claurst/0.0.9")
    }

    fn copilot_request_headers(
        &self,
        builder: reqwest::RequestBuilder,
        request: &ProviderRequest,
    ) -> reqwest::RequestBuilder {
        let builder = self
            .copilot_headers(builder)
            .header("Openai-Intent", "conversation-edits")
            .header("x-initiator", Self::request_initiator(request));

        if Self::request_has_image(request) {
            builder.header("Copilot-Vision-Request", "true")
        } else {
            builder
        }
    }

    fn apply_chat_provider_options(body: &mut Value, provider_options: &Value) {
        let Some(options) = provider_options.as_object() else {
            return;
        };

        for (key, value) in options {
            match key.as_str() {
                "reasoningEffort" => body["reasoning_effort"] = value.clone(),
                "textVerbosity" => body["verbosity"] = value.clone(),
                "thinking_budget" => body["thinking_budget"] = value.clone(),
                "reasoningSummary" | "include" => {}
                _ => body[key] = value.clone(),
            }
        }
    }

    fn apply_responses_provider_options(body: &mut Value, provider_options: &Value) {
        let Some(options) = provider_options.as_object() else {
            return;
        };

        let reasoning_effort = options.get("reasoningEffort").cloned();
        let reasoning_summary = options.get("reasoningSummary").cloned();
        if reasoning_effort.is_some() || reasoning_summary.is_some() {
            let mut reasoning = serde_json::Map::new();
            if let Some(value) = reasoning_effort {
                reasoning.insert("effort".to_string(), value);
            }
            if let Some(value) = reasoning_summary {
                reasoning.insert("summary".to_string(), value);
            }
            body["reasoning"] = Value::Object(reasoning);
        }

        if let Some(value) = options.get("textVerbosity") {
            body["text"] = json!({ "verbosity": value });
        }
        if let Some(value) = options.get("include") {
            body["include"] = value.clone();
        }

        for (key, value) in options {
            match key.as_str() {
                "reasoningEffort" | "reasoningSummary" | "textVerbosity" | "thinking_budget"
                | "include" => {}
                _ => body[key] = value.clone(),
            }
        }
    }

    fn use_responses_api(model: &str) -> bool {
        let model = model.trim().to_ascii_lowercase();
        if model.starts_with("gpt-5-mini") {
            return false;
        }

        let Some(rest) = model.strip_prefix("gpt-") else {
            return false;
        };

        let major: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        major
            .parse::<u32>()
            .map(|value| value >= 5)
            .unwrap_or(false)
    }

    /// Check whether a provider error indicates the model/endpoint is
    /// unsupported and a fallback to Chat Completions is worth trying.
    fn is_responses_api_fallback_candidate(err: &ProviderError) -> bool {
        matches!(
            err,
            ProviderError::InvalidRequest { .. }
                | ProviderError::ModelNotFound { .. }
                | ProviderError::Other {
                    status: Some(400..=499),
                    ..
                }
        )
    }

    fn system_prompt_to_text(request: &ProviderRequest) -> Option<String> {
        request.system_prompt.as_ref().map(|prompt| match prompt {
            crate::provider_types::SystemPrompt::Text(text) => text.clone(),
            crate::provider_types::SystemPrompt::Blocks(blocks) => blocks
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        })
    }

    fn tool_result_to_response_output(content: &ToolResultContent) -> String {
        match content {
            ToolResultContent::Text(text) => text.clone(),
            ToolResultContent::Blocks(blocks) => {
                let text = blocks
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.is_empty() {
                    serde_json::to_string(blocks).unwrap_or_else(|_| "[]".to_string())
                } else {
                    text
                }
            }
        }
    }

    fn user_block_to_responses_part(block: &ContentBlock, index: usize) -> Option<Value> {
        match block {
            ContentBlock::Text { text } => Some(json!({
                "type": "input_text",
                "text": text,
            })),
            ContentBlock::Image { source } => Some(json!({
                "type": "input_image",
                "image_url": Self::image_source_to_url(source),
            })),
            ContentBlock::Document { source, .. }
                if source.media_type.as_deref() == Some("application/pdf") =>
            {
                if let Some(url) = &source.url {
                    Some(json!({
                        "type": "input_file",
                        "file_url": url,
                    }))
                } else {
                    source.data.as_ref().map(|data| {
                        json!({
                            "type": "input_file",
                            "filename": format!("document-{}.pdf", index),
                            "file_data": format!("data:application/pdf;base64,{}", data),
                        })
                    })
                }
            }
            _ => None,
        }
    }

    /// Public re-export so other providers (e.g. `CodexProvider`) can reuse
    /// the same Responses-API message translation without duplicating the logic.
    pub fn to_responses_input_pub(request: &ProviderRequest) -> Vec<Value> {
        Self::to_responses_input(request)
    }

    fn to_responses_input(request: &ProviderRequest) -> Vec<Value> {
        let mut input = Vec::new();

        if let Some(system_text) = Self::system_prompt_to_text(request) {
            input.push(json!({
                "role": "system",
                "content": [{
                    "type": "input_text",
                    "text": system_text,
                }],
            }));
        }

        for message in &request.messages {
            match &message.content {
                MessageContent::Text(text) => {
                    let role = match &message.role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                    };
                    let content_type = if matches!(&message.role, Role::Assistant) {
                        "output_text"
                    } else {
                        "input_text"
                    };
                    input.push(json!({
                        "role": role,
                        "content": [{
                            "type": content_type,
                            "text": text,
                        }],
                    }));
                }
                MessageContent::Blocks(blocks) => match &message.role {
                    Role::User => {
                        let mut message_parts = Vec::new();
                        let flush_user_content =
                            |input: &mut Vec<Value>, content: &mut Vec<Value>| {
                                if !content.is_empty() {
                                    input.push(json!({
                                        "role": "user",
                                        "content": std::mem::take(content),
                                    }));
                                }
                            };
                        for (index, block) in blocks.iter().enumerate() {
                            if let Some(part) = Self::user_block_to_responses_part(block, index) {
                                message_parts.push(part);
                            } else if let ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } = block
                            {
                                flush_user_content(&mut input, &mut message_parts);
                                input.push(json!({
                                    "type": "function_call_output",
                                    "call_id": tool_use_id,
                                    "output": Self::tool_result_to_response_output(content),
                                }));
                            }
                        }
                        flush_user_content(&mut input, &mut message_parts);
                    }
                    Role::Assistant => {
                        let mut message_parts = Vec::new();
                        let flush_assistant_content =
                            |input: &mut Vec<Value>, content: &mut Vec<Value>| {
                                if !content.is_empty() {
                                    input.push(json!({
                                        "role": "assistant",
                                        "content": std::mem::take(content),
                                    }));
                                }
                            };
                        for block in blocks {
                            match block {
                                ContentBlock::Text { text } => message_parts.push(json!({
                                    "type": "output_text",
                                    "text": text,
                                })),
                                ContentBlock::ToolUse {
                                    id,
                                    name,
                                    input: tool_input,
                                } => {
                                    flush_assistant_content(&mut input, &mut message_parts);
                                    input.push(json!({
                                        "type": "function_call",
                                        "call_id": id,
                                        "name": name,
                                        "arguments": serde_json::to_string(tool_input)
                                            .unwrap_or_else(|_| "{}".to_string()),
                                    }));
                                }
                                ContentBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                                    flush_assistant_content(&mut input, &mut message_parts);
                                    input.push(json!({
                                        "type": "reasoning",
                                        "summary": [{
                                            "type": "summary_text",
                                            "text": thinking,
                                        }],
                                    }));
                                }
                                ContentBlock::RedactedThinking { data } if !data.is_empty() => {
                                    flush_assistant_content(&mut input, &mut message_parts);
                                    input.push(json!({
                                        "type": "reasoning",
                                        "encrypted_content": data,
                                        "summary": [],
                                    }));
                                }
                                _ => {}
                            }
                        }
                        flush_assistant_content(&mut input, &mut message_parts);
                    }
                },
            }
        }

        input
    }

    fn map_responses_finish_reason(reason: Option<&str>, has_tool_call: bool) -> StopReason {
        if has_tool_call {
            return StopReason::ToolUse;
        }

        match reason {
            Some("max_output_tokens") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::ContentFiltered,
            Some(other) if !other.is_empty() => StopReason::Other(other.to_string()),
            _ => StopReason::EndTurn,
        }
    }

    fn parse_responses_usage(usage: Option<&Value>) -> UsageInfo {
        let Some(usage) = usage else {
            return UsageInfo::default();
        };

        UsageInfo {
            input_tokens: usage
                .get("input_tokens")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            output_tokens: usage
                .get("output_tokens")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: usage
                .get("input_tokens_details")
                .and_then(|value| value.get("cached_tokens"))
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
        }
    }

    fn map_http_error(&self, status: u16, body: &str) -> ProviderError {
        parse_error_response(status, body, &self.id)
    }

    /// Hardcoded fallback model list used when the /models endpoint is
    /// unreachable or returns empty data.
    fn hardcoded_models(provider_id: &ProviderId) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: ModelId::new("claude-sonnet-4.6"),
                provider_id: provider_id.clone(),
                name: "Claude Sonnet 4.6 (Copilot)".into(),
                context_window: 128_000,
                max_output_tokens: 32_000,
            },
            ModelInfo {
                id: ModelId::new("claude-sonnet-4.5"),
                provider_id: provider_id.clone(),
                name: "Claude Sonnet 4.5 (Copilot)".into(),
                context_window: 128_000,
                max_output_tokens: 32_000,
            },
            ModelInfo {
                id: ModelId::new("claude-haiku-4.5"),
                provider_id: provider_id.clone(),
                name: "Claude Haiku 4.5 (Copilot)".into(),
                context_window: 128_000,
                max_output_tokens: 32_000,
            },
            ModelInfo {
                id: ModelId::new("gpt-4.1"),
                provider_id: provider_id.clone(),
                name: "GPT-4.1 (Copilot)".into(),
                context_window: 64_000,
                max_output_tokens: 16_384,
            },
            ModelInfo {
                id: ModelId::new("gpt-4o"),
                provider_id: provider_id.clone(),
                name: "GPT-4o (Copilot)".into(),
                context_window: 128_000,
                max_output_tokens: 16_384,
            },
            ModelInfo {
                id: ModelId::new("gpt-4o-mini"),
                provider_id: provider_id.clone(),
                name: "GPT-4o Mini (Copilot)".into(),
                context_window: 128_000,
                max_output_tokens: 16_384,
            },
            ModelInfo {
                id: ModelId::new("gpt-5.4"),
                provider_id: provider_id.clone(),
                name: "GPT-5.4 (Copilot)".into(),
                context_window: 128_000,
                max_output_tokens: 128_000,
            },
            ModelInfo {
                id: ModelId::new("gpt-5-mini"),
                provider_id: provider_id.clone(),
                name: "GPT-5 Mini (Copilot)".into(),
                context_window: 128_000,
                max_output_tokens: 128_000,
            },
            ModelInfo {
                id: ModelId::new("o3-mini"),
                provider_id: provider_id.clone(),
                name: "o3-mini (Copilot)".into(),
                context_window: 200_000,
                max_output_tokens: 100_000,
            },
            ModelInfo {
                id: ModelId::new("o4-mini"),
                provider_id: provider_id.clone(),
                name: "o4-mini (Copilot)".into(),
                context_window: 200_000,
                max_output_tokens: 100_000,
            },
            ModelInfo {
                id: ModelId::new("gemini-3-flash-preview"),
                provider_id: provider_id.clone(),
                name: "Gemini 3 Flash (Copilot)".into(),
                context_window: 128_000,
                max_output_tokens: 64_000,
            },
        ]
    }

    fn parse_responses_response(
        &self,
        json_val: &Value,
    ) -> Result<ProviderResponse, ProviderError> {
        let id = json_val
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
            .to_string();
        let model = json_val
            .get("model")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        let output = json_val
            .get("output")
            .and_then(|value| value.as_array())
            .ok_or_else(|| ProviderError::Other {
                provider: self.id.clone(),
                message: "No output in Copilot Responses API response".to_string(),
                status: None,
                body: Some(json_val.to_string()),
            })?;

        let mut content = Vec::new();
        let mut has_tool_call = false;

        for item in output {
            match item.get("type").and_then(|value| value.as_str()) {
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(|value| value.as_array()) {
                        for part in parts {
                            match part.get("type").and_then(|value| value.as_str()) {
                                Some("output_text") | Some("text") => {
                                    if let Some(text) =
                                        part.get("text").and_then(|value| value.as_str())
                                    {
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
                    // Extract reasoning summary text from the Responses API.
                    // Format: { "type": "reasoning", "summary": [{ "type": "summary_text", "text": "..." }] }
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
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = item
                        .get("arguments")
                        .and_then(|value| value.as_str())
                        .unwrap_or("{}");
                    let input = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                    content.push(ContentBlock::ToolUse { id, name, input });
                }
                _ => {}
            }
        }

        let stop_reason = Self::map_responses_finish_reason(
            json_val
                .get("incomplete_details")
                .and_then(|value| value.get("reason"))
                .and_then(|value| value.as_str()),
            has_tool_call,
        );
        let usage = Self::parse_responses_usage(json_val.get("usage"));

        Ok(ProviderResponse {
            id,
            content,
            stop_reason,
            usage,
            model,
        })
    }

    async fn send_responses_non_streaming(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
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
            "input": Self::to_responses_input(request),
            "max_output_tokens": request.max_tokens,
            "store": false,
        });

        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        Self::apply_responses_provider_options(&mut body, &request.provider_options);

        let url = format!("{}/responses", Self::base_url());
        let builder = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
        let builder = self.copilot_request_headers(builder, request);

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

        let json_val: Value = serde_json::from_str(&text).map_err(|e| ProviderError::Other {
            provider: self.id.clone(),
            message: format!("Failed to parse response JSON: {}", e),
            status: Some(status),
            body: Some(text.clone()),
        })?;

        self.parse_responses_response(&json_val)
    }

    fn stream_synthetic_response(
        &self,
        response: ProviderResponse,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>> {
        let stream = stream! {
            yield Ok(StreamEvent::MessageStart {
                id: response.id.clone(),
                model: response.model.clone(),
                usage: UsageInfo::default(),
            });

            for (index, block) in response.content.iter().enumerate() {
                let start_block = match block {
                    ContentBlock::Text { .. } => ContentBlock::Text { text: String::new() },
                    ContentBlock::ToolUse { id, name, .. } => ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: json!({}),
                    },
                    ContentBlock::Thinking { .. } => ContentBlock::Thinking {
                        thinking: String::new(),
                        signature: String::new(),
                    },
                    other => other.clone(),
                };
                yield Ok(StreamEvent::ContentBlockStart {
                    index,
                    content_block: start_block,
                });

                match block {
                    ContentBlock::Text { text } if !text.is_empty() => {
                        yield Ok(StreamEvent::TextDelta {
                            index,
                            text: text.clone(),
                        });
                    }
                    ContentBlock::ToolUse { input, .. } => {
                        let json_str = serde_json::to_string(input)
                            .unwrap_or_else(|_| "{}".to_string());
                        if json_str != "{}" {
                            yield Ok(StreamEvent::InputJsonDelta {
                                index,
                                partial_json: json_str,
                            });
                        }
                    }
                    ContentBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                        yield Ok(StreamEvent::ThinkingDelta {
                            index,
                            thinking: thinking.clone(),
                        });
                    }
                    _ => {}
                }

                yield Ok(StreamEvent::ContentBlockStop { index });
            }

            yield Ok(StreamEvent::MessageDelta {
                stop_reason: Some(response.stop_reason.clone()),
                usage: Some(response.usage.clone()),
            });
            yield Ok(StreamEvent::MessageStop);
        };

        Box::pin(stream)
    }

    async fn send_non_streaming(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        let messages = OpenAiProvider::to_openai_messages_pub(
            &request.messages,
            request.system_prompt.as_ref(),
        );
        let tools = OpenAiProvider::to_openai_tools_pub(&request.tools);

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
        Self::apply_chat_provider_options(&mut body, &request.provider_options);

        let url = format!("{}/chat/completions", Self::base_url());

        let builder = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json");
        let builder = self.copilot_request_headers(builder, request);

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

        let json_val: Value = serde_json::from_str(&text).map_err(|e| ProviderError::Other {
            provider: self.id.clone(),
            message: format!("Failed to parse response JSON: {}", e),
            status: Some(status),
            body: Some(text.clone()),
        })?;

        OpenAiProvider::parse_non_streaming_response_pub(&json_val, &self.id)
    }

    async fn do_streaming(
        &self,
        request: &ProviderRequest,
    ) -> Result<reqwest::Response, ProviderError> {
        let messages = OpenAiProvider::to_openai_messages_pub(
            &request.messages,
            request.system_prompt.as_ref(),
        );
        let tools = OpenAiProvider::to_openai_tools_pub(&request.tools);

        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
            "stream": true,
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
        Self::apply_chat_provider_options(&mut body, &request.provider_options);

        let url = format!("{}/chat/completions", Self::base_url());

        let builder = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");
        let builder = self.copilot_request_headers(builder, request);

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
impl LlmProvider for CopilotProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        "GitHub Copilot"
    }

    async fn create_message(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        if Self::use_responses_api(&request.model) {
            match self.send_responses_non_streaming(&request).await {
                Ok(resp) => return Ok(resp),
                Err(e) if Self::is_responses_api_fallback_candidate(&e) => {
                    // Responses API rejected the model — fall back to Chat Completions.
                    // Some OAuth apps / Copilot plans only expose models via /chat/completions.
                    debug!(model = %request.model, error = %e, "Responses API rejected, falling back to Chat Completions");
                }
                Err(e) => return Err(e),
            }
        }
        self.send_non_streaming(&request).await
    }

    async fn create_message_stream(
        &self,
        request: ProviderRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        if Self::use_responses_api(&request.model) {
            match self.send_responses_non_streaming(&request).await {
                Ok(response) => return Ok(self.stream_synthetic_response(response)),
                Err(e) if Self::is_responses_api_fallback_candidate(&e) => {
                    debug!(model = %request.model, error = %e, "Responses API rejected, falling back to Chat Completions");
                }
                Err(e) => return Err(e),
            }
        }

        let resp = self.do_streaming(&request).await?;
        let provider_id = self.id.clone();

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
                            debug!("Failed to parse Copilot SSE chunk: {}: {}", e, data);
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

                    // Extract reasoning traces (Copilot uses "reasoning_text").
                    for field in &["reasoning_text", "reasoning_content", "reasoning"] {
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

                    if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                        if !content.is_empty() {
                            yield Ok(StreamEvent::TextDelta {
                                index: 0,
                                text: content.to_string(),
                            });
                        }
                    }

                    if let Some(tool_calls) =
                        delta.get("tool_calls").and_then(|t| t.as_array())
                    {
                        for tc in tool_calls {
                            let tc_index = tc
                                .get("index")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as usize;
                            if let Some(tc_id) = tc.get("id").and_then(|v| v.as_str()) {
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
                                        input: serde_json::json!({}),
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
                            let usage =
                                usage_val.map(|u| OpenAiProvider::parse_usage_pub(Some(u)));

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
        // Try to fetch the live model list from the Copilot API.
        let url = format!("{}/models", Self::base_url());
        let builder = self.http_client.get(&url);
        let builder = self
            .copilot_headers(builder)
            .header("Accept", "application/json");

        let resp = builder.send().await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let text = r.text().await.map_err(|e| ProviderError::Other {
                    provider: self.id.clone(),
                    message: e.to_string(),
                    status: None,
                    body: None,
                })?;
                let json: Value =
                    serde_json::from_str(&text).map_err(|e| ProviderError::Other {
                        provider: self.id.clone(),
                        message: format!("Failed to parse models JSON: {}", e),
                        status: None,
                        body: Some(text.clone()),
                    })?;

                let mut models = Vec::new();

                // The Copilot /models endpoint may return { "data": [...] } or
                // a top-level array.
                let items: Option<&Vec<Value>> = json
                    .get("data")
                    .and_then(|d| d.as_array())
                    .or_else(|| json.as_array());

                if let Some(arr) = items {
                    for item in arr {
                        if item.get("model_picker_enabled").and_then(|v| v.as_bool()) == Some(false)
                        {
                            continue;
                        }
                        if let Some(endpoints) =
                            item.get("supported_endpoints").and_then(|v| v.as_array())
                        {
                            let supports_text_generation = endpoints.iter().any(|endpoint| {
                                endpoint
                                    .as_str()
                                    .map(|value| {
                                        value.contains("chat/completions")
                                            || value.contains("/responses")
                                    })
                                    .unwrap_or(false)
                            });
                            if !supports_text_generation {
                                continue;
                            }
                        }
                        if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or(id);
                            let ctx = item
                                .get("context_window")
                                .or_else(|| {
                                    item.get("capabilities").and_then(|c| {
                                        c.get("limits")
                                            .and_then(|l| l.get("max_context_window_tokens"))
                                    })
                                })
                                .or_else(|| {
                                    item.get("capabilities").and_then(|c| {
                                        c.get("limits").and_then(|l| l.get("max_prompt_tokens"))
                                    })
                                })
                                .and_then(|v| v.as_u64())
                                .unwrap_or(128_000) as u32;
                            let max_out = item
                                .get("max_output_tokens")
                                .or_else(|| {
                                    item.get("capabilities").and_then(|c| {
                                        c.get("limits").and_then(|l| l.get("max_output_tokens"))
                                    })
                                })
                                .and_then(|v| v.as_u64())
                                .unwrap_or(16_384) as u32;
                            models.push(ModelInfo {
                                id: ModelId::new(id),
                                provider_id: self.id.clone(),
                                name: name.to_string(),
                                context_window: ctx,
                                max_output_tokens: max_out,
                            });
                        }
                    }
                }

                if !models.is_empty() {
                    Ok(models)
                } else {
                    // API returned but no usable models — fall back to hardcoded.
                    Ok(Self::hardcoded_models(&self.id))
                }
            }
            _ => {
                // Network error or non-success status — fall back to hardcoded.
                Ok(Self::hardcoded_models(&self.id))
            }
        }
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        let url = format!("{}/models", Self::base_url());
        let builder = self.http_client.get(&url);
        let builder = self.copilot_headers(builder);

        let resp = builder.send().await;
        match resp {
            Ok(r) if r.status().is_success() => Ok(ProviderStatus::Healthy),
            Ok(r) if r.status().as_u16() == 401 || r.status().as_u16() == 403 => {
                Ok(ProviderStatus::Unavailable {
                    reason: "authentication failed — check GITHUB_TOKEN".to_string(),
                })
            }
            Ok(r) => Ok(ProviderStatus::Degraded {
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
