// provider.rs — Core trait definitions for the provider abstraction layer.
//
// Every LLM provider adapter must implement `LlmProvider`.  The trait is
// intentionally minimal: only what is needed to send messages, list models,
// and report capabilities.  Auth concerns live in `auth.rs`.

use async_trait::async_trait;
use claurst_core::provider_id::{ModelId, ProviderId};
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use crate::provider_error::ProviderError;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StreamEvent,
};

// ---------------------------------------------------------------------------
// ModelInfo
// ---------------------------------------------------------------------------

/// Static metadata about a model available through a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// The model's unique identifier (e.g. `"claude-opus-4-5"`).
    pub id: ModelId,

    /// The provider that hosts this model.
    pub provider_id: ProviderId,

    /// Human-readable display name (e.g. `"Claude Opus 4.5"`).
    pub name: String,

    /// Total context window size in tokens.
    pub context_window: u32,

    /// Maximum number of tokens the model can emit in a single response.
    pub max_output_tokens: u32,
}

// ---------------------------------------------------------------------------
// LlmProvider
// ---------------------------------------------------------------------------

/// The core trait every LLM provider adapter must implement.
///
/// Implementors are required to be `Send + Sync` so they can be held behind an
/// `Arc<dyn LlmProvider>` and shared across async tasks.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Unique machine-readable identifier, e.g. `"anthropic"`, `"openai"`.
    fn id(&self) -> &ProviderId;

    /// Human-readable display name, e.g. `"Anthropic"`, `"OpenAI"`.
    fn name(&self) -> &str;

    /// Send a message and receive a complete (non-streaming) response.
    async fn create_message(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError>;

    /// Send a message and receive a streaming response as a pinned `Stream` of
    /// provider-agnostic `StreamEvent`s.
    async fn create_message_stream(
        &self,
        request: ProviderRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>;

    /// Return the list of models available through this provider.
    ///
    /// Implementations may make a network call (e.g. `GET /v1/models`) or
    /// return a hard-coded list for providers that do not expose a models API.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;

    /// Check whether the provider is authenticated and reachable.
    ///
    /// Typically involves a lightweight API call (e.g. listing models or
    /// fetching account info).  Should not be called on the hot path.
    async fn health_check(&self) -> Result<ProviderStatus, ProviderError>;

    /// Return the static capabilities of this provider.
    ///
    /// This must not make a network call — it describes the provider's known
    /// feature set as compiled in.
    fn capabilities(&self) -> ProviderCapabilities;
}
