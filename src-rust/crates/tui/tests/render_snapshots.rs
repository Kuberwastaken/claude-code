//! T5-2: Message renderer snapshot tests.
//! Renders each message type and verifies key content in returned Lines.

use claurst_tui::messages::{
    render_assistant_text, render_code_block, render_compact_boundary, render_hook_progress,
    render_rate_limit_banner, render_summary_message, render_system_message, render_thinking_block,
    render_tool_result_error, render_tool_result_success, render_tool_use, render_unseen_divider,
    render_user_command, render_user_local_command_output, render_user_memory_input,
    render_user_text, RenderContext,
};
use claurst_tui::timeline::{Timeline, TimelineStatus};

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

// ---------------------------------------------------------------------------
// Timeline rendering contracts
// ---------------------------------------------------------------------------

fn timeline_status_label(status: TimelineStatus) -> &'static str {
    match status {
        TimelineStatus::Running => "running",
        TimelineStatus::Done => "done",
        TimelineStatus::Error => "error",
        TimelineStatus::Cancelled => "cancelled",
    }
}

fn timeline_metric(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn timeline_cost(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.4}"))
        .unwrap_or_else(|| "-".to_string())
}

fn render_timeline_contract_snapshot(timeline: &Timeline, visible: bool, expanded: bool) -> String {
    if !visible {
        return "timeline hidden".to_string();
    }

    let mut lines = vec!["Timeline".to_string()];
    for (idx, row) in timeline.rows.iter().enumerate() {
        let marker = if idx == timeline.selected_idx {
            '>'
        } else {
            ' '
        };
        let duration = row
            .duration_ms()
            .map(|value| format!("{value}ms"))
            .unwrap_or_else(|| "--".to_string());
        lines.push(format!(
            "{marker} {status:<9} {title:<18} {duration:<5} in:{input:<2} out:{output:<2} cost:{cost}",
            status = timeline_status_label(row.status),
            title = row.title,
            input = timeline_metric(row.token_delta_input),
            output = timeline_metric(row.token_delta_output),
            cost = timeline_cost(row.cost_delta_usd),
        ));
    }

    if expanded {
        if let Some(row) = timeline.selected_row() {
            lines.push(String::new());
            lines.push(format!("Selected: {}", row.title));
            lines.push(format!("Preview: {}", row.detail_preview));
            lines.push("Details:".to_string());
            for detail_line in row.expandable_details.lines() {
                lines.push(format!("  {detail_line}"));
            }
        }
    }

    lines.join("\n")
}

fn make_timeline_fixture() -> Timeline {
    let mut timeline = Timeline::new(8);
    timeline.add_turn_summary(
        "turn-1",
        "Bootstrap",
        100,
        118,
        "config loaded",
        "Loaded settings.json\nSelected provider: anthropic",
        Some(24),
        Some(9),
        Some(0.0125),
    );
    timeline.add_status_note(
        "status-1",
        "Summarize diff",
        140,
        TimelineStatus::Cancelled,
        "stopped by user",
        "User pressed Ctrl+C before model response.",
    );
    timeline.add_running_tool(
        "tool-1",
        "Write patch",
        170,
        "editing render.rs",
        "Applying patch to render.rs",
    );
    timeline.add_running_tool(
        "tool-2",
        "Run cargo test",
        220,
        "cargo test failed in tui crate",
        "error[E0599]: no method named `timeline`\nhelp: add App state and renderer wiring",
    );
    timeline.finish_tool(
        "tool-2",
        350,
        TimelineStatus::Error,
        "cargo test failed in tui crate",
        "error[E0599]: no method named `timeline`\nhelp: add App state and renderer wiring",
        Some(31),
        Some(0),
        Some(0.0410),
    );
    timeline.set_selected_idx(3);
    timeline
}

#[test]
fn timeline_hidden_snapshot() {
    let timeline = make_timeline_fixture();
    let snapshot = render_timeline_contract_snapshot(&timeline, false, false);
    assert_eq!(snapshot, "timeline hidden");
}

#[test]
fn timeline_visible_snapshot_with_mixed_statuses_and_selected_row() {
    let timeline = make_timeline_fixture();
    let snapshot = render_timeline_contract_snapshot(&timeline, true, false);
    assert_eq!(
        snapshot,
        "Timeline\n\
          done      Bootstrap          18ms  in:24 out:9  cost:0.0125\n\
          cancelled Summarize diff     0ms   in:-  out:-  cost:-\n\
          running   Write patch        --    in:-  out:-  cost:-\n\
        > error     Run cargo test     130ms in:31 out:0  cost:0.0410"
    );
}

#[test]
fn timeline_expanded_detail_snapshot() {
    let timeline = make_timeline_fixture();
    let snapshot = render_timeline_contract_snapshot(&timeline, true, true);
    assert_eq!(
        snapshot,
        "Timeline\n\
          done      Bootstrap          18ms  in:24 out:9  cost:0.0125\n\
          cancelled Summarize diff     0ms   in:-  out:-  cost:-\n\
          running   Write patch        --    in:-  out:-  cost:-\n\
        > error     Run cargo test     130ms in:31 out:0  cost:0.0410\n\
        \n\
        Selected: Run cargo test\n\
        Preview: cargo test failed in tui crate\n\
        Details:\n\
          error[E0599]: no method named `timeline`\n\
          help: add App state and renderer wiring"
    );
}
