//! Message manipulation utilities.
//! Mirrors key functions from src/utils/messages.ts (5,512 lines).

use crate::types::{ContentBlock, Message, MessageContent, Role};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Token estimation
// ---------------------------------------------------------------------------

/// Rough token count heuristic (chars / 4 + JSON overhead).
/// Mirrors the TS `countTokens` approximation.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.len() as f64 / 4.0).ceil() as u64
}

/// Estimate total tokens for a slice of messages.
pub fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|m| estimate_tokens(&get_message_text(m)) + 4)
        .sum()
}

/// Context-window info for a model / token count pair.
pub struct ContextUsage {
    pub used: u64,
    pub total: u64,
    pub pct: f64,
}

/// Calculate context window usage.
pub fn calculate_context_window_usage(messages: &[Message], model: &str) -> ContextUsage {
    let used = estimate_messages_tokens(messages);
    let total = context_window_for_model(model);
    let pct = if total > 0 {
        (used as f64 / total as f64) * 100.0
    } else {
        0.0
    };
    ContextUsage { used, total, pct }
}

/// Return the context window token limit for a known model.
pub fn context_window_for_model(model: &str) -> u64 {
    if model.contains("claude-3-5-haiku") {
        return 200_000;
    }
    if model.contains("claude-3-5-sonnet") {
        return 200_000;
    }
    if model.contains("claude-3-7-sonnet") {
        return 200_000;
    }
    if model.contains("claude-sonnet-4") {
        return 200_000;
    }
    if model.contains("claude-opus-4") {
        return 200_000;
    }
    if model.contains("opus") {
        return 200_000;
    }
    if model.contains("haiku") {
        return 200_000;
    }
    200_000 // safe default
}

// ---------------------------------------------------------------------------
// Message content helpers
// ---------------------------------------------------------------------------

/// Extract all displayable text from a message.
pub fn get_message_text(msg: &Message) -> String {
    match &msg.content {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Returns `true` if the message is a tool-use turn (assistant with tool_use blocks).
pub fn is_tool_use_message(msg: &Message) -> bool {
    msg.role == Role::Assistant
        && match &msg.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. })),
            _ => false,
        }
}

/// Returns `true` if the message is a tool-result turn.
pub fn is_tool_result_message(msg: &Message) -> bool {
    msg.role == Role::User
        && match &msg.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. })),
            _ => false,
        }
}

/// Merge consecutive `Text` blocks in a content array.
pub fn merge_consecutive_text_blocks(blocks: Vec<ContentBlock>) -> Vec<ContentBlock> {
    let mut result: Vec<ContentBlock> = Vec::new();
    for block in blocks {
        if let ContentBlock::Text { text } = &block {
            if let Some(ContentBlock::Text { text: prev_text }) = result.last_mut() {
                prev_text.push('\n');
                prev_text.push_str(text);
                continue;
            }
        }
        result.push(block);
    }
    result
}

/// Truncate the text content of a message to `max_chars`.
pub fn truncate_message_content(msg: &mut Message, max_chars: usize) {
    match &mut msg.content {
        MessageContent::Text(s) => {
            if s.len() > max_chars {
                s.truncate(max_chars);
                s.push_str("\u{2026}[truncated]");
            }
        }
        MessageContent::Blocks(blocks) => {
            for block in blocks.iter_mut() {
                if let ContentBlock::Text { text } = block {
                    if text.len() > max_chars {
                        text.truncate(max_chars);
                        text.push_str("\u{2026}[truncated]");
                    }
                }
            }
        }
    }
}

/// Format a tool result value for display / history.
pub fn format_tool_result(result: &Value) -> String {
    match result {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| {
                v.get("text")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, Message, MessageContent, Role};

    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
            uuid: None,
            cost: None,
        }
    }

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens("hello"), 2); // 5/4 = 1.25 → ceil = 2
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn get_text_from_text_message() {
        let m = user_msg("hello world");
        assert_eq!(get_message_text(&m), "hello world");
    }

    #[test]
    fn merge_text_blocks() {
        let blocks = vec![
            ContentBlock::Text {
                text: "a".to_string(),
            },
            ContentBlock::Text {
                text: "b".to_string(),
            },
        ];
        let merged = merge_consecutive_text_blocks(blocks);
        assert_eq!(merged.len(), 1);
        if let ContentBlock::Text { text } = &merged[0] {
            assert_eq!(text, "a\nb");
        }
    }
}
