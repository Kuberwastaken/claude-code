//! Codex schema adapter — translates between Anthropic Messages API and OpenAI API formats.
//!
//! When using OpenAI Codex provider, requests are translated from Anthropic's
//! CreateMessageRequest format to OpenAI's ChatCompletion API format, and responses
//! are translated back to Anthropic's CreateMessageResponse format.

use super::types::{CreateMessageRequest, CreateMessageResponse, SystemPrompt};
use claurst_core::types::UsageInfo;
use serde_json::{json, Value};

/// OpenAI Codex API endpoint for responses
pub const CODEX_RESPONSES_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";

/// Convert an Anthropic CreateMessageRequest to OpenAI ChatCompletion request format.
pub fn anthropic_to_openai_request(request: &CreateMessageRequest) -> Value {
    // Convert Anthropic messages to OpenAI format
    let messages: Vec<Value> = request
        .messages
        .iter()
        .map(|msg| {
            json!({
                "role": msg.role.to_lowercase(),
                "content": msg.content,
            })
        })
        .collect();

    // Build system message from prompt if present
    let mut openai_messages = vec![];

    if let Some(system) = &request.system {
        let system_text = match system {
            SystemPrompt::Text(text) => text.clone(),
            SystemPrompt::Blocks(blocks) => blocks
                .iter()
                .map(|b| b.text.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        };

        openai_messages.push(json!({
            "role": "system",
            "content": system_text,
        }));
    }

    // Add regular messages
    openai_messages.extend(messages);

    // Build OpenAI request
    let mut openai_req = json!({
        "model": request.model,
        "messages": openai_messages,
        "max_tokens": request.max_tokens,
        "stream": request.stream,
    });

    // Add optional parameters
    if let Some(temperature) = request.temperature {
        openai_req["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        openai_req["top_p"] = json!(top_p);
    }

    // Note: OpenAI Codex doesn't support thinking blocks or tools in the same way
    // Skip those fields for now — they would need special handling

    openai_req
}

/// Convert an OpenAI ChatCompletion response to Anthropic format fields.
/// Returns (content_text, finish_reason, input_tokens, output_tokens)
pub fn parse_openai_response(response: &Value) -> (String, String, u64, u64) {
    let content = response
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    let finish_reason = response
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str())
        .unwrap_or("stop");

    // Map OpenAI finish_reason to Anthropic stop_reason
    let stop_reason = match finish_reason {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "content_filter" => "end_turn",
        "function_call" => "tool_use",
        _ => "end_turn",
    }
    .to_string();

    // Extract usage info
    let input_tokens = response
        .get("usage")
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);

    let output_tokens = response
        .get("usage")
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);

    (content, stop_reason, input_tokens, output_tokens)
}

/// Build an Anthropic CreateMessageResponse from parsed OpenAI data.
pub fn build_anthropic_response(
    content: &str,
    stop_reason: &str,
    input_tokens: u64,
    output_tokens: u64,
    model: &str,
) -> CreateMessageResponse {
    // Generate a simple message ID
    let id = format!(
        "msg_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| format!("{:x}", d.as_nanos()))
            .unwrap_or_else(|_| "unknown".to_string())
    );

    CreateMessageResponse {
        id,
        response_type: "message".to_string(),
        role: "assistant".to_string(),
        content: vec![json!({
            "type": "text",
            "text": content,
        })],
        model: model.to_string(),
        stop_reason: Some(stop_reason.to_string()),
        stop_sequence: None,
        usage: UsageInfo {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ApiMessage, SystemPrompt};

    #[test]
    fn test_anthropic_to_openai_request_basic() {
        let request = CreateMessageRequest {
            model: "gpt-5.2-codex".to_string(),
            max_tokens: 1024,
            messages: vec![ApiMessage {
                role: "user".to_string(),
                content: json!("Hello"),
            }],
            system: Some(SystemPrompt::Text("You are helpful".to_string())),
            tools: None,
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: false,
            thinking: None,
        };

        let openai_req = anthropic_to_openai_request(&request);

        // Verify structure
        assert_eq!(openai_req["model"], "gpt-5.2-codex");
        assert_eq!(openai_req["max_tokens"], 1024);
        assert_eq!(openai_req["temperature"], 0.7);
        assert!(openai_req["messages"].is_array());

        let messages = openai_req["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2); // system + user
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn test_parse_openai_response_basic() {
        let openai_resp = json!({
            "choices": [{
                "message": {
                    "content": "Hello, world!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        let (content, stop_reason, input_tokens, output_tokens) =
            parse_openai_response(&openai_resp);

        assert_eq!(content, "Hello, world!");
        assert_eq!(stop_reason, "end_turn");
        assert_eq!(input_tokens, 10);
        assert_eq!(output_tokens, 5);
    }

    #[test]
    fn test_build_anthropic_response() {
        let response =
            build_anthropic_response("Test response", "end_turn", 100, 50, "gpt-5.2-codex");

        assert_eq!(response.response_type, "message");
        assert_eq!(response.role, "assistant");
        assert_eq!(response.model, "gpt-5.2-codex");
        assert_eq!(response.stop_reason, Some("end_turn".to_string()));
        assert_eq!(response.usage.input_tokens, 100);
        assert_eq!(response.usage.output_tokens, 50);
    }
}
