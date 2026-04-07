// transformers/anthropic.rs — Identity transformer for the Anthropic wire
// format (ProviderRequest → Anthropic JSON body and back).
//
// The Anthropic provider is the native/internal format for Claurst, so
// `to_provider` serialises the request fields directly to the Anthropic v1
// messages schema and `from_provider` parses the standard Anthropic response.

use crate::provider::ModelInfo;
use crate::provider_error::ProviderError;
use crate::provider_types::{ProviderRequest, ProviderResponse, StopReason};
use crate::providers::message_normalization::normalize_anthropic_messages;
use crate::transform::MessageTransformer;
use crate::types::{ApiMessage, ApiToolDefinition};
use claurst_core::provider_id::ProviderId;
use claurst_core::types::{ContentBlock, UsageInfo};

// ---------------------------------------------------------------------------
// AnthropicTransformer
// ---------------------------------------------------------------------------

/// Identity transformer: converts `ProviderRequest` to the Anthropic v1
/// messages JSON body, and parses the Anthropic JSON response into a
/// `ProviderResponse`.
///
/// This mirrors the logic in `AnthropicProvider::build_request` and the
/// `create_message` accumulation code, but works purely as a JSON↔type
/// mapping without owning an HTTP client.
pub struct AnthropicTransformer;

impl MessageTransformer for AnthropicTransformer {
    fn to_provider(
        &self,
        request: &ProviderRequest,
        _model: &ModelInfo,
    ) -> Result<serde_json::Value, ProviderError> {
        use serde_json::json;

        // Convert messages to API wire format.
        let normalized_messages = normalize_anthropic_messages(&request.messages);
        let api_messages: Vec<ApiMessage> =
            normalized_messages.iter().map(ApiMessage::from).collect();
        let messages_json =
            serde_json::to_value(&api_messages).map_err(|e| ProviderError::Other {
                provider: ProviderId::new(ProviderId::ANTHROPIC),
                message: format!("failed to serialise messages: {}", e),
                status: None,
                body: None,
            })?;

        // Convert tools to API wire format.
        let api_tools: Vec<ApiToolDefinition> =
            request.tools.iter().map(ApiToolDefinition::from).collect();

        let mut body = json!({
            "model": request.model,
            "messages": messages_json,
            "max_tokens": request.max_tokens,
        });

        // System prompt — Anthropic uses a top-level `system` field.
        if let Some(sys) = &request.system_prompt {
            use crate::provider_types::SystemPrompt;
            let sys_text = match sys {
                SystemPrompt::Text(t) => t.clone(),
                SystemPrompt::Blocks(blocks) => blocks
                    .iter()
                    .map(|b| b.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            body["system"] = serde_json::Value::String(sys_text);
        }

        // Tools.
        if !request.tools.is_empty() {
            let tools_json =
                serde_json::to_value(&api_tools).map_err(|e| ProviderError::Other {
                    provider: ProviderId::new(ProviderId::ANTHROPIC),
                    message: format!("failed to serialise tools: {}", e),
                    status: None,
                    body: None,
                })?;
            body["tools"] = tools_json;
        }

        // Optional sampling parameters.
        if let Some(t) = request.temperature {
            body["temperature"] = serde_json::Value::from(t);
        }
        if let Some(p) = request.top_p {
            body["top_p"] = serde_json::Value::from(p);
        }
        if let Some(k) = request.top_k {
            body["top_k"] = serde_json::Value::from(k);
        }
        if !request.stop_sequences.is_empty() {
            body["stop_sequences"] =
                serde_json::to_value(&request.stop_sequences).unwrap_or_default();
        }

        // Extended thinking.
        if let Some(tc) = &request.thinking {
            body["thinking"] = serde_json::to_value(tc).unwrap_or_default();
        }

        Ok(body)
    }

    fn from_provider(
        &self,
        response: &serde_json::Value,
    ) -> Result<ProviderResponse, ProviderError> {
        let anthropic_id = ProviderId::new(ProviderId::ANTHROPIC);

        let id = response
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let model = response
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let stop_reason = response
            .get("stop_reason")
            .and_then(|v| v.as_str())
            .map(map_stop_reason)
            .unwrap_or(StopReason::EndTurn);

        // Parse usage.
        let usage = {
            let u = response.get("usage");
            UsageInfo {
                input_tokens: u
                    .and_then(|v| v.get("input_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                output_tokens: u
                    .and_then(|v| v.get("output_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cache_creation_input_tokens: u
                    .and_then(|v| v.get("cache_creation_input_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cache_read_input_tokens: u
                    .and_then(|v| v.get("cache_read_input_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            }
        };

        // Parse content blocks.
        let content_arr = response
            .get("content")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ProviderError::Other {
                provider: anthropic_id.clone(),
                message: "missing 'content' array in response".to_string(),
                status: None,
                body: None,
            })?;

        let mut content: Vec<ContentBlock> = Vec::new();
        for block in content_arr {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    let text = block
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    content.push(ContentBlock::Text { text });
                }
                "tool_use" => {
                    let tool_id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = block
                        .get("input")
                        .cloned()
                        .unwrap_or(serde_json::Value::Object(Default::default()));
                    content.push(ContentBlock::ToolUse {
                        id: tool_id,
                        name,
                        input,
                    });
                }
                "thinking" => {
                    let thinking = block
                        .get("thinking")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let signature = block
                        .get("signature")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    content.push(ContentBlock::Thinking {
                        thinking,
                        signature,
                    });
                }
                // redacted_thinking, citations, etc. — skip silently for now.
                _ => {}
            }
        }

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
// Helpers
// ---------------------------------------------------------------------------

fn map_stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" => StopReason::EndTurn,
        "stop_sequence" => StopReason::StopSequence,
        "max_tokens" => StopReason::MaxTokens,
        "tool_use" => StopReason::ToolUse,
        other => StopReason::Other(other.to_string()),
    }
}
