// Auto-compact service for cc-query.
//
// When the conversation context window fills up (~90%+), we automatically
// summarise older messages to free space. This mirrors the TypeScript
// autoCompact / compact service behaviour.
//
// Strategy:
//   1. Keep the last KEEP_RECENT_MESSAGES messages verbatim.
//   2. Ask the model to summarise everything before those messages.
//   3. Replace the head of the conversation with a single synthetic
//      <compact-summary> user message, followed by the recent tail.
//
// The summary is generated in a single non-agentic API call so it doesn't
// trigger another compaction recursively.

use cc_api::{ApiMessage, CreateMessageRequest, StreamAccumulator, StreamEvent, StreamHandler, SystemPrompt};
use cc_core::error::ClaudeError;
use cc_core::types::{Message, Role};
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Constants (mirrors TypeScript autoCompact.ts)
// ---------------------------------------------------------------------------

/// We target keeping this many context tokens free after compaction.
#[allow(dead_code)]
const AUTOCOMPACT_BUFFER_TOKENS: u64 = 13_000;

/// Start warning when this many tokens remain in the context window.
const WARNING_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;

/// Fraction of the context window at which auto-compact triggers.
const AUTOCOMPACT_TRIGGER_FRACTION: f64 = 0.90;

/// How many recent messages to preserve verbatim after compaction.
const KEEP_RECENT_MESSAGES: usize = 10;

/// Max consecutive auto-compact failures before giving up (circuit breaker).
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Tracks auto-compact state across turns.
#[derive(Debug, Default, Clone)]
pub struct AutoCompactState {
    /// Total compactions performed this session.
    pub compaction_count: u32,
    /// Consecutive failures (reset on success).
    pub consecutive_failures: u32,
    /// Whether the circuit breaker is open (too many failures).
    pub disabled: bool,
}

impl AutoCompactState {
    /// Record a successful compaction.
    pub fn on_success(&mut self) {
        self.compaction_count += 1;
        self.consecutive_failures = 0;
    }

    /// Record a failed compaction; open circuit breaker if too many.
    pub fn on_failure(&mut self) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            warn!(
                failures = self.consecutive_failures,
                "Auto-compact circuit breaker opened – disabling for this session"
            );
            self.disabled = true;
        }
    }
}

/// Token-usage state relative to the context window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenWarningState {
    /// Plenty of space left.
    Ok,
    /// Getting close – warn the user.
    Warning,
    /// Critical – compact now.
    Critical,
}

// ---------------------------------------------------------------------------
// Threshold helpers
// ---------------------------------------------------------------------------

/// Return the effective context-window size in tokens for the given model.
/// These are approximate; the API enforces the real limits server-side.
pub fn context_window_for_model(model: &str) -> u64 {
    if model.contains("opus-4") || model.contains("sonnet-4") || model.contains("haiku-4") {
        200_000
    } else if model.contains("claude-3-5") || model.contains("claude-3.5") {
        200_000
    } else {
        100_000
    }
}

/// Determine token-warning state given current input token count and model.
pub fn calculate_token_warning_state(input_tokens: u64, model: &str) -> TokenWarningState {
    let window = context_window_for_model(model);
    let remaining = window.saturating_sub(input_tokens);

    if remaining <= WARNING_THRESHOLD_BUFFER_TOKENS as u64 {
        TokenWarningState::Warning
    } else {
        TokenWarningState::Ok
    }
}

/// Return `true` when auto-compaction should fire.
pub fn should_auto_compact(input_tokens: u64, model: &str, state: &AutoCompactState) -> bool {
    if state.disabled {
        return false;
    }
    let window = context_window_for_model(model);
    let threshold = (window as f64 * AUTOCOMPACT_TRIGGER_FRACTION) as u64;
    input_tokens >= threshold
}

// ---------------------------------------------------------------------------
// Core compaction logic
// ---------------------------------------------------------------------------

/// Summarise `messages[..split_at]` using the Anthropic API and return a
/// new conversation consisting of a single summary message followed by
/// `messages[split_at..]`.
async fn summarise_head(
    client: &(dyn cc_api::LlmClient + Send + Sync),
    messages: &[Message],
    split_at: usize,
    model: &str,
) -> Result<Vec<Message>, ClaudeError> {
    if split_at == 0 {
        return Ok(messages.to_vec());
    }

    let head = &messages[..split_at];

    // Build a transcript string for the summarisation prompt.
    let mut transcript = String::new();
    for msg in head {
        let role_label = match msg.role {
            Role::User => "Human",
            Role::Assistant => "Assistant",
        };
        let text = msg.get_all_text();
        if !text.is_empty() {
            transcript.push_str(&format!("{}: {}\n\n", role_label, text));
        }
    }

    let summarise_prompt = format!(
        "Please create a comprehensive yet concise summary of the conversation transcript \
         below. The summary will be used as context for continuing the conversation, so \
         include all important decisions, code changes, findings, and context that would be \
         needed to continue seamlessly.\n\n\
         Focus on:\n\
         - Key decisions made and their rationale\n\
         - Code or files that were created/modified\n\
         - Important findings or conclusions\n\
         - The current state of any ongoing tasks\n\
         - Any constraints or requirements discovered\n\n\
         <transcript>\n{}\n</transcript>",
        transcript
    );

    let api_msgs = vec![ApiMessage {
        role: "user".to_string(),
        content: Value::String(summarise_prompt),
    }];

    let request = CreateMessageRequest::builder(model, 4096)
        .messages(api_msgs)
        .system(SystemPrompt::Text(
            "You are a helpful assistant that creates concise conversation summaries. \
             Be thorough but concise. Preserve technical details, file names, and code snippets \
             that would be important for continuing the work."
                .to_string(),
        ))
        .build();

    // Use a null handler since we just want the final accumulated message.
    let handler: Arc<dyn StreamHandler> = Arc::new(cc_api::streaming::NullStreamHandler);
    let mut rx = client.create_message_stream(request, handler).await?;
    let mut acc = StreamAccumulator::new();

    while let Some(evt) = rx.recv().await {
        acc.on_event(&evt);
        if matches!(evt, StreamEvent::MessageStop) {
            break;
        }
    }

    let (summary_msg, _usage, _stop) = acc.finish();
    let summary_text = summary_msg.get_all_text();

    if summary_text.is_empty() {
        return Err(ClaudeError::Other("Compact summary was empty".to_string()));
    }

    // Build the new conversation:
    //   [user: compact summary preamble] [assistant: summary content] [tail messages]
    let compact_notice = Message::user(format!(
        "<compact-summary>\n\
         The conversation history has been automatically compacted to stay within context limits.\n\
         The following is a summary of the previous conversation:\n\n\
         {}\n\
         </compact-summary>",
        summary_text
    ));

    let mut new_messages = vec![compact_notice];
    new_messages.extend_from_slice(&messages[split_at..]);

    Ok(new_messages)
}

/// Compact `messages` in-place, replacing the head with a summary.
/// Returns the new messages vector on success.
pub async fn compact_conversation(
    client: &(dyn cc_api::LlmClient + Send + Sync),
    messages: &[Message],
    model: &str,
) -> Result<Vec<Message>, ClaudeError> {
    let total = messages.len();

    if total <= KEEP_RECENT_MESSAGES + 1 {
        debug!(
            total,
            "Too few messages to compact – keeping everything"
        );
        return Ok(messages.to_vec());
    }

    // Split: summarise everything except the most recent KEEP_RECENT_MESSAGES.
    let split_at = total.saturating_sub(KEEP_RECENT_MESSAGES);

    info!(
        total,
        split_at,
        keep = KEEP_RECENT_MESSAGES,
        "Compacting conversation"
    );

    summarise_head(client, messages, split_at, model).await
}

/// Auto-compact `messages` if needed.  Updates `state` in place.
/// Returns `Some(new_messages)` if compaction ran, `None` otherwise.
pub async fn auto_compact_if_needed(
    client: &(dyn cc_api::LlmClient + Send + Sync),
    messages: &[Message],
    input_tokens: u64,
    model: &str,
    state: &mut AutoCompactState,
) -> Option<Vec<Message>> {
    if !should_auto_compact(input_tokens, model, state) {
        return None;
    }

    info!(
        input_tokens,
        model,
        compaction_count = state.compaction_count,
        "Auto-compact triggered"
    );

    match compact_conversation(client, messages, model).await {
        Ok(new_msgs) => {
            state.on_success();
            info!(
                original_count = messages.len(),
                new_count = new_msgs.len(),
                "Auto-compact complete"
            );
            Some(new_msgs)
        }
        Err(e) => {
            warn!(error = %e, "Auto-compact failed");
            state.on_failure();
            None
        }
    }
}
