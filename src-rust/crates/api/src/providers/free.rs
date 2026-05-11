// providers/free.rs — Composite "Free" provider.
//
// Wraps two upstream OpenAI-compatible endpoints:
//   1. OpenCode Zen (primary)  — https://opencode.ai/zen/v1
//   2. OpenRouter free router (fallback) — openrouter/free model
//
// If a request to the primary provider fails before any data has been
// streamed (auth, rate limit, server error, request error), the same request
// is retried against the fallback provider transparently. Mid-stream failures
// are surfaced as-is — we don't replay partial conversations.
//
// Model translation: the caller sees a single synthetic model `free/auto`.
// When dispatched to Zen we translate to `minimax-m2.5-free`; for OpenRouter
// we translate to `openrouter/free`. Callers may also pass through a
// provider-prefixed id (e.g. `zen/big-pickle` or `openrouter/free`) to pin a
// specific upstream model.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use claurst_core::provider_id::{ModelId, ProviderId};
use futures::Stream;

use crate::provider::{LlmProvider, ModelInfo};
use crate::provider_error::ProviderError;
use crate::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StreamEvent,
    SystemPromptStyle,
};

use super::openai_compat_providers::{opencode_zen, openrouter};

// ---------------------------------------------------------------------------
// FreeProvider
// ---------------------------------------------------------------------------

/// Composite provider: OpenCode Zen primary, OpenRouter free as fallback.
pub struct FreeProvider {
    id: ProviderId,
    primary: Arc<dyn LlmProvider>,
    fallback: Arc<dyn LlmProvider>,
}

impl FreeProvider {
    /// Build with explicit upstream providers (used by the registry layer
    /// once both API keys have been resolved from auth/settings).
    pub fn new(primary: Arc<dyn LlmProvider>, fallback: Arc<dyn LlmProvider>) -> Self {
        Self {
            id: ProviderId::new(ProviderId::FREE),
            primary,
            fallback,
        }
    }

    /// Convenience constructor that reads `OPENCODE_API_KEY` and
    /// `OPENROUTER_API_KEY` from the environment.
    pub fn from_env() -> Self {
        let primary = Arc::new(opencode_zen()) as Arc<dyn LlmProvider>;
        let fallback = Arc::new(openrouter()) as Arc<dyn LlmProvider>;
        Self::new(primary, fallback)
    }

    /// Build with explicit API keys.
    pub fn with_keys(zen_key: String, openrouter_key: String) -> Self {
        let primary =
            Arc::new(opencode_zen().with_api_key(zen_key)) as Arc<dyn LlmProvider>;
        let fallback =
            Arc::new(openrouter().with_api_key(openrouter_key)) as Arc<dyn LlmProvider>;
        Self::new(primary, fallback)
    }

    fn translate_for_primary(model: &str) -> String {
        match model {
            // Synthetic auto pick → MiniMax M2.5 Free (largest free Zen model).
            "free" | "free/auto" | "auto" => "minimax-m2.5-free".to_string(),
            // Explicit `zen/<id>` form — strip the prefix.
            other => other
                .strip_prefix("zen/")
                .or_else(|| other.strip_prefix("opencode-zen/"))
                .map(|s| s.to_string())
                .unwrap_or_else(|| other.to_string()),
        }
    }

    fn translate_for_fallback(model: &str) -> String {
        match model {
            // Synthetic auto pick → OpenRouter free router.
            "free" | "free/auto" | "auto" => "openrouter/free".to_string(),
            // If the caller pinned a Zen-only model, the fallback still uses
            // the generic free router (it can't serve Zen IDs).
            m if m.starts_with("zen/") || m.starts_with("opencode-zen/") => {
                "openrouter/free".to_string()
            }
            other => other.to_string(),
        }
    }

    fn should_fallback(err: &ProviderError) -> bool {
        // We never fall back on user-fixable problems (invalid request,
        // content filter) — those would behave the same on the fallback.
        !matches!(
            err,
            ProviderError::InvalidRequest { .. } | ProviderError::ContentFiltered { .. }
        )
    }
}

#[async_trait]
impl LlmProvider for FreeProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        "Free (Zen \u{2192} OpenRouter)"
    }

    async fn create_message(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        let mut primary_req = request.clone();
        primary_req.model = Self::translate_for_primary(&request.model);

        match self.primary.create_message(primary_req).await {
            Ok(resp) => Ok(resp),
            Err(err) if Self::should_fallback(&err) => {
                tracing::warn!(
                    "FreeProvider: primary (Zen) failed: {} — falling back to OpenRouter",
                    err
                );
                let mut fb_req = request.clone();
                fb_req.model = Self::translate_for_fallback(&request.model);
                self.fallback.create_message(fb_req).await
            }
            Err(err) => Err(err),
        }
    }

    async fn create_message_stream(
        &self,
        request: ProviderRequest,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>,
        ProviderError,
    > {
        let mut primary_req = request.clone();
        primary_req.model = Self::translate_for_primary(&request.model);

        match self.primary.create_message_stream(primary_req).await {
            Ok(stream) => Ok(stream),
            Err(err) if Self::should_fallback(&err) => {
                tracing::warn!(
                    "FreeProvider: primary (Zen) stream failed: {} — falling back to OpenRouter",
                    err
                );
                let mut fb_req = request.clone();
                fb_req.model = Self::translate_for_fallback(&request.model);
                self.fallback.create_message_stream(fb_req).await
            }
            Err(err) => Err(err),
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        // Hardcoded curated list — both upstreams' `/v1/models` endpoints
        // surface many paid models we don't want to advertise here. The
        // synthetic `free/auto` entry is shown first as the default.
        let id_for = |id: &str| ModelId::new(id);
        let provider_id = self.id.clone();

        let mk = |id: &str, name: &str, ctx: u32| ModelInfo {
            id: id_for(id),
            provider_id: provider_id.clone(),
            name: name.to_string(),
            context_window: ctx,
            max_output_tokens: 8_192,
        };

        Ok(vec![
            mk(
                "free/auto",
                "Free \u{2014} Auto (Zen \u{2192} OpenRouter)",
                200_000,
            ),
            // OpenCode Zen free pool
            mk("zen/minimax-m2.5-free", "MiniMax M2.5 (Free, via Zen)", 200_000),
            mk("zen/big-pickle", "Big Pickle (Free, via Zen)", 128_000),
            mk("zen/ring-2.6-1t-free", "Ring 2.6 1T (Free, via Zen)", 128_000),
            mk(
                "zen/nemotron-3-super-free",
                "Nemotron 3 Super (Free, via Zen)",
                128_000,
            ),
            // OpenRouter free router
            mk(
                "openrouter/free",
                "OpenRouter Free Router (random pick)",
                200_000,
            ),
        ])
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        // Healthy as long as one upstream is reachable.
        let primary = self.primary.health_check().await;
        if matches!(primary, Ok(ProviderStatus::Healthy)) {
            return primary;
        }
        let fallback = self.fallback.health_check().await;
        if matches!(fallback, Ok(ProviderStatus::Healthy)) {
            return fallback;
        }
        primary.or(fallback)
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
            structured_output: false,
            system_prompt_style: SystemPromptStyle::SystemMessage,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_for_primary_auto() {
        assert_eq!(FreeProvider::translate_for_primary("free/auto"), "minimax-m2.5-free");
        assert_eq!(FreeProvider::translate_for_primary("free"), "minimax-m2.5-free");
        assert_eq!(FreeProvider::translate_for_primary("auto"), "minimax-m2.5-free");
    }

    #[test]
    fn translate_for_primary_zen_prefix_stripped() {
        assert_eq!(FreeProvider::translate_for_primary("zen/big-pickle"), "big-pickle");
        assert_eq!(
            FreeProvider::translate_for_primary("opencode-zen/ring-2.6-1t-free"),
            "ring-2.6-1t-free",
        );
    }

    #[test]
    fn translate_for_fallback_auto() {
        assert_eq!(FreeProvider::translate_for_fallback("free/auto"), "openrouter/free");
        assert_eq!(FreeProvider::translate_for_fallback("auto"), "openrouter/free");
    }

    #[test]
    fn translate_for_fallback_zen_models_route_to_free_router() {
        assert_eq!(
            FreeProvider::translate_for_fallback("zen/big-pickle"),
            "openrouter/free",
        );
    }

    #[test]
    fn should_fallback_on_auth_and_rate_limit() {
        let pid = ProviderId::new("opencode-zen");
        assert!(FreeProvider::should_fallback(&ProviderError::AuthFailed {
            provider: pid.clone(),
            message: "bad key".into(),
        }));
        assert!(FreeProvider::should_fallback(&ProviderError::RateLimited {
            provider: pid.clone(),
            retry_after: Some(60),
        }));
        assert!(FreeProvider::should_fallback(&ProviderError::ServerError {
            provider: pid.clone(),
            status: Some(500),
            message: "boom".into(),
            is_retryable: true,
        }));
    }

    #[test]
    fn should_not_fallback_on_invalid_request() {
        let pid = ProviderId::new("opencode-zen");
        assert!(!FreeProvider::should_fallback(&ProviderError::InvalidRequest {
            provider: pid.clone(),
            message: "bad request".into(),
        }));
        assert!(!FreeProvider::should_fallback(&ProviderError::ContentFiltered {
            provider: pid,
            message: "filtered".into(),
        }));
    }
}
