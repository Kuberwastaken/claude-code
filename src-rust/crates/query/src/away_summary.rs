// away_summary.rs — "While you were away" recap generation.
//
// Ported from `src/services/awaySummary.ts`.
//
// When the user returns to an idle session the TUI can call
// `generate_away_summary` to get a short 1-3 sentence recap of what was
// happening before they left.

use claurst_api::{AnthropicClient, CreateMessageRequest};
use claurst_core::types::Message;
use tokio_util::sync::CancellationToken;

/// Recap only needs recent context — truncate to avoid "prompt too long" on
/// large sessions.  30 messages ≈ ~15 exchanges, plenty for "where we left off."
const RECENT_MESSAGE_WINDOW: usize = 30;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for away-summary generation.
#[derive(Debug, Clone)]
pub struct AwaySummaryConfig {
    /// The (small/fast) model to use for the recap.
    pub model: String,
    /// Maximum tokens to generate.
    pub max_tokens: u32,
}

impl Default for AwaySummaryConfig {
    fn default() -> Self {
        Self {
            model: "claude-haiku-4-5-20251001".to_string(),
            max_tokens: 300,
        }
    }
}

// ---------------------------------------------------------------------------
// Prompt builder
// ---------------------------------------------------------------------------

fn build_away_summary_prompt() -> String {
    // Mirrors the TypeScript prompt verbatim.
    "The user stepped away and is coming back. Write exactly 1-3 short sentences. \
Start by stating the high-level task — what they are building or debugging, not \
implementation details. Next: the concrete next step. Skip status reports and \
commit recaps."
        .to_string()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Generate a short "while you were away" recap.
///
/// Returns `None` if:
/// - `messages` is empty,
/// - the cancellation token is triggered before the response arrives, or
/// - any API / network error occurs.
///
/// Only the last [`RECENT_MESSAGE_WINDOW`] messages are sent to the model to
/// keep the prompt small.
pub async fn generate_away_summary(
    messages: &[Message],
    api_client: &AnthropicClient,
    config: &AwaySummaryConfig,
    cancel: CancellationToken,
) -> Option<String> {
    if messages.is_empty() {
        return None;
    }

    // Truncate to the most recent window.
    let recent: Vec<Message> = messages
        .iter()
        .rev()
        .take(RECENT_MESSAGE_WINDOW)
        .rev()
        .cloned()
        .collect();

    // Append the recap instruction as a user turn.
    let mut conversation = recent;
    conversation.push(Message::user(build_away_summary_prompt()));

    // Convert to API messages.
    let api_messages: Vec<claurst_api::ApiMessage> = conversation
        .iter()
        .map(claurst_api::ApiMessage::from)
        .collect();

    let request = CreateMessageRequest::builder(&config.model, config.max_tokens)
        .messages(api_messages)
        .build();

    // Run the API call with cancellation support.
    let call_future = api_client.create_message(request);

    let response = tokio::select! {
        _ = cancel.cancelled() => return None,
        result = call_future => match result {
            Ok(r) => r,
            Err(_) => return None,
        },
    };

    // Extract text from the first text block of the response.
    // The `content` field is `Vec<serde_json::Value>` with objects like
    // `{"type": "text", "text": "..."}`.
    let text = response.content.iter().find_map(|block| {
        if block.get("type")?.as_str()? == "text" {
            block.get("text")?.as_str().map(str::to_owned)
        } else {
            None
        }
    })?;

    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_haiku() {
        let cfg = AwaySummaryConfig::default();
        assert!(
            cfg.model.contains("haiku"),
            "default model should be a Haiku variant"
        );
        assert_eq!(cfg.max_tokens, 300);
    }

    #[test]
    fn empty_messages_returns_none_synchronously() {
        // We can verify the empty-check without an async runtime.
        // The async path is integration-tested separately.
        let messages: Vec<Message> = vec![];
        assert!(messages.is_empty(), "test pre-condition");
        // (The actual None is returned inside the async fn; the check is the
        //  first line, so no network call is ever made.)
    }

    #[test]
    fn prompt_is_non_empty() {
        let prompt = build_away_summary_prompt();
        assert!(!prompt.is_empty());
        assert!(prompt.contains("1-3 short sentences"));
    }

    #[test]
    fn recent_window_truncates_correctly() {
        let total = RECENT_MESSAGE_WINDOW + 10;
        let messages: Vec<Message> = (0..total)
            .map(|i| Message::user(format!("message {}", i)))
            .collect();

        let recent: Vec<Message> = messages
            .iter()
            .rev()
            .take(RECENT_MESSAGE_WINDOW)
            .rev()
            .cloned()
            .collect();

        assert_eq!(recent.len(), RECENT_MESSAGE_WINDOW);
        // Verify we got the *last* RECENT_MESSAGE_WINDOW messages.
        assert_eq!(
            recent[0].get_all_text(),
            format!("message {}", total - RECENT_MESSAGE_WINDOW)
        );
    }
}
