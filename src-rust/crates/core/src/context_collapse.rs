//! Context Collapse Service
//!
//! Automatically reduces conversation size to fit within model context windows.
//! Uses simple word-count estimation and message dropping strategy.
//!
//! Gated behind `cached_microcompact` feature flag.

use crate::types::Message;
use serde::{Deserialize, Serialize};

/// Strategy for collapsing a conversation when it exceeds token limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CollapseStrategy {
    /// Drop oldest non-system messages first
    DropOldest,
    /// Summarize the middle of the conversation
    Summarize,
}

/// Collapse state persisted to disk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollapseState {
    pub session_id: String,
    pub messages_dropped: usize,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub strategy_used: String,
    pub collapsed_at: String,
}

/// Simple token estimator: ~4 characters per token
const CHARS_PER_TOKEN: usize = 4;

/// Estimate token count from text using simple heuristic.
fn estimate_tokens(text: &str) -> u64 {
    (text.len() / CHARS_PER_TOKEN).max(1) as u64
}

/// Estimate total tokens in a message list.
pub fn estimate_message_tokens(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|m| {
            let content_tokens = estimate_tokens(&format!("{:?}", m.content));
            let role_tokens = 2u64; // "user", "assistant", etc.
            content_tokens + role_tokens
        })
        .sum()
}

/// Collapse a message list to fit within max_tokens.
/// Returns the collapsed message list and collapse state (if collapsing occurred).
#[cfg(feature = "cached_microcompact")]
pub fn collapse_context(
    messages: Vec<Message>,
    max_tokens: u64,
    strategy: CollapseStrategy,
) -> (Vec<Message>, Option<CollapseState>) {
    let initial_tokens = estimate_message_tokens(&messages);

    // Already under limit
    if initial_tokens <= max_tokens {
        return (messages, None);
    }

    let (collapsed, dropped_count) = match strategy {
        CollapseStrategy::DropOldest => drop_oldest_messages(messages, max_tokens),
        CollapseStrategy::Summarize => summarize_messages(messages, max_tokens),
    };

    let final_tokens = estimate_message_tokens(&collapsed);

    let state = CollapseState {
        session_id: "unknown".to_string(),
        messages_dropped: dropped_count,
        tokens_before: initial_tokens,
        tokens_after: final_tokens,
        strategy_used: format!("{:?}", strategy),
        collapsed_at: chrono::Utc::now().to_rfc3339(),
    };

    (collapsed, Some(state))
}

#[cfg(not(feature = "cached_microcompact"))]
pub fn collapse_context(
    messages: Vec<Message>,
    _max_tokens: u64,
    _strategy: CollapseStrategy,
) -> (Vec<Message>, Option<CollapseState>) {
    // Without feature flag, return as-is
    (messages, None)
}

/// Drop oldest non-system messages until under token limit.
#[cfg(feature = "cached_microcompact")]
fn drop_oldest_messages(mut messages: Vec<Message>, max_tokens: u64) -> (Vec<Message>, usize) {
    let mut dropped = 0;

    // Find first non-system user/assistant message (skip system roles)
    let first_user_idx = messages
        .iter()
        .position(|m| m.role != Role::Assistant) // Keep assistant responses, drop user turns
        .unwrap_or(0);

    // Drop messages starting from first_user_idx
    while estimate_message_tokens(&messages) > max_tokens && messages.len() > first_user_idx + 1 {
        messages.remove(first_user_idx);
        dropped += 1;
    }

    (messages, dropped)
}

/// Summarize conversation by keeping first and last N messages.
/// (Full summarization would require calling Claude, so this is a placeholder.)
#[cfg(feature = "cached_microcompact")]
fn summarize_messages(messages: Vec<Message>, _max_tokens: u64) -> (Vec<Message>, usize) {
    let initial_len = messages.len();

    // Simple heuristic: keep first and last 30% of messages
    let target_len = (messages.len() as f64 * 0.6) as usize;
    let keep_per_side = target_len / 2;

    let result = if messages.len() > target_len {
        let mut kept = Vec::new();

        // Keep first messages
        for (i, msg) in messages.iter().enumerate() {
            if i < keep_per_side {
                kept.push(msg.clone());
            }
        }

        // Skip to near end and keep last messages
        let skip_idx = initial_len.saturating_sub(keep_per_side);
        for (i, msg) in messages.iter().enumerate() {
            if i >= skip_idx {
                kept.push(msg.clone());
            }
        }
        kept
    } else {
        messages
    };

    let dropped = initial_len - result.len();
    (result, dropped)
}

/// Persist collapse state to ~/.claurst/context_collapse_state.json
#[cfg(feature = "cached_microcompact")]
pub fn save_collapse_state(_session_id: &str, state: &CollapseState) -> anyhow::Result<()> {
    let path = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?
        .join(".claurst")
        .join("context_collapse_state.json");

    std::fs::create_dir_all(path.parent().unwrap())?;
    let json = serde_json::to_string(state)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Load collapse state from ~/.claurst/context_collapse_state.json
#[cfg(feature = "cached_microcompact")]
pub fn load_collapse_state(_session_id: &str) -> Option<CollapseState> {
    let path = dirs::home_dir()?
        .join(".claurst")
        .join("context_collapse_state.json");

    if !path.exists() {
        return None;
    }

    let json = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        let text = "This is a test message with some content";
        let tokens = estimate_tokens(text);
        assert!(tokens > 0);
    }
}
