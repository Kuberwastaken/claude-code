// providers/anthropic.rs — AnthropicProvider: wraps AnthropicClient in the
// unified LlmProvider trait.
//
// Phase 2A: create_message and create_message_stream are fully implemented by
// mapping ProviderRequest → CreateMessageRequest and mapping
// AnthropicStreamEvent → provider_types::StreamEvent.

use std::pin::Pin;
use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use claurst_core::provider_id::{ModelId, ProviderId};
use claurst_core::types::{ContentBlock, UsageInfo};
use futures::Stream;

use crate::client::{AnthropicClient, ClientConfig};
use crate::provider::{LlmProvider, ModelInfo};
use crate::provider_error::ProviderError;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StopReason,
    StreamEvent, SystemPromptStyle,
};
use crate::streaming::{AnthropicStreamEvent, ContentDelta, NullStreamHandler};
use crate::types::{ApiMessage, ApiToolDefinition, CreateMessageRequest};

use super::message_normalization::normalize_anthropic_messages;

// ---------------------------------------------------------------------------
// AnthropicProvider
// ---------------------------------------------------------------------------

/// Wraps [`AnthropicClient`] so it can be held in a [`ProviderRegistry`] behind
/// `Arc<dyn LlmProvider>`.
pub struct AnthropicProvider {
    client: Arc<AnthropicClient>,
    id: ProviderId,
}

impl AnthropicProvider {
    /// Wrap an already-constructed (and Arc-wrapped) [`AnthropicClient`].
    pub fn new(client: Arc<AnthropicClient>) -> Self {
        Self {
            client,
            id: ProviderId::new(ProviderId::ANTHROPIC),
        }
    }

    /// Construct directly from a [`ClientConfig`], creating the inner client.
    pub fn from_config(config: ClientConfig) -> Self {
        let client = AnthropicClient::new(config)
            .expect("AnthropicProvider::from_config: failed to create AnthropicClient");
        Self {
            client: Arc::new(client),
            id: ProviderId::new(ProviderId::ANTHROPIC),
        }
    }

    /// Build a [`CreateMessageRequest`] from a [`ProviderRequest`].
    fn build_request(request: &ProviderRequest) -> CreateMessageRequest {
        let normalized_messages = normalize_anthropic_messages(&request.messages);
        let api_messages: Vec<ApiMessage> =
            normalized_messages.iter().map(ApiMessage::from).collect();

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

    /// Map a string stop_reason from Anthropic wire format to [`StopReason`].
    fn map_stop_reason(s: &str) -> StopReason {
        match s {
            "end_turn" => StopReason::EndTurn,
            "stop_sequence" => StopReason::StopSequence,
            "max_tokens" => StopReason::MaxTokens,
            "tool_use" => StopReason::ToolUse,
            other => StopReason::Other(other.to_string()),
        }
    }

    /// Map an [`AnthropicStreamEvent`] to the provider-agnostic [`StreamEvent`].
    fn map_stream_event(evt: AnthropicStreamEvent) -> Option<StreamEvent> {
        match evt {
            AnthropicStreamEvent::MessageStart { id, model, usage } => {
                Some(StreamEvent::MessageStart { id, model, usage })
            }
            AnthropicStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => Some(StreamEvent::ContentBlockStart {
                index,
                content_block,
            }),
            AnthropicStreamEvent::ContentBlockDelta { index, delta } => match delta {
                ContentDelta::TextDelta { text } => Some(StreamEvent::TextDelta { index, text }),
                ContentDelta::ThinkingDelta { thinking } => {
                    Some(StreamEvent::ThinkingDelta { index, thinking })
                }
                ContentDelta::SignatureDelta { signature } => {
                    Some(StreamEvent::SignatureDelta { index, signature })
                }
                ContentDelta::InputJsonDelta { partial_json } => {
                    Some(StreamEvent::InputJsonDelta {
                        index,
                        partial_json,
                    })
                }
            },
            AnthropicStreamEvent::ContentBlockStop { index } => {
                Some(StreamEvent::ContentBlockStop { index })
            }
            AnthropicStreamEvent::MessageDelta { stop_reason, usage } => {
                let mapped_stop = stop_reason.as_deref().map(Self::map_stop_reason);
                Some(StreamEvent::MessageDelta {
                    stop_reason: mapped_stop,
                    usage,
                })
            }
            AnthropicStreamEvent::MessageStop => Some(StreamEvent::MessageStop),
            AnthropicStreamEvent::Error {
                error_type,
                message,
            } => Some(StreamEvent::Error {
                error_type,
                message,
            }),
            AnthropicStreamEvent::Ping => None,
        }
    }
}

// ---------------------------------------------------------------------------
// LlmProvider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        "Anthropic"
    }

    async fn create_message(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        // Collect stream events to build a complete response.
        let mut stream = self.create_message_stream(request).await?;

        let mut id = String::from("unknown");
        let mut model = String::new();
        let mut text_parts: Vec<(usize, String)> = Vec::new();
        let mut content_blocks: Vec<ContentBlock> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = UsageInfo::default();

        // We need to track tool use blocks being assembled from partial JSON.
        // Use a simple per-index buffer.
        let mut tool_buffers: std::collections::HashMap<usize, (String, String, String)> =
            std::collections::HashMap::new(); // index -> (id, name, json_buf)

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
                        // Finalize any tool use block at this index.
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
                    StreamEvent::Error {
                        error_type,
                        message,
                    } => {
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

        // Assemble text blocks into content, sorted by index.
        text_parts.sort_by_key(|(i, _)| *i);
        let mut all_blocks: Vec<(usize, ContentBlock)> = text_parts
            .into_iter()
            .map(|(i, text)| (i, ContentBlock::Text { text }))
            .collect();
        // We don't have indices for the non-text blocks — just append them.
        // In practice content blocks are already in-order from the stream.
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
        let handler = Arc::new(NullStreamHandler);

        let provider_id = self.id.clone();

        let mut rx = self
            .client
            .create_message_stream(api_request, handler)
            .await
            .map_err(|e| ProviderError::Other {
                provider: provider_id.clone(),
                message: e.to_string(),
                status: None,
                body: None,
            })?;

        let s = stream! {
            while let Some(anthropic_evt) = rx.recv().await {
                if let Some(unified_evt) = AnthropicProvider::map_stream_event(anthropic_evt) {
                    yield Ok(unified_evt);
                }
            }
        };

        Ok(Box::pin(s))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let anthropic_id = ProviderId::new(ProviderId::ANTHROPIC);
        Ok(vec![
            ModelInfo {
                id: ModelId::new("claude-opus-4-6"),
                provider_id: anthropic_id.clone(),
                name: "Claude Opus 4.6".to_string(),
                context_window: 200_000,
                max_output_tokens: 32_000,
            },
            ModelInfo {
                id: ModelId::new("claude-sonnet-4-6"),
                provider_id: anthropic_id.clone(),
                name: "Claude Sonnet 4.6".to_string(),
                context_window: 200_000,
                max_output_tokens: 16_000,
            },
            ModelInfo {
                id: ModelId::new("claude-haiku-4-5-20251001"),
                provider_id: anthropic_id.clone(),
                name: "Claude Haiku 4.5".to_string(),
                context_window: 200_000,
                max_output_tokens: 8_096,
            },
        ])
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        // Client was successfully constructed with a non-empty API key.
        Ok(ProviderStatus::Healthy)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            thinking: true,
            image_input: true,
            pdf_input: true,
            audio_input: false,
            video_input: false,
            caching: true,
            structured_output: true,
            system_prompt_style: SystemPromptStyle::TopLevel,
        }
    }
}
