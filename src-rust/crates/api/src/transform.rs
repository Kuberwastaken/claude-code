// transform.rs — Message transformation trait for converting between the
// internal representation and provider-specific JSON wire formats.
//
// Each provider adapter implements `MessageTransformer` to handle the mapping
// in both directions (outbound request serialisation and inbound response
// deserialisation).

use crate::provider::ModelInfo;
use crate::provider_error::ProviderError;
use crate::provider_types::{ProviderRequest, ProviderResponse};

// ---------------------------------------------------------------------------
// MessageTransformer
// ---------------------------------------------------------------------------

/// Converts between the internal provider-agnostic types and the JSON wire
/// format expected by a specific LLM provider.
///
/// Implementors must be `Send + Sync` so they can be held in shared state
/// alongside the provider client.
pub trait MessageTransformer: Send + Sync {
    /// Serialize a `ProviderRequest` into the provider-specific JSON request
    /// body.
    ///
    /// The returned `Value` is typically passed directly to `reqwest` as the
    /// body of a `POST` request.
    fn to_provider(
        &self,
        request: &ProviderRequest,
        model: &ModelInfo,
    ) -> Result<serde_json::Value, ProviderError>;

    /// Deserialize a provider-specific JSON response body into a
    /// `ProviderResponse`.
    fn from_provider(
        &self,
        response: &serde_json::Value,
    ) -> Result<ProviderResponse, ProviderError>;

    /// Apply provider-specific prompt-caching markers to an already-serialized
    /// request JSON object (in-place).
    ///
    /// The default implementation is a no-op: most providers do not support
    /// prompt caching, so adapters that do need to override this method.
    fn apply_caching(&self, request_json: &mut serde_json::Value, _model: &ModelInfo) {
        // Default: no-op.
        let _ = request_json;
    }
}
