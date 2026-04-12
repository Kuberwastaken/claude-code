//! Token budget utilities — mirrors src/utils/context/token_budget.ts
//!
//! Provides helpers for tracking token usage, computing warning thresholds,
//! and building thinking budget configs for extended thinking.

use serde::{Deserialize, Serialize};

/// Warning level based on context window fill percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TokenWarningLevel {
    /// < 80% used — no warning.
    None,
    /// >= 80% used — show caution indicator.
    Warning,
    /// >= 95% used — show critical warning; compact strongly recommended.
    Critical,
}

/// Token budget snapshot for a single turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudget {
    /// Tokens used so far in the context window.
    pub tokens_used: u64,
    /// Maximum context window size for the current model.
    pub context_window: u64,
    /// Remaining tokens available.
    pub tokens_remaining: u64,
    /// Fill fraction in [0, 1].
    pub fill_fraction: f64,
    /// Warning level derived from fill fraction.
    pub warning_level: TokenWarningLevel,
}

impl TokenBudget {
    /// Construct from used/total pair.
    pub fn new(tokens_used: u64, context_window: u64) -> Self {
        let remaining = context_window.saturating_sub(tokens_used);
        let fraction = if context_window == 0 {
            0.0
        } else {
            tokens_used as f64 / context_window as f64
        };
        let warning_level = if fraction >= 0.95 {
            TokenWarningLevel::Critical
        } else if fraction >= 0.80 {
            TokenWarningLevel::Warning
        } else {
            TokenWarningLevel::None
        };
        Self {
            tokens_used,
            context_window,
            tokens_remaining: remaining,
            fill_fraction: fraction,
            warning_level,
        }
    }

    /// True if we should trigger reactive compact (≥ 90% used).
    pub fn should_compact(&self) -> bool {
        self.fill_fraction >= 0.90
    }

    /// True if we should trigger context collapse (≥ 97% used).
    pub fn should_collapse(&self) -> bool {
        self.fill_fraction >= 0.97
    }

    /// Format as a human-readable string: "42K / 200K (21%)".
    pub fn display(&self) -> String {
        format!(
            "{} / {} ({:.0}%)",
            format_token_count(self.tokens_used),
            format_token_count(self.context_window),
            self.fill_fraction * 100.0,
        )
    }
}

/// Format a token count as a compact string: 1234 → "1.2K", 1234567 → "1.2M".
pub fn format_token_count(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// Context window sizes for known models.
/// Returns None if the model is unknown (caller should use a safe default).
pub fn context_window_for_model(model: &str) -> Option<u64> {
    let model_lower = model.to_lowercase();
    // claude-4 family
    if model_lower.contains("claude-opus-4")
        || model_lower.contains("claude-sonnet-4")
        || model_lower.contains("claude-haiku-4")
    {
        return Some(200_000);
    }
    // claude-3-5 family
    if model_lower.contains("claude-3-5") {
        return Some(200_000);
    }
    // claude-3 family
    if model_lower.contains("claude-3-opus")
        || model_lower.contains("claude-3-sonnet")
        || model_lower.contains("claude-3-haiku")
    {
        return Some(200_000);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_levels() {
        assert_eq!(
            TokenBudget::new(79_000, 100_000).warning_level,
            TokenWarningLevel::None
        );
        assert_eq!(
            TokenBudget::new(80_000, 100_000).warning_level,
            TokenWarningLevel::Warning
        );
        assert_eq!(
            TokenBudget::new(95_000, 100_000).warning_level,
            TokenWarningLevel::Critical
        );
    }

    #[test]
    fn should_compact_threshold() {
        assert!(!TokenBudget::new(89_000, 100_000).should_compact());
        assert!(TokenBudget::new(90_000, 100_000).should_compact());
    }

    #[test]
    fn format_token_count_cases() {
        assert_eq!(format_token_count(500), "500");
        assert_eq!(format_token_count(1500), "1.5K");
        assert_eq!(format_token_count(1_200_000), "1.2M");
    }
}
