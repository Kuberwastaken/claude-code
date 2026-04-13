//! Context window analysis utilities.
//! Mirrors src/utils/analyzeContext.ts (1,382 lines).
//! Used by the /ctx-viz slash command.

use claurst_core::types::{ContentBlock, Message, MessageContent};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Token category for context window breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextCategory {
    SystemPrompt,
    ToolDefinitions,
    ConversationHistory,
    ToolResults,
    Attachments,
    Unknown,
}

impl ContextCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::SystemPrompt => "System prompt",
            Self::ToolDefinitions => "Tool definitions",
            Self::ConversationHistory => "Conversation history",
            Self::ToolResults => "Tool results",
            Self::Attachments => "Attachments",
            Self::Unknown => "Other",
        }
    }
}

/// Token count breakdown by category.
#[derive(Debug, Clone, Default)]
pub struct ContextAnalysis {
    pub system_prompt_tokens: u64,
    pub tool_definitions_tokens: u64,
    pub conversation_history_tokens: u64,
    pub tool_results_tokens: u64,
    pub attachments_tokens: u64,
    pub total_tokens: u64,
    /// Overall compressibility estimate (0.0 = not compressible, 1.0 = highly compressible).
    pub compressibility: f64,
}

impl ContextAnalysis {
    /// Percentage of total tokens used by each category.
    pub fn category_pct(&self, cat: ContextCategory) -> f64 {
        if self.total_tokens == 0 {
            return 0.0;
        }
        let count = match cat {
            ContextCategory::SystemPrompt => self.system_prompt_tokens,
            ContextCategory::ToolDefinitions => self.tool_definitions_tokens,
            ContextCategory::ConversationHistory => self.conversation_history_tokens,
            ContextCategory::ToolResults => self.tool_results_tokens,
            ContextCategory::Attachments => self.attachments_tokens,
            ContextCategory::Unknown => 0,
        };
        (count as f64 / self.total_tokens as f64) * 100.0
    }
}

/// Compaction strategy recommendation.
#[derive(Debug, Clone)]
pub enum CompactionStrategy {
    /// Full history compaction — all messages summarised.
    FullCompact { expected_reduction_pct: f64 },
    /// Partial compaction — only oldest N messages.
    PartialCompact {
        messages_to_compact: usize,
        expected_reduction_pct: f64,
    },
    /// Collapse repeated file reads.
    CollapseReads { expected_reduction_pct: f64 },
    /// Nothing needed.
    None,
}

// ---------------------------------------------------------------------------
// Token estimation (mirrors cc-core::message_utils)
// ---------------------------------------------------------------------------

fn estimate_chars(s: &str) -> u64 {
    (s.len() as f64 / 4.0).ceil() as u64
}

fn content_tokens(content: &MessageContent) -> u64 {
    match content {
        MessageContent::Text(s) => estimate_chars(s),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .map(|b| match b {
                ContentBlock::Text { text } => estimate_chars(text),
                ContentBlock::Thinking { thinking, .. } => estimate_chars(thinking),
                ContentBlock::ToolUse { name, input, .. } => {
                    estimate_chars(name) + estimate_chars(&input.to_string())
                }
                ContentBlock::ToolResult { content, .. } => {
                    use claurst_core::types::ToolResultContent;
                    match content {
                        ToolResultContent::Text(t) => estimate_chars(t),
                        ToolResultContent::Blocks(inner) => inner
                            .iter()
                            .map(|ib| {
                                if let ContentBlock::Text { text } = ib {
                                    estimate_chars(text)
                                } else {
                                    10
                                }
                            })
                            .sum(),
                    }
                }
                _ => 10,
            })
            .sum(),
    }
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

/// Analyse the context window usage by category.
///
/// `system_prompt` and `tool_defs_json` are the separately-tracked strings;
/// `messages` are the in-context conversation turns.
pub fn analyze_context(
    system_prompt: Option<&str>,
    tool_defs_json: Option<&str>,
    messages: &[Message],
) -> ContextAnalysis {
    let sp_tokens = system_prompt.map_or(0, estimate_chars);
    let td_tokens = tool_defs_json.map_or(0, estimate_chars);

    let mut conv_tokens: u64 = 0;
    let mut tool_result_tokens: u64 = 0;
    let mut attach_tokens: u64 = 0;

    for msg in messages {
        let is_tool_result = matches!(&msg.content, MessageContent::Blocks(b)
            if b.iter().any(|bl| matches!(bl, ContentBlock::ToolResult { .. })));

        let toks = content_tokens(&msg.content);

        if is_tool_result {
            tool_result_tokens += toks;
        } else {
            // Heuristic: assistant messages containing "attachment" text go to attachments.
            let text = match &msg.content {
                MessageContent::Text(s) => s.as_str(),
                _ => "",
            };
            if text.contains("[Attachment:") || text.contains("[IDE:") || text.contains("[Pasted") {
                attach_tokens += toks;
            } else {
                conv_tokens += toks;
            }
        }
    }

    let total = sp_tokens + td_tokens + conv_tokens + tool_result_tokens + attach_tokens;

    // Compressibility: tool results are highly compressible; conversation is moderate.
    let compressibility = if total == 0 {
        0.0
    } else {
        let compressible = tool_result_tokens as f64 * 0.9 + conv_tokens as f64 * 0.5;
        compressible / total as f64
    };

    ContextAnalysis {
        system_prompt_tokens: sp_tokens,
        tool_definitions_tokens: td_tokens,
        conversation_history_tokens: conv_tokens,
        tool_results_tokens: tool_result_tokens,
        attachments_tokens: attach_tokens,
        total_tokens: total,
        compressibility,
    }
}

/// Suggest a compaction strategy based on the analysis.
pub fn suggest_compaction(analysis: &ContextAnalysis, context_limit: u64) -> CompactionStrategy {
    if context_limit == 0 || analysis.total_tokens == 0 {
        return CompactionStrategy::None;
    }

    let usage_pct = analysis.total_tokens as f64 / context_limit as f64;

    if usage_pct < 0.75 {
        return CompactionStrategy::None;
    }

    // If tool results dominate (> 40%), suggest collapsing reads first.
    let tool_result_pct = analysis.tool_results_tokens as f64 / analysis.total_tokens as f64;
    if tool_result_pct > 0.4 && usage_pct < 0.90 {
        return CompactionStrategy::CollapseReads {
            expected_reduction_pct: tool_result_pct * 0.7 * 100.0,
        };
    }

    // If usage > 90%, suggest full compact.
    if usage_pct > 0.90 {
        return CompactionStrategy::FullCompact {
            expected_reduction_pct: analysis.compressibility * 70.0,
        };
    }

    // Otherwise partial compact of oldest 50% of conversation.
    let messages_to_compact = (analysis.conversation_history_tokens / 2 / 50).max(1) as usize;
    CompactionStrategy::PartialCompact {
        messages_to_compact,
        expected_reduction_pct: 40.0,
    }
}

/// Format the context analysis as a human-readable breakdown string.
///
/// Used by the /ctx-viz slash command.
pub fn format_ctx_viz(analysis: &ContextAnalysis, context_limit: u64) -> String {
    let categories = [
        (ContextCategory::SystemPrompt, analysis.system_prompt_tokens),
        (
            ContextCategory::ToolDefinitions,
            analysis.tool_definitions_tokens,
        ),
        (
            ContextCategory::ConversationHistory,
            analysis.conversation_history_tokens,
        ),
        (ContextCategory::ToolResults, analysis.tool_results_tokens),
        (ContextCategory::Attachments, analysis.attachments_tokens),
    ];

    let mut lines = Vec::new();
    let usage_pct = if context_limit > 0 {
        analysis.total_tokens as f64 / context_limit as f64 * 100.0
    } else {
        0.0
    };

    lines.push(format!(
        "Context window: ~{:.0}K / {:.0}K tokens ({:.1}%)",
        analysis.total_tokens as f64 / 1000.0,
        context_limit as f64 / 1000.0,
        usage_pct
    ));
    lines.push(String::new());

    let bar_width = 40usize;
    for (cat, tokens) in &categories {
        if *tokens == 0 {
            continue;
        }
        let pct = analysis.category_pct(*cat);
        let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
        let bar = "█".repeat(filled) + &"░".repeat(bar_width - filled);
        lines.push(format!(
            "{:<24} [{bar}] {:.1}% (~{:.0}K)",
            cat.label(),
            pct,
            *tokens as f64 / 1000.0,
        ));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use claurst_core::types::{Message, MessageContent, Role};

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: MessageContent::Text(text.to_string()),
            uuid: None,
            cost: None,
        }
    }

    #[test]
    fn analyze_basic() {
        let msgs = vec![
            text_msg(Role::User, "Hello"),
            text_msg(Role::Assistant, "Hi there"),
        ];
        let analysis = analyze_context(Some("You are helpful."), None, &msgs);
        assert!(analysis.total_tokens > 0);
        assert!(analysis.conversation_history_tokens > 0);
        assert!(analysis.compressibility >= 0.0 && analysis.compressibility <= 1.0);
    }

    #[test]
    fn suggest_none_for_low_usage() {
        let analysis = ContextAnalysis {
            total_tokens: 10_000,
            conversation_history_tokens: 10_000,
            ..Default::default()
        };
        let strategy = suggest_compaction(&analysis, 200_000);
        assert!(matches!(strategy, CompactionStrategy::None));
    }

    #[test]
    fn format_ctx_viz_basic() {
        let analysis = ContextAnalysis {
            total_tokens: 50_000,
            conversation_history_tokens: 30_000,
            tool_results_tokens: 20_000,
            ..Default::default()
        };
        let output = format_ctx_viz(&analysis, 200_000);
        assert!(output.contains("Context window"));
        assert!(output.contains("Conversation history"));
    }
}
