//! Codex schema adapter — translates between Anthropic Messages API and OpenAI Responses formats.
//!
//! When using OpenAI Codex provider, requests are translated from Anthropic's
//! CreateMessageRequest format to the OpenAI Responses API shape used by the
//! ChatGPT Codex backend, and responses are translated back to Anthropic's
//! CreateMessageResponse format.

use super::types::{CreateMessageRequest, CreateMessageResponse, SystemPrompt};
use claurst_core::types::UsageInfo;
use serde_json::{json, Value};

/// OpenAI Codex API endpoint for responses
pub const CODEX_RESPONSES_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";

fn system_text(system: &Option<SystemPrompt>) -> String {
    match system {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|b| b.text.clone())
            .collect::<Vec<_>>()
            .join("\n"),
        None => String::new(),
    }
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(|text| text.as_str())
                    .map(|text| text.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => content.as_str().unwrap_or("").to_string(),
    }
}

/// Convert an Anthropic CreateMessageRequest to OpenAI Responses request format.
pub fn anthropic_to_openai_request(request: &CreateMessageRequest) -> Value {
    let input: Vec<Value> = request
        .messages
        .iter()
        .map(|msg| {
            let role = msg.role.to_lowercase();
            let text = content_text(&msg.content);

            if role == "assistant" {
                json!({
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": text,
                    }],
                })
            } else {
                json!({
                    "role": role,
                    "content": [{
                        "type": "input_text",
                        "text": text,
                    }],
                })
            }
        })
        .collect();

    let mut openai_req = json!({
        "model": request.model,
        "instructions": system_text(&request.system),
        "input": input,
        "store": false,
        "stream": true,
    });

    if let Some(temperature) = request.temperature {
        openai_req["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        openai_req["top_p"] = json!(top_p);
    }

    openai_req
}

fn extract_output_text(response: &Value) -> Option<String> {
    if let Some(text) = response.get("output_text").and_then(|text| text.as_str()) {
        return Some(text.to_string());
    }

    response
        .get("output")
        .and_then(|output| output.as_array())
        .map(|items| {
            items
                .iter()
                .flat_map(|item| {
                    if item.get("type").and_then(|v| v.as_str()) == Some("message") {
                        item.get("content")
                            .and_then(|content| content.as_array())
                            .into_iter()
                            .flatten()
                            .filter_map(|part| {
                                part.get("text")
                                    .and_then(|text| text.as_str())
                                    .map(|text| text.to_string())
                            })
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    }
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .filter(|text| !text.is_empty())
}

/// Convert an OpenAI response to Anthropic format fields.
/// Returns (content_text, finish_reason, input_tokens, output_tokens)
pub fn parse_openai_response(response: &Value) -> (String, String, u64, u64) {
    let content = extract_output_text(response).unwrap_or_else(|| {
        response
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string()
    });

    let finish_reason = response
        .get("incomplete_details")
        .and_then(|details| details.get("reason"))
        .and_then(|reason| reason.as_str())
        .or_else(|| {
            response
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("finish_reason"))
                .and_then(|f| f.as_str())
        })
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
        .and_then(|u| u.get("input_tokens").or_else(|| u.get("prompt_tokens")))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);

    let output_tokens = response
        .get("usage")
        .and_then(|u| {
            u.get("output_tokens")
                .or_else(|| u.get("completion_tokens"))
        })
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
        assert_eq!(openai_req["instructions"], "You are helpful");
        assert!(openai_req.get("max_completion_tokens").is_none());
        assert!(openai_req.get("max_output_tokens").is_none());
        assert!(openai_req.get("max_tokens").is_none());
        assert_eq!(openai_req["store"], false);
        assert_eq!(openai_req["stream"], true);
        let temp = openai_req["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 1e-6);
        assert!(openai_req["input"].is_array());

        let input = openai_req["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
    }

    #[test]
    fn test_parse_openai_response_basic() {
        let openai_resp = json!({
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": "Hello, world!"
                }]
            }],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
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
