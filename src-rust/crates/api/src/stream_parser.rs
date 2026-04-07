// stream_parser.rs — Trait and marker structs for parsing provider HTTP
// response bodies into unified `StreamEvent` streams.
//
// Concrete parsing logic lives in the provider-specific adapter crates and
// will be filled in during Phase 2A.

use async_trait::async_trait;
use claurst_core::provider_id::ProviderId;
use futures::Stream;
use std::pin::Pin;

use crate::provider_error::ProviderError;
use crate::provider_types::StreamEvent;

// ---------------------------------------------------------------------------
// StreamParser
// ---------------------------------------------------------------------------

/// Parses an HTTP response body into a stream of provider-agnostic
/// `StreamEvent`s.
///
/// Each provider adapter provides its own `StreamParser` implementation that
/// knows how to decode the wire format (SSE, JSON Lines, etc.) used by that
/// provider.
#[async_trait]
pub trait StreamParser: Send + Sync {
    /// Consume a `reqwest::Response` and produce a pinned stream of
    /// `StreamEvent`s.
    ///
    /// The returned stream yields `Ok(event)` for each successfully decoded
    /// event and `Err(ProviderError)` if parsing fails mid-stream.
    async fn parse(
        &self,
        response: reqwest::Response,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>;
}

// ---------------------------------------------------------------------------
// SseStreamParser  (marker — implementation deferred to Phase 2A)
// ---------------------------------------------------------------------------

/// Marker for SSE-based stream parsers used by Anthropic, Google Gemini, etc.
///
/// The actual parsing logic will be implemented in Phase 2A.
pub struct SseStreamParser;

impl SseStreamParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SseStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StreamParser for SseStreamParser {
    async fn parse(
        &self,
        _response: reqwest::Response,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        // Will be implemented in Phase 2A.
        Err(ProviderError::Other {
            provider: ProviderId::new("unknown"),
            message: "SseStreamParser::parse is not yet implemented".to_string(),
            status: None,
            body: None,
        })
    }
}

// ---------------------------------------------------------------------------
// JsonLinesStreamParser  (marker — implementation deferred to Phase 2A)
// ---------------------------------------------------------------------------

/// Marker for JSON Lines stream parsers used by OpenAI, Azure OpenAI, etc.
///
/// The actual parsing logic will be implemented in Phase 2A.
pub struct JsonLinesStreamParser;

impl JsonLinesStreamParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for JsonLinesStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StreamParser for JsonLinesStreamParser {
    async fn parse(
        &self,
        _response: reqwest::Response,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        // Will be implemented in Phase 2A.
        Err(ProviderError::Other {
            provider: ProviderId::new("unknown"),
            message: "JsonLinesStreamParser::parse is not yet implemented".to_string(),
            status: None,
            body: None,
        })
    }
}
