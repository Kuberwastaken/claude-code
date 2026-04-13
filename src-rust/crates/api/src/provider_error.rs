// provider_error.rs — Unified error type for all provider adapters.
//
// Every provider implementation maps its own error representation onto
// `ProviderError` so that the application-layer code can handle errors
// generically without knowing which provider was involved.

use claurst_core::error::ClaudeError;
use claurst_core::provider_id::ProviderId;
use std::fmt;

// ---------------------------------------------------------------------------
// ProviderError
// ---------------------------------------------------------------------------

/// A structured error produced by any provider adapter.
#[derive(Debug, Clone)]
pub enum ProviderError {
    /// The request exceeded the model's context window.
    ContextOverflow {
        provider: ProviderId,
        message: String,
        /// The provider's advertised context limit in tokens, if known.
        max_tokens: Option<u64>,
    },

    /// The provider returned HTTP 429 or an equivalent rate-limit signal.
    RateLimited {
        provider: ProviderId,
        /// How long to wait before retrying, in seconds (if provided).
        retry_after: Option<u64>,
    },

    /// The API key or credentials were rejected by the provider.
    AuthFailed {
        provider: ProviderId,
        message: String,
    },

    /// The account's usage quota has been exhausted.
    QuotaExceeded {
        provider: ProviderId,
        message: String,
    },

    /// The requested model does not exist or is not accessible.
    ModelNotFound {
        provider: ProviderId,
        model: String,
        /// Alternative model IDs the caller might try instead.
        suggestions: Vec<String>,
    },

    /// The provider returned a 5xx or equivalent server-side error.
    ServerError {
        provider: ProviderId,
        /// HTTP status code, if applicable.
        status: Option<u16>,
        message: String,
        /// Whether the caller should retry this request.
        is_retryable: bool,
    },

    /// The request itself was malformed or contained invalid parameters.
    InvalidRequest {
        provider: ProviderId,
        message: String,
    },

    /// The response was blocked by the provider's content-safety system.
    ContentFiltered {
        provider: ProviderId,
        message: String,
    },

    /// An error occurred during streaming after the response had already begun.
    StreamError {
        provider: ProviderId,
        message: String,
        /// Any content blocks that had been received before the error, if any.
        partial_response: Option<String>,
    },

    /// A catch-all variant for errors that do not fit any of the above.
    Other {
        provider: ProviderId,
        message: String,
        /// HTTP status code, if applicable.
        status: Option<u16>,
        /// Raw response body, if available.
        body: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// impl ProviderError
// ---------------------------------------------------------------------------

impl ProviderError {
    /// Returns `true` if the caller should retry the request after a delay.
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::RateLimited { .. } => true,
            ProviderError::ServerError { is_retryable, .. } => *is_retryable,
            ProviderError::StreamError { .. } => true,
            _ => false,
        }
    }

    /// Returns the `ProviderId` of the provider that produced this error.
    pub fn provider_id(&self) -> &ProviderId {
        match self {
            ProviderError::ContextOverflow { provider, .. } => provider,
            ProviderError::RateLimited { provider, .. } => provider,
            ProviderError::AuthFailed { provider, .. } => provider,
            ProviderError::QuotaExceeded { provider, .. } => provider,
            ProviderError::ModelNotFound { provider, .. } => provider,
            ProviderError::ServerError { provider, .. } => provider,
            ProviderError::InvalidRequest { provider, .. } => provider,
            ProviderError::ContentFiltered { provider, .. } => provider,
            ProviderError::StreamError { provider, .. } => provider,
            ProviderError::Other { provider, .. } => provider,
        }
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::ContextOverflow {
                provider,
                message,
                max_tokens,
            } => {
                write!(f, "[{}] Context overflow: {}", provider, message)?;
                if let Some(max) = max_tokens {
                    write!(f, " (max {} tokens)", max)?;
                }
                Ok(())
            }
            ProviderError::RateLimited {
                provider,
                retry_after,
            } => {
                write!(f, "[{}] Rate limited", provider)?;
                if let Some(secs) = retry_after {
                    write!(f, "; retry after {}s", secs)?;
                }
                Ok(())
            }
            ProviderError::AuthFailed { provider, message } => {
                write!(f, "[{}] Authentication failed: {}", provider, message)
            }
            ProviderError::QuotaExceeded { provider, message } => {
                write!(f, "[{}] Quota exceeded: {}", provider, message)
            }
            ProviderError::ModelNotFound {
                provider,
                model,
                suggestions,
            } => {
                write!(f, "[{}] Model not found: {}", provider, model)?;
                if !suggestions.is_empty() {
                    write!(f, " (suggestions: {})", suggestions.join(", "))?;
                }
                Ok(())
            }
            ProviderError::ServerError {
                provider,
                status,
                message,
                ..
            } => match status {
                Some(s) => write!(f, "[{}] Server error {}: {}", provider, s, message),
                None => write!(f, "[{}] Server error: {}", provider, message),
            },
            ProviderError::InvalidRequest { provider, message } => {
                write!(f, "[{}] Invalid request: {}", provider, message)
            }
            ProviderError::ContentFiltered { provider, message } => {
                write!(f, "[{}] Content filtered: {}", provider, message)
            }
            ProviderError::StreamError {
                provider, message, ..
            } => {
                write!(f, "[{}] Stream error: {}", provider, message)
            }
            ProviderError::Other {
                provider,
                message,
                status,
                ..
            } => match status {
                Some(s) => write!(f, "[{}] Error {}: {}", provider, s, message),
                None => write!(f, "[{}] Error: {}", provider, message),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// std::error::Error
// ---------------------------------------------------------------------------

impl std::error::Error for ProviderError {}

// ---------------------------------------------------------------------------
// From<ProviderError> for ClaudeError
// ---------------------------------------------------------------------------

impl From<ProviderError> for ClaudeError {
    fn from(err: ProviderError) -> Self {
        match &err {
            ProviderError::ContextOverflow { .. } => ClaudeError::ContextWindowExceeded,
            ProviderError::RateLimited { .. } => ClaudeError::RateLimit,
            ProviderError::AuthFailed { message, .. } => ClaudeError::Auth(message.clone()),
            ProviderError::ServerError {
                status: Some(s),
                message,
                ..
            } => ClaudeError::ApiStatus {
                status: *s,
                message: message.clone(),
            },
            _ => ClaudeError::Api(err.to_string()),
        }
    }
}
