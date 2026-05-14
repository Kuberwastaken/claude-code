// providers/minimax.rs — MinimaxProvider: Anthropic-compatible provider for MiniMax.
// Minimax requires API key in Authorization header without Bearer prefix.

use std::pin::Pin;

use async_stream::stream;
use async_trait::async_trait;
use claurst_core::provider_id::{ModelId, ProviderId};
use claurst_core::types::{ContentBlock, UsageInfo};
use futures::Stream;
use reqwest::Client;
use serde_json::Value;

use crate::provider::{LlmProvider, ModelInfo};
use crate::provider_error::ProviderError;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StopReason,
    StreamEvent, SystemPromptStyle,
};
use crate::types::{ApiMessage, ApiToolDefinition, CreateMessageRequest};

use super::message_normalization::normalize_anthropic_messages;

pub struct MinimaxProvider {
    http_client: Client,
    api_key: String,
    api_base: String,
    id: ProviderId,
}

impl MinimaxProvider {
    pub fn new(api_key: String) -> Self {
        let api_base = std::env::var("MINIMAX_BASE_URL")
            .unwrap_or_else(|_| "https://api.minimax.io/anthropic".to_string());

        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .expect("MinimaxProvider: failed to build HTTP client");

        Self {
            http_client,
            api_key,
            api_base,
            id: ProviderId::new(ProviderId::MINIMAX),
        }
    }

    fn build_request(request: &ProviderRequest) -> CreateMessageRequest {
        let normalized_messages = normalize_anthropic_messages(&request.messages);
        let api_messages: Vec<ApiMessage> = normalized_messages
            .iter()
            .map(ApiMessage::from)
            .collect();

        let api_tools: Option<Vec<ApiToolDefinition>> = if request.tools.is_empty() {
            None
        } else {
            Some(request.tools.iter().map(ApiToolDefinition::from).collect())
        };

        let system = request.system_prompt.clone();

        let mut builder = CreateMessageRequest::builder(&request.model, request.max_tokens)
            .messages(api_messages);

        if let Some(sys) = system {
            builder = builder.system(sys);
        }
        if let Some(tools) = api_tools {
            builder = builder.tools(tools);
        }
        if let Some(t) = request.temperature {
            builder = builder.temperature(t as f32);
        }
        if let Some(p) = request.top_p {
            builder = builder.top_p(p as f32);
        }
        if let Some(k) = request.top_k {
            builder = builder.top_k(k);
        }
        if !request.stop_sequences.is_empty() {
            builder = builder.stop_sequences(request.stop_sequences.clone());
        }
        if let Some(tc) = request.thinking.clone() {
            builder = builder.thinking(tc);
        }

        builder.build()
    }

    fn map_stop_reason(s: &str) -> StopReason {
        match s {
            "end_turn" => StopReason::EndTurn,
            "stop_sequence" => StopReason::StopSequence,
            "max_tokens" => StopReason::MaxTokens,
            "tool_use" => StopReason::ToolUse,
            other => StopReason::Other(other.to_string()),
        }
    }

    fn map_anthropic_event(value: Value) -> Option<StreamEvent> {
        let event_type = value.get("type")?.as_str()?;

        match event_type {
            "message_start" => {
                let id = value.get("message")?.get("id")?.as_str()?.to_string();
                let model = value.get("message")?.get("model")?.as_str()?.to_string();
                let usage = UsageInfo {
                    input_tokens: value.get("message")?.get("usage")?.get("input_tokens")?.as_u64()?,
                    output_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                };
                Some(StreamEvent::MessageStart { id, model, usage })
            }
            "content_block_start" => {
                let index = value.get("index")?.as_u64()? as usize;
                let content_type = value.get("content_block")?.get("type")?.as_str()?;

                let content_block = match content_type {
                    "text" => ContentBlock::Text {
                        text: String::new(),
                    },
                    "tool_use" => {
                        let id = value.get("content_block")?.get("id")?.as_str()?.to_string();
                        let name = value.get("content_block")?.get("name")?.as_str()?.to_string();
                        ContentBlock::ToolUse {
                            id,
                            name,
                            input: serde_json::Value::Object(Default::default()),
                        }
                    }
                    _ => return None,
                };

                Some(StreamEvent::ContentBlockStart { index, content_block })
            }
            "content_block_delta" => {
                let index = value.get("index")?.as_u64()? as usize;
                let delta_type = value.get("delta")?.get("type")?.as_str()?;

                match delta_type {
                    "text_delta" => {
                        let text = value.get("delta")?.get("text")?.as_str()?.to_string();
                        Some(StreamEvent::TextDelta { index, text })
                    }
                    "thinking_delta" => {
                        let thinking = value.get("delta")?.get("thinking")?.as_str()?.to_string();
                        Some(StreamEvent::ThinkingDelta { index, thinking })
                    }
                    "signature_delta" => {
                        let signature = value.get("delta")?.get("signature")?.as_str()?.to_string();
                        Some(StreamEvent::SignatureDelta { index, signature })
                    }
                    "input_json_delta" => {
                        let partial_json = value.get("delta")?.get("partial_json")?.as_str()?.to_string();
                        Some(StreamEvent::InputJsonDelta { index, partial_json })
                    }
                    _ => None,
                }
            }
            "content_block_stop" => {
                let index = value.get("index")?.as_u64()? as usize;
                Some(StreamEvent::ContentBlockStop { index })
            }
            "message_delta" => {
                let stop_reason = value.get("delta")?
                    .get("stop_reason")?
                    .as_str()
                    .map(Self::map_stop_reason);

                let usage = value.get("delta")?.get("usage")
                    .and_then(|u| {
                        Some(UsageInfo {
                            input_tokens: u.get("input_tokens")?.as_u64()?,
                            output_tokens: u.get("output_tokens")?.as_u64()?,
                            cache_creation_input_tokens: u.get("cache_creation_input_tokens")?.as_u64().unwrap_or(0),
                            cache_read_input_tokens: u.get("cache_read_input_tokens")?.as_u64().unwrap_or(0),
                        })
                    });

                Some(StreamEvent::MessageDelta {
                    stop_reason,
                    usage,
                })
            }
            "message_stop" => Some(StreamEvent::MessageStop),
            "error" => {
                let error_type = value.get("error")?.get("type")?.as_str()?.to_string();
                let message = value.get("error")?.get("message")?.as_str()?.to_string();
                Some(StreamEvent::Error { error_type, message })
            }
            "ping" => None,
            _ => None,
        }
    }
}

#[async_trait]
impl LlmProvider for MinimaxProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        "MiniMax"
    }

    async fn create_message(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        let mut stream = self.create_message_stream(request).await?;

        let mut id = String::from("unknown");
        let mut model = String::new();
        let mut text_parts: Vec<(usize, String)> = Vec::new();
        let mut content_blocks: Vec<ContentBlock> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = UsageInfo::default();

        let mut tool_buffers: std::collections::HashMap<usize, (String, String, String)> =
            std::collections::HashMap::new();

        use futures::StreamExt;
        while let Some(result) = stream.next().await {
            match result {
                Err(e) => return Err(e),
                Ok(evt) => match evt {
                    StreamEvent::MessageStart {
                        id: msg_id,
                        model: msg_model,
                        usage: msg_usage,
                    } => {
                        id = msg_id;
                        model = msg_model;
                        usage = msg_usage;
                    }
                    StreamEvent::ContentBlockStart {
                        index,
                        content_block,
                    } => match content_block {
                        ContentBlock::Text { text } => {
                            text_parts.push((index, text));
                        }
                        ContentBlock::ToolUse {
                            id: tool_id,
                            name,
                            input: _,
                        } => {
                            tool_buffers.insert(index, (tool_id, name, String::new()));
                        }
                        other => {
                            content_blocks.push(other);
                        }
                    },
                    StreamEvent::TextDelta { index, text } => {
                        if let Some(entry) = text_parts.iter_mut().find(|(i, _)| *i == index) {
                            entry.1.push_str(&text);
                        }
                    }
                    StreamEvent::InputJsonDelta {
                        index,
                        partial_json,
                    } => {
                        if let Some((_, _, buf)) = tool_buffers.get_mut(&index) {
                            buf.push_str(&partial_json);
                        }
                    }
                    StreamEvent::ContentBlockStop { index } => {
                        if let Some((tool_id, name, json_buf)) = tool_buffers.remove(&index) {
                            let input = serde_json::from_str(&json_buf)
                                .unwrap_or(serde_json::Value::Object(Default::default()));
                            content_blocks.push(ContentBlock::ToolUse {
                                id: tool_id,
                                name,
                                input,
                            });
                        }
                    }
                    StreamEvent::MessageDelta {
                        stop_reason: sr,
                        usage: delta_usage,
                    } => {
                        if let Some(r) = sr {
                            stop_reason = r;
                        }
                        if let Some(u) = delta_usage {
                            usage.output_tokens += u.output_tokens;
                        }
                    }
                    StreamEvent::MessageStop => break,
                    StreamEvent::Error { error_type, message } => {
                        return Err(ProviderError::StreamError {
                            provider: self.id.clone(),
                            message: format!("[{}] {}", error_type, message),
                            partial_response: None,
                        });
                    }
                    _ => {}
                },
            }
        }

        text_parts.sort_by_key(|(i, _)| *i);
        let mut all_blocks: Vec<(usize, ContentBlock)> = text_parts
            .into_iter()
            .map(|(i, text)| (i, ContentBlock::Text { text }))
            .collect();
        for block in content_blocks {
            all_blocks.push((usize::MAX, block));
        }
        let final_content: Vec<ContentBlock> = all_blocks.into_iter().map(|(_, b)| b).collect();

        Ok(ProviderResponse {
            id,
            content: final_content,
            stop_reason,
            usage,
            model,
        })
    }

    async fn create_message_stream(
        &self,
        request: ProviderRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        let api_request = Self::build_request(&request);

        let body = serde_json::to_value(&api_request)
            .map_err(|e| ProviderError::Other {
                provider: self.id.clone(),
                message: format!("Failed to serialize request: {}", e),
                status: None,
                body: None,
            })?;

        let url = format!("{}/v1/messages", self.api_base);
        let api_key = self.api_key.clone();
        let http_client = self.http_client.clone();
        let provider_id = self.id.clone();

        let resp = http_client
            .post(&url)
            .header("Authorization", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Other {
                provider: provider_id.clone(),
                message: format!("HTTP request failed: {}", e),
                status: None,
                body: None,
            })?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let error_body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Other {
                provider: provider_id.clone(),
                message: format!("API error: {}", error_body),
                status: Some(status),
                body: Some(error_body),
            });
        }

        let provider_id_inner = provider_id.clone();
        let s = stream! {
            let byte_stream = resp.bytes_stream();
            let mut leftover = String::new();

            use futures::StreamExt;
            let mut stream = std::pin::pin!(byte_stream);

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
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

                            let data = if let Some(rest) = line.strip_prefix("data:") {
                                rest.trim()
                            } else {
                                line
                            };

                            match serde_json::from_str::<Value>(data) {
                                Ok(value) => {
                                    if let Some(stream_evt) = Self::map_anthropic_event(value) {
                                        yield Ok(stream_evt);
                                    }
                                }
                                Err(e) => {
                                    yield Err(ProviderError::Other {
                                        provider: provider_id_inner.clone(),
                                        message: format!("Failed to parse event: {}", e),
                                        status: None,
                                        body: None,
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(ProviderError::Other {
                            provider: provider_id_inner.clone(),
                            message: format!("Stream error: {}", e),
                            status: None,
                            body: None,
                        });
                    }
                }
            }
        };

        Ok(Box::pin(s))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let minimax_id = ProviderId::new(ProviderId::MINIMAX);
        Ok(vec![
            ModelInfo {
                id: ModelId::new("MiniMax-M2.7"),
                provider_id: minimax_id.clone(),
                name: "MiniMax-M2.7".to_string(),
                context_window: 128_000,
                max_output_tokens: 8192,
            },
        ])
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
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
            structured_output: true,
            system_prompt_style: SystemPromptStyle::TopLevel,
        }
    }
}
