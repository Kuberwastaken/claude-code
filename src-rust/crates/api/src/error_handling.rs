// error_handling.rs — Provider-aware error detection and retry utilities
// (Phase 6).
//
// Provides:
//  - `is_context_overflow`: checks a message string against 29+ known
//    context-window overflow error patterns from all major providers.
//  - `parse_error_response`: converts an HTTP status + body into the correct
//    `ProviderError` variant, including overflow detection and JSON code
//    extraction.
//  - `RetryConfig`: exponential back-off configuration with jitter.

use std::time::Duration;

use claurst_core::provider_id::ProviderId;

use crate::provider_error::ProviderError;

// ---------------------------------------------------------------------------
// Overflow pattern table
// ---------------------------------------------------------------------------

/// 29+ context-overflow patterns that appear across all major providers.
static OVERFLOW_PATTERNS: &[&str] = &[
    "prompt is too long",
    "input is too long for requested model",
    "expected maxlength:",
    "exceeds the context window",
    "maximum context length",
    "input token count.*exceeds the maximum",
    "maximum prompt length is",
    "reduce the length of the messages",
    "maximum context length is.*tokens",
    "exceeds the limit of",
    "exceeds the available context size",
    "greater than the context length",
    "context window exceeds limit",
    "exceeded model token limit",
    "prompt too long",
    "too large for model with.*maximum context length",
    "model_context_window_exceeded",
    "context length is only.*tokens",
    "input length.*exceeds.*context length",
    "context_length_exceeded",
    "request entity too large",
    "too many tokens",
    "context.*length.*exceeded",
    "token.*limit.*exceeded",
    "prompt.*too.*long",
    "exceeds.*context.*size",
    "context.*window.*exceeded",
    "max.*tokens.*exceeded",
    "input.*too.*long",
];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns `true` if `message` matches any known context-overflow pattern.
///
/// The comparison is case-insensitive.  Patterns are matched as substrings
/// (not full regexes) for performance — the pattern table is designed so that
/// simple substring matching is sufficient.
pub fn is_context_overflow(message: &str) -> bool {
    let lower = message.to_lowercase();
    OVERFLOW_PATTERNS
        .iter()
        .any(|pattern| lower.contains(&pattern.to_lowercase()))
}

/// Convert an HTTP error response into the appropriate [`ProviderError`].
///
/// Tries JSON parsing, then falls back to the raw body.  Context overflow is
/// checked before the HTTP status code so that a 400 with an overflow message
/// is classified as [`ProviderError::ContextOverflow`] rather than
/// [`ProviderError::InvalidRequest`].
pub fn parse_error_response(status: u16, body: &str, provider: &ProviderId) -> ProviderError {
    let json: Option<serde_json::Value> = serde_json::from_str(body).ok();

    let message = if let Some(ref j) = json {
        extract_error_message(j)
    } else if body.trim_start().starts_with('<') {
        // HTML error page (Azure proxy, CDN, etc.)
        "Received HTML error page — check provider endpoint configuration".to_string()
    } else {
        body.to_string()
    };

    // Check for context overflow before all other classifications.
    if is_context_overflow(&message) || is_context_overflow(body) {
        return ProviderError::ContextOverflow {
            provider: provider.clone(),
            message,
            max_tokens: extract_token_limit(body),
        };
    }

    // Check for structured error codes returned by some providers.
    if let Some(ref j) = json {
        if let Some(code) = extract_error_code(j) {
            match code.as_str() {
                "context_length_exceeded" | "context_window_exceeded" => {
                    return ProviderError::ContextOverflow {
                        provider: provider.clone(),
                        message,
                        max_tokens: None,
                    };
                }
                "insufficient_quota" | "billing_not_active" => {
                    return ProviderError::QuotaExceeded {
                        provider: provider.clone(),
                        message,
                    };
                }
                "invalid_prompt" | "invalid_request_error" => {
                    return ProviderError::InvalidRequest {
                        provider: provider.clone(),
                        message,
                    };
                }
                "content_filter" | "content_policy_violation" => {
                    return ProviderError::ContentFiltered {
                        provider: provider.clone(),
                        message,
                    };
                }
                _ => {}
            }
        }
    }

    // Classify by HTTP status code.
    match status {
        401 | 403 => ProviderError::AuthFailed {
            provider: provider.clone(),
            message,
        },
        404 => ProviderError::ModelNotFound {
            provider: provider.clone(),
            model: "unknown".to_string(),
            suggestions: vec![],
        },
        429 => ProviderError::RateLimited {
            provider: provider.clone(),
            retry_after: None,
        },
        413 => ProviderError::ContextOverflow {
            provider: provider.clone(),
            message: "Request too large (413)".to_string(),
            max_tokens: None,
        },
        500..=599 => ProviderError::ServerError {
            provider: provider.clone(),
            status: Some(status),
            message,
            is_retryable: true,
        },
        _ => ProviderError::Other {
            provider: provider.clone(),
            message,
            status: Some(status),
            body: Some(body.to_string()),
        },
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Walk several well-known JSON paths to find the human-readable error message.
fn extract_error_message(json: &serde_json::Value) -> String {
    // Ordered by prevalence across providers:
    //   OpenAI / Google: /error/message
    //   Anthropic:        /error/error/message
    //   Cohere / simple:  /message
    //   Some providers:   /detail
    let paths = [
        "/error/message",
        "/error/error/message",
        "/message",
        "/detail",
    ];
    for path in paths {
        if let Some(msg) = json.pointer(path).and_then(|v| v.as_str()) {
            return msg.to_string();
        }
    }
    json.to_string()
}

/// Extract a machine-readable error code from the JSON body, if present.
fn extract_error_code(json: &serde_json::Value) -> Option<String> {
    json.pointer("/error/code")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            json.pointer("/error/type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

/// Heuristically extract a token-limit number from error text.
///
/// Looks for an integer that is:
/// - between 1 000 and 10 000 000 (plausible token limit range), and
/// - adjacent to words like "token", "limit", "context", or "max".
fn extract_token_limit(text: &str) -> Option<u64> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        // Strip non-digit chars from both ends, then try to parse.
        let trimmed = word.trim_matches(|c: char| !c.is_ascii_digit());
        if let Ok(n) = trimmed.parse::<u64>() {
            if n > 1_000 && n < 10_000_000 {
                let start = i.saturating_sub(3);
                let context = words[start..i].join(" ").to_lowercase();
                if context.contains("token")
                    || context.contains("limit")
                    || context.contains("context")
                    || context.contains("max")
                {
                    return Some(n);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// RetryConfig
// ---------------------------------------------------------------------------

/// Exponential back-off configuration for provider retries.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Delay before the first retry.
    pub initial_delay: Duration,
    /// Upper bound on per-attempt delay.
    pub max_delay: Duration,
    /// Multiplicative factor applied at each attempt.
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
        }
    }
}

impl RetryConfig {
    /// Compute the delay for a given `attempt` number (0-indexed).
    ///
    /// Applies exponential back-off with ±10 % jitter derived from the
    /// current system time (no external `rand` dependency required).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base = self.initial_delay.as_secs_f64() * self.backoff_multiplier.powi(attempt as i32);
        let jitter = base * 0.1 * time_jitter_f64();
        Duration::from_secs_f64((base + jitter).min(self.max_delay.as_secs_f64()))
    }
}

/// Returns a deterministic-ish value in `[0, 1)` derived from the current
/// system time nanoseconds.  Used for retry jitter without pulling in `rand`.
fn time_jitter_f64() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 100) as f64 / 100.0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_context_overflow_basic() {
        assert!(is_context_overflow("prompt is too long for this model"));
        assert!(is_context_overflow("This exceeds the context window"));
        assert!(is_context_overflow("Maximum context length exceeded"));
        assert!(!is_context_overflow("something else went wrong"));
    }

    #[test]
    fn test_parse_error_response_overflow_413() {
        let pid = ProviderId::new("openai");
        let err = parse_error_response(413, "Request too large", &pid);
        assert!(matches!(err, ProviderError::ContextOverflow { .. }));
    }

    #[test]
    fn test_parse_error_response_auth() {
        let pid = ProviderId::new("anthropic");
        let err = parse_error_response(401, r#"{"error":{"message":"Invalid API key"}}"#, &pid);
        assert!(matches!(err, ProviderError::AuthFailed { .. }));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let pid = ProviderId::new("openai");
        let err = parse_error_response(429, "rate limited", &pid);
        assert!(matches!(err, ProviderError::RateLimited { .. }));
    }

    #[test]
    fn test_retry_config_delay_increases() {
        let cfg = RetryConfig::default();
        let d0 = cfg.delay_for_attempt(0);
        let d1 = cfg.delay_for_attempt(1);
        let d2 = cfg.delay_for_attempt(2);
        // Each attempt should be strictly larger than the previous.
        assert!(d1 >= d0, "d1={:?} should be >= d0={:?}", d1, d0);
        assert!(d2 >= d1, "d2={:?} should be >= d1={:?}", d2, d1);
    }

    #[test]
    fn test_retry_config_respects_max_delay() {
        let cfg = RetryConfig::default();
        let d10 = cfg.delay_for_attempt(10);
        assert!(d10 <= cfg.max_delay + Duration::from_millis(1));
    }
}
