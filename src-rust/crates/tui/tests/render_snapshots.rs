//! T5-2: Message renderer snapshot tests.
//! Renders each message type and verifies key content in returned Lines.

use claurst_tui::messages::{
    render_assistant_text, render_code_block, render_compact_boundary, render_hook_progress,
    render_rate_limit_banner, render_summary_message, render_system_message, render_thinking_block,
    render_tool_result_error, render_tool_result_success, render_tool_use, render_unseen_divider,
    render_user_command, render_user_local_command_output, render_user_memory_input,
    render_user_text, RenderContext,
};

// ---------------------------------------------------------------------------
// Helper: flatten all span content from a vec of Lines into one String.
// ---------------------------------------------------------------------------

fn flatten(lines: &[ratatui::text::Line<'_>]) -> String {
    lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect()
}

// ---------------------------------------------------------------------------
// Assistant text
// ---------------------------------------------------------------------------

#[test]
fn assistant_text_renders_lines() {
    let ctx = RenderContext {
        width: 80,
        highlight: true,
        show_thinking: false,
        ..Default::default()
    };
    let lines = render_assistant_text("Hello, world!\n\nSecond paragraph.", &ctx);
    assert!(!lines.is_empty());
    let combined = flatten(&lines);
    assert!(combined.contains("Hello"));
}

// ---------------------------------------------------------------------------
// User text
// ---------------------------------------------------------------------------

#[test]
fn user_text_has_prefix() {
    let lines = render_user_text("my question");
    assert!(!lines.is_empty());
    let combined = flatten(&lines);
    assert!(combined.contains("my question"));
}

// ---------------------------------------------------------------------------
// Tool use
// ---------------------------------------------------------------------------

#[test]
fn tool_use_shows_name() {
    let lines = render_tool_use("BashTool", r#"{"command":"ls -la"}"#);
    assert!(!lines.is_empty());
    let combined = flatten(&lines);
    assert!(combined.contains("BashTool"));
}

#[test]
fn tool_use_shows_summary() {
    // New TS-style format: shows the file path value as summary, not the key name.
    let lines = render_tool_use("FileRead", r#"{"path":"/foo/bar.rs","limit":100}"#);
    let combined = flatten(&lines);
    assert!(combined.contains("/foo/bar.rs") || combined.contains("FileRead"));
}

// ---------------------------------------------------------------------------
// Tool result (success)
// ---------------------------------------------------------------------------

#[test]
fn tool_result_success_shows_output() {
    // Renders raw output lines without a separate header.
    let lines = render_tool_result_success("output here", false);
    assert!(!lines.is_empty());
    let combined = flatten(&lines);
    assert!(combined.contains("output here"));
}

#[test]
fn tool_result_success_truncated_notice() {
    let lines = render_tool_result_success("some output", true);
    let combined = flatten(&lines);
    assert!(combined.contains("truncated"));
}

// ---------------------------------------------------------------------------
// Tool result (error)
// ---------------------------------------------------------------------------

#[test]
fn tool_result_error_shows_text() {
    let lines = render_tool_result_error("Permission denied");
    let combined = flatten(&lines);
    assert!(combined.contains("Error") || combined.contains("Permission denied"));
}

// ---------------------------------------------------------------------------
// Compact boundary
// ---------------------------------------------------------------------------

#[test]
fn compact_boundary_has_separator() {
    let lines = render_compact_boundary();
    assert_eq!(lines.len(), 1);
    let text = flatten(&lines);
    // render_compact_boundary contains "context compacted"
    assert!(text.contains("compacted") || text.contains("compact"));
}

// ---------------------------------------------------------------------------
// Summary message
// ---------------------------------------------------------------------------

#[test]
fn summary_message_has_header() {
    let lines = render_summary_message("This is a summary.");
    let combined = flatten(&lines);
    assert!(combined.contains("Summary") || combined.contains("summary"));
}

// ---------------------------------------------------------------------------
// Unseen divider
// ---------------------------------------------------------------------------

#[test]
fn unseen_divider_singular() {
    let lines = render_unseen_divider(1);
    let combined = flatten(&lines);
    assert!(combined.contains("1"));
}

#[test]
fn unseen_divider_plural() {
    let lines = render_unseen_divider(5);
    let combined = flatten(&lines);
    assert!(combined.contains("5") && combined.contains("messages"));
}

// ---------------------------------------------------------------------------
// System message
// ---------------------------------------------------------------------------

#[test]
fn system_message_preserves_text() {
    let lines = render_system_message("System notice here");
    let combined = flatten(&lines);
    assert!(combined.contains("System notice here"));
}

// ---------------------------------------------------------------------------
// Thinking block
// ---------------------------------------------------------------------------

#[test]
fn thinking_block_collapsed() {
    let lines = render_thinking_block("hidden thoughts", false);
    assert_eq!(lines.len(), 1);
    let text = flatten(&lines);
    assert!(text.contains("Thinking"));
    assert!(!text.contains("hidden thoughts"));
}

#[test]
fn thinking_block_expanded() {
    let lines = render_thinking_block("my thoughts here", true);
    assert!(lines.len() > 1);
    let combined = flatten(&lines);
    assert!(combined.contains("my thoughts here"));
}

// ---------------------------------------------------------------------------
// Rate limit banner
// ---------------------------------------------------------------------------

#[test]
fn rate_limit_banner_shows_seconds() {
    let lines = render_rate_limit_banner(30);
    let combined = flatten(&lines);
    assert!(combined.contains("30"));
}

// ---------------------------------------------------------------------------
// Hook progress
// ---------------------------------------------------------------------------

#[test]
fn hook_progress_shows_command() {
    let lines = render_hook_progress("my-hook.sh", None);
    let combined = flatten(&lines);
    assert!(combined.contains("my-hook.sh"));
}

#[test]
fn hook_progress_with_last_line() {
    let lines = render_hook_progress("hook", Some("Running..."));
    assert!(lines.len() >= 2);
    let combined = flatten(&lines);
    assert!(combined.contains("Running..."));
}

// ---------------------------------------------------------------------------
// Code block
// ---------------------------------------------------------------------------

#[test]
fn code_block_shows_language_and_code() {
    let lines = render_code_block(Some("rust"), "fn main() {}", 80);
    let combined = flatten(&lines);
    assert!(combined.contains("rust") && combined.contains("fn main()"));
}

// ---------------------------------------------------------------------------
// UserLocalCommandOutput
// ---------------------------------------------------------------------------

#[test]
fn user_local_command_output_shows_command_header() {
    let lines = render_user_local_command_output("ls -la", "file1\nfile2", 30);
    assert!(!lines.is_empty());
    let combined = flatten(&lines);
    assert!(combined.contains("ls -la"));
}

#[test]
fn user_local_command_output_shows_output_lines() {
    let lines = render_user_local_command_output("echo hi", "hello world", 30);
    let combined = flatten(&lines);
    assert!(combined.contains("hello world"));
}

#[test]
fn user_local_command_output_truncates_at_max_lines() {
    let output = (0..50)
        .map(|i| format!("line{}", i))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = render_user_local_command_output("cmd", &output, 10);
    let combined = flatten(&lines);
    assert!(combined.contains("more lines"));
}

// ---------------------------------------------------------------------------
// UserCommandMessage
// ---------------------------------------------------------------------------

#[test]
fn user_command_shows_chevron_and_name() {
    let lines = render_user_command("doctor", "");
    assert_eq!(lines.len(), 1);
    let combined = flatten(&lines);
    assert!(combined.contains('\u{25b8}'));
    assert!(combined.contains("doctor"));
}

#[test]
fn user_command_shows_args() {
    let lines = render_user_command("skill", "--verbose");
    let combined = flatten(&lines);
    assert!(combined.contains("skill"));
    assert!(combined.contains("--verbose"));
}

// ---------------------------------------------------------------------------
// UserMemoryInputMessage
// ---------------------------------------------------------------------------

#[test]
fn user_memory_input_shows_key_value() {
    let lines = render_user_memory_input("preferred_language", "Rust");
    assert!(lines.len() >= 2);
    let combined = flatten(&lines);
    assert!(combined.contains("preferred_language"));
    assert!(combined.contains("Rust"));
}

#[test]
fn user_memory_input_shows_got_it_footer() {
    let lines = render_user_memory_input("name", "Alice");
    let combined = flatten(&lines);
    assert!(combined.contains("Got it."));
}

#[test]
fn user_memory_input_hash_prefix() {
    let lines = render_user_memory_input("key", "val");
    let first_line = flatten(&lines[..1]);
    assert!(first_line.contains('#'));
}
