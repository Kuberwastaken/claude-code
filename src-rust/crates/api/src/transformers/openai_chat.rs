// transformers/openai_chat.rs — MessageTransformer that produces the OpenAI
// Chat Completions JSON wire format.
//
// Delegates message/tool conversion to the public helpers exposed by
// `OpenAiProvider` so that all format logic lives in one place.

use crate::provider::ModelInfo;
use crate::provider_error::ProviderError;
use crate::provider_types::{ProviderRequest, ProviderResponse};
use crate::providers::OpenAiProvider;
use crate::transform::MessageTransformer;
use claurst_core::provider_id::ProviderId;

// ---------------------------------------------------------------------------
// OpenAiChatTransformer
// ---------------------------------------------------------------------------

/// Converts `ProviderRequest` to an OpenAI Chat Completions JSON body and
/// parses the non-streaming response JSON into a `ProviderResponse`.
///
/// Delegates the heavy lifting to `OpenAiProvider::to_openai_messages_pub`,
/// `OpenAiProvider::to_openai_tools_pub`, and
/// `OpenAiProvider::parse_non_streaming_response_pub`.
pub struct OpenAiChatTransformer;

impl MessageTransformer for OpenAiChatTransformer {
    fn to_provider(
        &self,
        request: &ProviderRequest,
        _model: &ModelInfo,
    ) -> Result<serde_json::Value, ProviderError> {
        use serde_json::json;

        let messages = OpenAiProvider::to_openai_messages_pub(
            &request.messages,
            request.system_prompt.as_ref(),
        );
        let tools = OpenAiProvider::to_openai_tools_pub(&request.tools);

        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
            "stream": false,
        });

        if !request.tools.is_empty() {
            body["tools"] = serde_json::Value::Array(tools);
        }
        if let Some(t) = request.temperature {
            body["temperature"] = serde_json::Value::from(t);
        }
        if let Some(p) = request.top_p {
            body["top_p"] = serde_json::Value::from(p);
        }
        if !request.stop_sequences.is_empty() {
            body["stop"] = serde_json::to_value(&request.stop_sequences).unwrap_or_default();
        }

        Ok(body)
    }

    fn from_provider(
        &self,
        response: &serde_json::Value,
    ) -> Result<ProviderResponse, ProviderError> {
        let openai_id = ProviderId::new(ProviderId::OPENAI);
        OpenAiProvider::parse_non_streaming_response_pub(response, &openai_id)
    }
}
