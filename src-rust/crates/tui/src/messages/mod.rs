//! Message type renderers for the TUI.
//! Mirrors src/components/messages/ and src/components/Messages.tsx.
//!
//! Each message type has a dedicated render function. The top-level
//! `render_message()` dispatcher routes to the correct renderer based
//! on message content.

use std::collections::HashMap;

use claurst_core::types::{ContentBlock, Message, Role, ToolResultContent};
use crate::app::TurnMetadata;
use crate::kitty_image::render_image;
use crate::transcript_turn::reasoning_heading;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

mod markdown;
pub use markdown::render_markdown;

mod markdown_enhanced;
pub use markdown_enhanced::{
    detect_table, render_table, parse_inline_formatting,
    Table, TableAlignment,
};

/// Context passed to all renderers.
pub struct RenderContext {
    /// Current terminal width (for word-wrap decisions).
    pub width: u16,
    /// Whether syntax highlighting is enabled.
    pub highlight: bool,
    /// Whether to show thinking blocks.
    pub show_thinking: bool,
    /// Maps `tool_use_id` → `tool_name` so ToolResult blocks can dispatch to
    /// the correct specialized renderer (e.g. Bash output vs. generic result).
    pub tool_names: HashMap<String, String>,
    /// Set of thinking block content hashes that are expanded per-block.
    pub expanded_thinking: std::collections::HashSet<u64>,
}

impl Default for RenderContext {
    fn default() -> Self {
        Self {
            width: 80,
            highlight: true,
            show_thinking: false,
            tool_names: HashMap::new(),
            expanded_thinking: std::collections::HashSet::new(),
        }
    }
}

/// A styled line for rendering.
pub type StyledLine<'a> = Line<'a>;

const MAX_USER_PROMPT_DISPLAY_CHARS: usize = 10_000;
const TRUNCATE_USER_PROMPT_HEAD_CHARS: usize = 2_500;
const TRUNCATE_USER_PROMPT_TAIL_CHARS: usize = 2_500;

/// Claude orange: Rgb(215, 119, 87)
const CLAUDE_ORANGE: Color = Color::Rgb(233, 30, 99);
const TRANSCRIPT_USER_BG: Color = Color::Rgb(23, 23, 31);
const TRANSCRIPT_CHIP_BG: Color = Color::Rgb(31, 31, 41);
const TRANSCRIPT_TEXT: Color = Color::Rgb(236, 236, 241);
const TRANSCRIPT_MUTED: Color = Color::Rgb(139, 139, 153);
const TRANSCRIPT_SUBTLE: Color = Color::Rgb(112, 112, 126);

const TOOL_RESULT_MAX_LINES: usize = 30;

/// Accent color for goal-event blocks (warm amber/gold).
const GOAL_ACCENT: Color = Color::Rgb(255, 170, 50);
/// Body text color for goal-event objective display.
const GOAL_BODY: Color = Color::Rgb(215, 180, 110);
/// Muted color for goal continuation turn markers.
const GOAL_MUTED: Color = Color::Rgb(130, 115, 75);

/// Render a code block with optional language label. Uses basic styling
/// since full syntect integration is behind a feature flag.
pub fn render_code_block(lang: Option<&str>, code: &str, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let label = lang.unwrap_or("code");
    // Language label in brackets at the top
    lines.push(Line::from(vec![Span::styled(
        format!("  [{lang_name}]", lang_name = label),
        Style::default()
            .fg(Color::Rgb(150, 150, 150))
            .add_modifier(Modifier::DIM),
    )]));
    lines.push(Line::from(vec![Span::styled(
        "  ┌─────────────────────────────────────────────────".to_string(),
        Style::default().fg(Color::Rgb(100, 100, 100)),
    )]));
    // `2` chars for the leading "  " indent; at least 10 chars of content
    let max_content = (width as usize).saturating_sub(4).max(10);
    for line in code.lines() {
        let display: String = if line.chars().count() > max_content {
            let truncated: String = line.chars().take(max_content.saturating_sub(1)).collect();
            format!("{truncated}\u{2026}")
        } else {
            line.to_string()
        };
        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(Color::Rgb(100, 100, 100))),
            Span::styled(display, Style::default().fg(Color::White)),
        ]));
    }
    lines.push(Line::from(vec![Span::styled(
        "  └─────────────────────────────────────────────────".to_string(),
        Style::default().fg(Color::Rgb(100, 100, 100)),
    )]));
    lines
}

/// Render an assistant text message body.
pub fn render_assistant_text(text: &str, ctx: &RenderContext) -> Vec<Line<'static>> {
    render_markdown(text, ctx.width.saturating_sub(3))
}

/// Render a user text message body.
fn render_user_text_with_ctx(text: &str, ctx: &RenderContext) -> Vec<Line<'static>> {
    let truncated = truncate_user_prompt_text(text);
    render_markdown(&truncated, ctx.width.saturating_sub(3))
}

/// Legacy public helper retained for snapshot tests.
pub fn render_user_text(text: &str) -> Vec<Line<'static>> {
    render_user_text_with_ctx(text, &RenderContext::default())
}

fn indent_line(
    mut line: Line<'static>,
    prefix: &str,
    prefix_style: Style,
    default_fg: Color,
) -> Line<'static> {
    for span in &mut line.spans {
        if span.style.fg.is_none() {
            span.style = span.style.fg(default_fg);
        }
    }

    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::styled(prefix.to_string(), prefix_style));
    spans.extend(line.spans);
    Line::from(spans)
}

fn indent_lines(
    lines: Vec<Line<'static>>,
    prefix: &str,
    prefix_style: Style,
    default_fg: Color,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| indent_line(line, prefix, prefix_style, default_fg))
        .collect()
}

fn apply_block_style(mut line: Line<'static>, width: u16) -> Line<'static> {
    let bg = TRANSCRIPT_USER_BG;
    for span in &mut line.spans {
        if span.style.fg.is_none() {
            span.style = span.style.fg(TRANSCRIPT_TEXT);
        }
        span.style = span.style.bg(bg);
    }

    let mut spans = vec![
        Span::styled("▏", Style::default().fg(CLAUDE_ORANGE).bg(bg)),
        Span::styled(" ", Style::default().bg(bg)),
    ];
    spans.extend(line.spans);

    let used = spans.iter().map(|span| span.content.width()).sum::<usize>();
    if used < width as usize {
        spans.push(Span::styled(
            " ".repeat(width as usize - used),
            Style::default().bg(bg),
        ));
    }

    Line::from(spans)
}

fn empty_block_line(width: u16) -> Line<'static> {
    apply_block_style(Line::from(""), width)
}

fn short_model_name(model: &str) -> String {
    model
        .split_once('/')
        .map(|(_, model)| model)
        .unwrap_or(model)
        .to_string()
}

fn title_case_mode(mode: &str) -> String {
    let mut chars = mode.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn render_attachment_chip(kind: &str, label: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {} ", kind),
            Style::default()
                .fg(Color::Black)
                .bg(CLAUDE_ORANGE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} ", label),
            Style::default().fg(TRANSCRIPT_MUTED).bg(TRANSCRIPT_CHIP_BG),
        ),
    ])
}

fn user_metadata_line(_meta: Option<&TurnMetadata>) -> Option<Line<'static>> {
    // User prompt line has no metadata — mode/model/duration are shown on the
    // assistant footer instead (matching OpenCode's layout).
    None
}

pub fn render_transcript_assistant_meta(meta: Option<&TurnMetadata>, accent: Color) -> Option<Line<'static>> {
    let meta = meta?;

    // Always show at least mode — matches OpenCode's "Build · Model · Duration"
    let mode = meta
        .agent_mode
        .as_deref()
        .filter(|m| !m.is_empty())
        .map(title_case_mode)
        .unwrap_or_else(|| "Build".to_string());

    let mut spans = vec![
        Span::styled(
            "   \u{25a3} ",
            Style::default()
                .fg(accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(mode, Style::default().fg(TRANSCRIPT_TEXT)),
    ];

    if let Some(model) = meta.model_name.as_deref().filter(|m| !m.is_empty()) {
        spans.push(Span::styled(" \u{00b7} ", Style::default().fg(TRANSCRIPT_SUBTLE)));
        spans.push(Span::styled(
            short_model_name(model),
            Style::default().fg(TRANSCRIPT_MUTED),
        ));
    }

    if let Some(duration) = meta.duration.as_deref().filter(|d| !d.is_empty()) {
        spans.push(Span::styled(" \u{00b7} ", Style::default().fg(TRANSCRIPT_SUBTLE)));
        spans.push(Span::styled(
            duration.to_string(),
            Style::default().fg(TRANSCRIPT_MUTED),
        ));
    }

    if meta.interrupted {
        spans.push(Span::styled(" \u{00b7} ", Style::default().fg(TRANSCRIPT_SUBTLE)));
        spans.push(Span::styled(
            "interrupted",
            Style::default().fg(TRANSCRIPT_MUTED),
        ));
    }

    Some(Line::from(spans))
}

pub fn render_transcript_live_text(text: &str, width: u16) -> Vec<Line<'static>> {
    indent_lines(
        render_markdown(text, width.saturating_sub(4)),
        "   ",
        Style::default(),
        TRANSCRIPT_TEXT,
    )
}

pub fn render_transcript_user_message(
    msg: &Message,
    meta: Option<&TurnMetadata>,
    width: u16,
) -> Vec<Line<'static>> {
    // Goal-event messages injected by the /goal machinery render as a compact
    // event block, not as a user input bubble.
    if let Some(ContentBlock::Text { text }) = msg.content_blocks().into_iter().next() {
        if is_goal_event_message(&text) {
            return render_goal_event(&text, width);
        }
    }

    let inner_width = width.saturating_sub(4).max(10);
    let mut lines = Vec::new();
    let mut pending_text = String::new();

    let flush_text = |buffer: &mut String, target: &mut Vec<Line<'static>>| {
        if buffer.is_empty() {
            return;
        }
        target.extend(render_user_text_with_ctx(buffer, &RenderContext {
            width: inner_width,
            ..RenderContext::default()
        }));
        buffer.clear();
    };

    for block in msg.content_blocks() {
        match block {
            ContentBlock::Text { text } => {
                if !pending_text.is_empty() {
                    pending_text.push('\n');
                }
                pending_text.push_str(&text);
            }
            ContentBlock::Image { source } => {
                flush_text(&mut pending_text, &mut lines);
                let label = source
                    .media_type
                    .or(source.url)
                    .unwrap_or_else(|| "pasted image".to_string());
                lines.push(render_attachment_chip("img", label));
            }
            ContentBlock::Document { title, context, source, .. } => {
                flush_text(&mut pending_text, &mut lines);
                let label = title
                    .or(context)
                    .or(source.url)
                    .or(source.media_type)
                    .unwrap_or_else(|| "attached document".to_string());
                lines.push(render_attachment_chip("doc", label));
            }
            ContentBlock::UserLocalCommandOutput { command, output } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(render_user_local_command_output(&command, &output, 30));
            }
            ContentBlock::UserCommand { name, args } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(render_user_command(&name, &args));
            }
            ContentBlock::UserMemoryInput { key, value } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(render_user_memory_input(&key, &value));
            }
            ContentBlock::SystemAPIError { message, retry_secs } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(render_system_api_error(&message, retry_secs));
            }
            ContentBlock::CollapsedReadSearch { tool_name, paths, n_hidden } => {
                flush_text(&mut pending_text, &mut lines);
                let path_refs: Vec<&str> = paths.iter().map(|path| path.as_str()).collect();
                lines.extend(render_collapsed_read_search(&tool_name, &path_refs, n_hidden));
            }
            ContentBlock::TaskAssignment { id, subject, description } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(render_task_assignment(&id, &subject, &description));
            }
            ContentBlock::ToolUse { name, input, .. } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(render_tool_use_inner(&name, &input));
            }
            ContentBlock::ToolResult { tool_use_id: _, content, is_error } => {
                flush_text(&mut pending_text, &mut lines);
                let text = tool_result_text(&content);
                let rendered = if is_error.unwrap_or(false) {
                    render_tool_result_error(&text)
                } else {
                    render_tool_result_success(&text, false)
                };
                lines.extend(rendered);
            }
            ContentBlock::Thinking { thinking, .. } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(render_transcript_reasoning_block(&thinking, false, inner_width));
            }
            ContentBlock::RedactedThinking { .. } => {
                flush_text(&mut pending_text, &mut lines);
                lines.push(Line::from(vec![Span::styled(
                    "Thinking hidden".to_string(),
                    Style::default()
                        .fg(TRANSCRIPT_MUTED)
                        .add_modifier(Modifier::ITALIC),
                )]));
            }
        }
    }
    flush_text(&mut pending_text, &mut lines);

    if let Some(meta_line) = user_metadata_line(meta) {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(meta_line);
    }

    if lines.is_empty() {
        lines.push(Line::from(""));
    }

    let mut wrapped = Vec::with_capacity(lines.len() + 2);
    wrapped.push(empty_block_line(width));
    wrapped.extend(lines.into_iter().map(|line| apply_block_style(line, width)));
    wrapped.push(empty_block_line(width));
    wrapped
}

pub fn render_transcript_reasoning_block(
    text: &str,
    expanded: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let heading = reasoning_heading(text).unwrap_or_else(|| "Thinking".to_string());
    let chevron = if expanded { "▼" } else { "▶" };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {} Thinking: ", chevron),
            Style::default()
                .fg(TRANSCRIPT_MUTED)
                .add_modifier(Modifier::ITALIC),
        ),
        Span::styled(
            heading,
            Style::default()
                .fg(TRANSCRIPT_SUBTLE)
                .add_modifier(Modifier::ITALIC),
        ),
    ]));

    if expanded {
        let rendered = render_markdown(text, width.saturating_sub(6));
        lines.extend(indent_lines(
            rendered,
            "    ",
            Style::default(),
            TRANSCRIPT_MUTED,
        ));
    }

    lines
}

/// Render the thinking content body (without header) for live streaming display.
pub fn render_thinking_live_content(text: &str, width: u16) -> Vec<Line<'static>> {
    let rendered = render_markdown(text, width.saturating_sub(6));
    indent_lines(rendered, "    ", Style::default(), TRANSCRIPT_MUTED)
}

/// Returns lines for each content block with an optional thinking hash tag.
/// The hash is `Some(hash)` only for the header line of a Thinking block,
/// enabling click-to-expand in the TUI.
pub fn render_transcript_assistant_message_tagged(
    msg: &Message,
    ctx: &RenderContext,
) -> Vec<(Line<'static>, Option<u64>)> {
    let mut out: Vec<(Line<'static>, Option<u64>)> = Vec::new();
    let mut pending_text = String::new();

    let flush_text = |buffer: &mut String, target: &mut Vec<(Line<'static>, Option<u64>)>, width: u16| {
        if buffer.is_empty() {
            return;
        }
        for line in render_transcript_live_text(buffer, width) {
            target.push((line, None));
        }
        buffer.clear();
    };

    for block in msg.content_blocks() {
        match block {
            ContentBlock::Text { text } => {
                if !pending_text.is_empty() {
                    pending_text.push('\n');
                }
                pending_text.push_str(&text);
            }
            ContentBlock::Thinking { thinking, .. } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                let thinking_hash = {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut h = DefaultHasher::new();
                    thinking.hash(&mut h);
                    h.finish()
                };
                let expanded = ctx.show_thinking || ctx.expanded_thinking.contains(&thinking_hash);
                let block_lines = render_transcript_reasoning_block(&thinking, expanded, ctx.width);
                for (i, line) in block_lines.into_iter().enumerate() {
                    // Tag only the header line (index 0) with the hash so it's clickable.
                    out.push((line, if i == 0 { Some(thinking_hash) } else { None }));
                }
            }
            ContentBlock::RedactedThinking { .. } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                out.push((
                    Line::from(vec![Span::styled(
                        "  Thinking hidden".to_string(),
                        Style::default()
                            .fg(TRANSCRIPT_MUTED)
                            .add_modifier(Modifier::ITALIC),
                    )]),
                    None,
                ));
            }
            ContentBlock::ToolUse { name, input, .. } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                for line in indent_lines(
                    render_tool_use_inner(&name, &input),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ) {
                    out.push((line, None));
                }
            }
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                let text = tool_result_text(&content);
                let tool_name = ctx.tool_names.get(&tool_use_id).map(|name| name.as_str());
                let rendered = if is_error.unwrap_or(false) {
                    render_tool_result_error(&text)
                } else {
                    match tool_name {
                        Some("Bash") | Some("PowerShell") => render_bash_output_block(&text, TOOL_RESULT_MAX_LINES),
                        Some("Read") => render_file_read_result(&text),
                        Some("Edit") => render_file_op_result(false),
                        Some("Write") => render_file_op_result(true),
                        _ => render_tool_result_success(&text, false),
                    }
                };
                for line in indent_lines(rendered, "   ", Style::default(), TRANSCRIPT_TEXT) {
                    out.push((line, None));
                }
            }
            ContentBlock::Image { source } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                let label = render_image(&source).unwrap_or_else(|| {
                    source
                        .media_type
                        .or(source.url)
                        .unwrap_or_else(|| "assistant image".to_string())
                });
                for line in indent_lines(
                    vec![render_attachment_chip("img", label)],
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ) {
                    out.push((line, None));
                }
            }
            ContentBlock::Document { title, context, source, .. } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                let label = title
                    .or(context)
                    .or(source.url)
                    .or(source.media_type)
                    .unwrap_or_else(|| "attached document".to_string());
                for line in indent_lines(
                    vec![render_attachment_chip("doc", label)],
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ) {
                    out.push((line, None));
                }
            }
            ContentBlock::UserLocalCommandOutput { command, output } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                for line in indent_lines(
                    render_user_local_command_output(&command, &output, 30),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ) {
                    out.push((line, None));
                }
            }
            ContentBlock::UserCommand { name, args } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                for line in indent_lines(
                    render_user_command(&name, &args),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ) {
                    out.push((line, None));
                }
            }
            ContentBlock::UserMemoryInput { key, value } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                for line in indent_lines(
                    render_user_memory_input(&key, &value),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ) {
                    out.push((line, None));
                }
            }
            ContentBlock::SystemAPIError { message, retry_secs } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                for line in indent_lines(
                    render_system_api_error(&message, retry_secs),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ) {
                    out.push((line, None));
                }
            }
            ContentBlock::CollapsedReadSearch { tool_name, paths, n_hidden } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                let path_refs: Vec<&str> = paths.iter().map(|path| path.as_str()).collect();
                for line in indent_lines(
                    render_collapsed_read_search(&tool_name, &path_refs, n_hidden),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ) {
                    out.push((line, None));
                }
            }
            ContentBlock::TaskAssignment { id, subject, description } => {
                flush_text(&mut pending_text, &mut out, ctx.width);
                for line in indent_lines(
                    render_task_assignment(&id, &subject, &description),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ) {
                    out.push((line, None));
                }
            }
        }
    }

    flush_text(&mut pending_text, &mut out, ctx.width);
    out
}

pub fn render_transcript_assistant_message(
    msg: &Message,
    ctx: &RenderContext,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut pending_text = String::new();

    let flush_text = |buffer: &mut String, target: &mut Vec<Line<'static>>| {
        if buffer.is_empty() {
            return;
        }
        target.extend(render_transcript_live_text(buffer, ctx.width));
        buffer.clear();
    };

    for block in msg.content_blocks() {
        match block {
            ContentBlock::Text { text } => {
                if !pending_text.is_empty() {
                    pending_text.push('\n');
                }
                pending_text.push_str(&text);
            }
            ContentBlock::Thinking { thinking, .. } => {
                flush_text(&mut pending_text, &mut lines);
                let thinking_hash = {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut h = DefaultHasher::new();
                    thinking.hash(&mut h);
                    h.finish()
                };
                let expanded = ctx.show_thinking || ctx.expanded_thinking.contains(&thinking_hash);
                lines.extend(render_transcript_reasoning_block(&thinking, expanded, ctx.width));
            }
            ContentBlock::RedactedThinking { .. } => {
                flush_text(&mut pending_text, &mut lines);
                lines.push(Line::from(vec![Span::styled(
                    "  Thinking hidden".to_string(),
                    Style::default()
                        .fg(TRANSCRIPT_MUTED)
                        .add_modifier(Modifier::ITALIC),
                )]));
            }
            ContentBlock::ToolUse { name, input, .. } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(indent_lines(
                    render_tool_use_inner(&name, &input),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ));
            }
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                flush_text(&mut pending_text, &mut lines);
                let text = tool_result_text(&content);
                let tool_name = ctx.tool_names.get(&tool_use_id).map(|name| name.as_str());
                let rendered = if is_error.unwrap_or(false) {
                    render_tool_result_error(&text)
                } else {
                    match tool_name {
                        Some("Bash") | Some("PowerShell") => render_bash_output_block(&text, TOOL_RESULT_MAX_LINES),
                        Some("Read") => render_file_read_result(&text),
                        Some("Edit") => render_file_op_result(false),
                        Some("Write") => render_file_op_result(true),
                        _ => render_tool_result_success(&text, false),
                    }
                };
                lines.extend(indent_lines(
                    rendered,
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ));
            }
            ContentBlock::Image { source } => {
                flush_text(&mut pending_text, &mut lines);
                let label = render_image(&source).unwrap_or_else(|| {
                    source
                        .media_type
                        .or(source.url)
                        .unwrap_or_else(|| "assistant image".to_string())
                });
                lines.extend(indent_lines(
                    vec![render_attachment_chip("img", label)],
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ));
            }
            ContentBlock::Document { title, context, source, .. } => {
                flush_text(&mut pending_text, &mut lines);
                let label = title
                    .or(context)
                    .or(source.url)
                    .or(source.media_type)
                    .unwrap_or_else(|| "attached document".to_string());
                lines.extend(indent_lines(
                    vec![render_attachment_chip("doc", label)],
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ));
            }
            ContentBlock::UserLocalCommandOutput { command, output } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(indent_lines(
                    render_user_local_command_output(&command, &output, 30),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ));
            }
            ContentBlock::UserCommand { name, args } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(indent_lines(
                    render_user_command(&name, &args),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ));
            }
            ContentBlock::UserMemoryInput { key, value } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(indent_lines(
                    render_user_memory_input(&key, &value),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ));
            }
            ContentBlock::SystemAPIError { message, retry_secs } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(indent_lines(
                    render_system_api_error(&message, retry_secs),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ));
            }
            ContentBlock::CollapsedReadSearch { tool_name, paths, n_hidden } => {
                flush_text(&mut pending_text, &mut lines);
                let path_refs: Vec<&str> = paths.iter().map(|path| path.as_str()).collect();
                lines.extend(indent_lines(
                    render_collapsed_read_search(&tool_name, &path_refs, n_hidden),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ));
            }
            ContentBlock::TaskAssignment { id, subject, description } => {
                flush_text(&mut pending_text, &mut lines);
                lines.extend(indent_lines(
                    render_task_assignment(&id, &subject, &description),
                    "   ",
                    Style::default(),
                    TRANSCRIPT_TEXT,
                ));
            }
        }
    }

    flush_text(&mut pending_text, &mut lines);
    lines
}

/// Extract a short one-line summary of a tool call's arguments.
/// Used by both the transcript renderer and live tool block renderer in render.rs.
fn title_case_word(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

pub fn extract_tool_summary(tool_name: &str, input: &serde_json::Value) -> String {
    fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
        input.get(key).and_then(|v| v.as_str()).unwrap_or("")
    }
    fn truncate(s: &str, n: usize) -> String {
        let s = s.trim();
        let chars: Vec<char> = s.chars().collect();
        if chars.len() > n {
            format!("{}\u{2026}", chars[..n].iter().collect::<String>())
        } else {
            s.to_string()
        }
    }
    match tool_name.to_ascii_lowercase().as_str() {
        "bash" | "powershell" => {
            let cmd = str_field(input, "command");
            truncate(cmd.lines().next().unwrap_or(""), 60)
        }
        "read" => truncate(str_field(input, "file_path"), 60),
        "edit" => truncate(str_field(input, "file_path"), 60),
        "write" => truncate(str_field(input, "file_path"), 60),
        "glob" => truncate(str_field(input, "pattern"), 60),
        "grep" => truncate(str_field(input, "pattern"), 60),
        "webfetch" => truncate(str_field(input, "url"), 60),
        "websearch" => truncate(str_field(input, "query"), 60),
        "task" | "agent" => {
            let task = str_field(input, "task");
            let task = if task.is_empty() { str_field(input, "description") } else { task };
            truncate(task.lines().next().unwrap_or(""), 60)
        }
        _ => {
            // First string value from the input object
            if let Some(obj) = input.as_object() {
                for v in obj.values() {
                    if let Some(s) = v.as_str() {
                        return truncate(s, 60);
                    }
                }
            }
            String::new()
        }
    }
}

pub fn subagent_title(input: &serde_json::Value) -> String {
    let label = input
        .get("subagent_type")
        .and_then(|value| value.as_str())
        .map(title_case_word)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "General".to_string());
    format!("{label} agent")
}

/// Render a compact tool-use block that matches the newer transcript language.
pub fn render_tool_use(tool_name: &str, input_json: &str) -> Vec<Line<'static>> {
    let input: serde_json::Value =
        serde_json::from_str(input_json).unwrap_or(serde_json::Value::Null);
    render_tool_use_inner(tool_name, &input)
}

fn render_tool_use_inner(tool_name: &str, input: &serde_json::Value) -> Vec<Line<'static>> {
    let summary = extract_tool_summary(tool_name, input);
    let mut lines = Vec::new();
    let title = match tool_name.to_ascii_lowercase().as_str() {
        "bash" | "powershell" => "Running command",
        "read" => "Reading file",
        "write" => "Writing file",
        "edit" => "Editing file",
        "glob" | "list" => "Listing files",
        "grep" => "Searching code",
        "webfetch" => "Fetching page",
        "websearch" => "Searching web",
        "task" | "agent" => return {
            let mut task_lines = Vec::new();
            task_lines.push(Line::from(vec![
                Span::styled("  ~ ".to_string(), Style::default().fg(CLAUDE_ORANGE)),
                Span::styled(
                    subagent_title(input),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
            ]));
            if !summary.is_empty() {
                task_lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(summary, Style::default().fg(TRANSCRIPT_MUTED)),
                ]));
            }
            task_lines
        },
        _ => tool_name,
    };

    lines.push(Line::from(vec![
        Span::styled("  ~ ".to_string(), Style::default().fg(CLAUDE_ORANGE)),
        Span::styled(
            title.to_string(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
    ]));
    if !summary.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(summary, Style::default().fg(TRANSCRIPT_MUTED)),
        ]));
    }

    if matches!(tool_name.to_ascii_lowercase().as_str(), "bash" | "powershell") {
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        for (i, cmd_line) in command.lines().enumerate() {
            if i >= 2 {
                break;
            }
            let display: String = cmd_line.chars().take(160).collect();
            let display = if cmd_line.chars().count() > 160 {
                format!("{}\u{2026}", display)
            } else {
                display
            };
            lines.push(Line::from(vec![
                Span::styled(
                    "    $ ".to_string(),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    display,
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }

    lines
}

/// Render a file-read tool result: `Read N lines` summary.
fn render_file_read_result(output: &str) -> Vec<Line<'static>> {
    let n = output.lines().count();
    vec![Line::from(vec![Span::styled(
        format!("  Read {} line{}", n, if n == 1 { "" } else { "s" }),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    )])]
}

/// Render a file-edit/write tool result: `Updated file` or `Created file`.
fn render_file_op_result(is_create: bool) -> Vec<Line<'static>> {
    let action = if is_create { "Created" } else { "Updated" };
    vec![Line::from(vec![Span::styled(
        format!("  {} file", action),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    )])]
}

/// Render a tool result (success variant) — generic fallback.
pub fn render_tool_result_success(output: &str, truncated: bool) -> Vec<Line<'static>> {
    let total_lines = output.lines().count();
    let mut lines: Vec<Line<'static>> = output
        .lines()
        .enumerate()
        .take_while(|(i, _)| *i < TOOL_RESULT_MAX_LINES)
        .map(|(_, l)| {
            Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::raw(l.to_string()),
            ])
        })
        .collect();
    if total_lines > TOOL_RESULT_MAX_LINES {
        let remaining = total_lines - TOOL_RESULT_MAX_LINES;
        lines.push(Line::from(vec![Span::styled(
            format!("  ... {} more lines", remaining),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        )]));
    }
    if truncated {
        lines.push(Line::from(vec![Span::styled(
            "  ... output truncated".to_string(),
            Style::default().fg(Color::DarkGray),
        )]));
    }
    lines
}

/// Render a tool result (error variant).
pub fn render_tool_result_error(error: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    // Use orange instead of red for color-blind accessibility
    let error_color = Color::Rgb(255, 140, 0);  // Orange
    lines.push(Line::from(vec![Span::styled(
        "  Error",
        Style::default().fg(error_color).add_modifier(Modifier::BOLD),
    )]));
    for line in error.lines().take(10) {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(line.to_string(), Style::default().fg(error_color)),
        ]));
    }
    lines
}

/// Render a cancelled tool result.
pub fn render_tool_result_cancelled(tool_name: &str) -> Vec<Line<'static>> {
    vec![Line::from(vec![Span::styled(
        format!("  \u{2717} {} \u{2014} cancelled", tool_name),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    )])]
}

/// Render a rejected (interrupted) tool result with reason.
pub fn render_tool_result_rejected(tool_name: &str, reason: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![Span::styled(
            format!("  \u{2717} {} \u{2014} interrupted", tool_name),
            Style::default().fg(CLAUDE_ORANGE),
        )]),
        Line::from(vec![Span::styled(
            format!("    {}", reason),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )]),
    ]
}

/// Render an attachment message (skill listing, agent listing, MCP instructions, hook results, etc.)
pub fn render_attachment_message(kind_label: &str, content: &str, width: u16) -> Vec<Line<'static>> {
    // Reserve space for the "  [label] " prefix and a small margin.
    let prefix_len = kind_label.len() + 6; // "  [label] "
    let preview_max = (width as usize).saturating_sub(prefix_len).max(20).min(120);
    let preview: String = content.chars().take(preview_max).collect();
    let preview = if content.chars().count() > preview_max {
        format!("{preview}\u{2026}")
    } else {
        preview
    };
    vec![Line::from(vec![
        Span::styled(
            format!("  [{kind_label}] "),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(preview, Style::default().fg(Color::White)),
    ])]
}

/// Render an advisor status line.
pub fn render_advisor_message(
    is_loading: bool,
    model_name: Option<&str>,
) -> Vec<Line<'static>> {
    let model_suffix = model_name
        .map(|m| format!(" ({})", m))
        .unwrap_or_default();
    if is_loading {
        vec![Line::from(vec![Span::styled(
            format!("  \u{25cc} Advising\u{2026}{}", model_suffix),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        )])]
    } else {
        vec![Line::from(vec![Span::styled(
            format!("  \u{2713} Advisor reviewed{}", model_suffix),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        )])]
    }
}

/// Render an agent notification line.
pub fn render_agent_notification(agent_name: &str, message: &str) -> Vec<Line<'static>> {
    render_agent_notification_with_severity(agent_name, message, "info")
}

/// Render an agent notification line with a severity level.
/// severity: "info" (cyan), "warn" (yellow), "error" (red).
pub fn render_agent_notification_with_severity(
    agent_name: &str,
    message: &str,
    severity: &str,
) -> Vec<Line<'static>> {
    let color = match severity {
        "warn" => Color::Yellow,
        "error" => Color::Red,
        _ => Color::Cyan,
    };
    vec![Line::from(vec![
        Span::styled(
            format!("  [{}] ", agent_name),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(message.to_string(), Style::default().fg(color)),
    ])]
}

/// Render a session shutdown message.
pub fn render_shutdown_message(reason: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![Span::styled(
            "\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}\u{2014}",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        )]),
        Line::from(vec![Span::styled(
            format!(
                "  \u{2014} Session ended: {} \u{2014}",
                reason
            ),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        )]),
    ]
}

/// Render a bash command input line with a green `$ ` prefix.
pub fn render_bash_input_line(command: &str) -> Vec<Line<'static>> {
    vec![Line::from(vec![
        Span::styled(
            "  $ ".to_string(),
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            command.to_string(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
    ])]
}

/// Render bash output lines truncated to `max_lines` with an overflow indicator.
pub fn render_bash_output_block(output: &str, max_lines: usize) -> Vec<Line<'static>> {
    let total = output.lines().count();
    let mut lines: Vec<Line<'static>> = output
        .lines()
        .take(max_lines)
        .map(|l| {
            Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(l.to_string(), Style::default().fg(Color::Gray)),
            ])
        })
        .collect();
    if total > max_lines {
        let remaining = total - max_lines;
        lines.push(Line::from(vec![Span::styled(
            format!("  ... {} more lines", remaining),
            Style::default().fg(Color::DarkGray),
        )]));
    }
    lines
}

/// Render a plan with numbered steps.
pub fn render_plan_steps(steps: &[String]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        "  Plan:".to_string(),
        Style::default().fg(CLAUDE_ORANGE).add_modifier(Modifier::BOLD),
    )]));
    for (i, step) in steps.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {}. ", i + 1),
                Style::default().fg(CLAUDE_ORANGE),
            ),
            Span::styled(step.clone(), Style::default().fg(Color::White)),
        ]));
    }
    lines
}

/// Render a plan approval prompt.
pub fn render_plan_approval_prompt() -> Vec<Line<'static>> {
    vec![Line::from(vec![
        Span::styled(
            "  Approve this plan? ".to_string(),
            Style::default().fg(CLAUDE_ORANGE).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "[y] yes  [n] no  [e] edit".to_string(),
            Style::default().fg(Color::White),
        ),
    ])]
}

/// Render a "compact boundary" separator.
pub fn render_compact_boundary() -> Vec<Line<'static>> {
    vec![Line::from(vec![Span::styled(
        "----------- context compacted -----------",
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
    )])]
}

/// Render a summary message (post-compact).
pub fn render_summary_message(text: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        "Summary",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )]));
    for line in text.lines() {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(line.to_string(), Style::default().fg(Color::Gray)),
        ]));
    }
    lines
}

/// Render an unseen divider.
pub fn render_unseen_divider(count: usize) -> Vec<Line<'static>> {
    vec![Line::from(vec![Span::styled(
        format!("---- {} new message{} ----", count, if count == 1 { "" } else { "s" }),
        Style::default().fg(Color::Yellow),
    )])]
}

/// Render a system message (dimmed, italic).
pub fn render_system_message(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .map(|line| {
            Line::from(vec![Span::styled(
                line.to_string(),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )])
        })
        .collect()
}

/// Render a thinking block (collapsible - show header only when collapsed).
pub fn render_thinking_block(text: &str, expanded: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let heading = reasoning_heading(text).unwrap_or_else(|| "Thinking".to_string());
    lines.push(Line::from(vec![
        Span::styled(
            "Thinking: ",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        ),
        Span::styled(
            heading,
            Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
        ),
    ]));
    if expanded {
        for line in text.lines() {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(line.to_string(), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }
    lines
}

/// Render a rate-limit warning banner.
pub fn render_rate_limit_banner(retry_after_secs: u64) -> Vec<Line<'static>> {
    render_rate_limit_with_hint(retry_after_secs, false)
}

/// Render a rate-limit warning banner with optional upgrade hint.
pub fn render_rate_limit_with_hint(retry_after_secs: u64, show_upgrade_hint: bool) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![Span::styled(
            "Rate limit exceeded",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            format!("  Retrying in {}s...", retry_after_secs),
            Style::default().fg(Color::Yellow),
        )]),
    ];
    if show_upgrade_hint {
        lines.push(Line::from(vec![Span::styled(
            "  \u{2192} claude.ai/upgrade for higher limits",
            Style::default().fg(Color::DarkGray),
        )]));
    }
    lines
}

/// Render a hook progress line (grey spinner + command).
pub fn render_hook_progress(command: &str, last_line: Option<&str>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("... ", Style::default().fg(Color::DarkGray)),
        Span::styled(command.to_string(), Style::default().fg(Color::DarkGray)),
    ]));
    if let Some(line) = last_line {
        lines.push(Line::from(vec![Span::styled(
            format!("  {}", line),
            Style::default().fg(Color::DarkGray),
        )]));
    }
    lines
}

fn truncate_user_prompt_text(text: &str) -> String {
    if text.len() <= MAX_USER_PROMPT_DISPLAY_CHARS {
        return text.to_string();
    }

    let head = &text[..TRUNCATE_USER_PROMPT_HEAD_CHARS.min(text.len())];
    let tail_start = text.len().saturating_sub(TRUNCATE_USER_PROMPT_TAIL_CHARS);
    let tail = &text[tail_start..];
    let hidden_lines = text
        .chars()
        .take(TRUNCATE_USER_PROMPT_HEAD_CHARS)
        .filter(|c| *c == '\n')
        .count()
        .saturating_sub(tail.chars().filter(|c| *c == '\n').count());

    format!("{head}\n… +{hidden_lines} lines …\n{tail}")
}

fn prefix_message_lines(
    mut rendered: Vec<Line<'static>>,
    role: &Role,
    width: u16,
) -> Vec<Line<'static>> {
    if rendered.is_empty() {
        return rendered;
    }

    let (prefix, prefix_style, body_style) = match role {
        Role::User => (
            "› ",
            Style::default()
                .fg(Color::Rgb(233, 30, 99))
                .add_modifier(Modifier::BOLD),
            Style::default().fg(Color::White),
        ),
        Role::Assistant => (
            "",
            Style::default(),
            Style::default().fg(Color::White),
        ),
    };

    if !prefix.is_empty() {
        if let Some(first) = rendered.first_mut() {
            let mut spans = Vec::with_capacity(first.spans.len() + 1);
            spans.push(Span::styled(prefix.to_string(), prefix_style));
            spans.extend(first.spans.clone());
            first.spans = spans;
        }
    }

    if *role == Role::User {
        let background = Color::Rgb(52, 52, 52);
        for line in &mut rendered {
            let mut line_width = 0usize;
            for span in &mut line.spans {
                line_width += span.content.width();
                if span.style.fg.is_none() {
                    span.style = body_style;
                }
                span.style = span.style.bg(background);
            }
            let pad = (width as usize).saturating_sub(line_width.min(width as usize));
            if pad > 0 {
                line.spans.push(Span::styled(
                    " ".repeat(pad),
                    Style::default().bg(background),
                ));
            }
        }
    }

    rendered
}

fn flush_text(lines: &mut Vec<Line<'static>>, role: &Role, text: &mut String, ctx: &RenderContext) {
    if text.is_empty() {
        return;
    }

    let rendered = match role {
        Role::User => prefix_message_lines(render_markdown(text, ctx.width), role, ctx.width),
        Role::Assistant => prefix_message_lines(render_assistant_text(text, ctx), role, ctx.width),
    };
    lines.extend(rendered);
    text.clear();
}

fn tool_result_text(content: &ToolResultContent) -> String {
    match content {
        ToolResultContent::Text(text) => text.clone(),
        ToolResultContent::Blocks(blocks) => {
            let joined = blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                    ContentBlock::RedactedThinking { .. } => Some("[redacted thinking]"),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if joined.is_empty() {
                "[structured tool result]".to_string()
            } else {
                joined
            }
        }
    }
}

fn render_attachment_line(kind: &str, label: String) -> Vec<Line<'static>> {
    vec![Line::from(vec![
        Span::styled(
            format!("  {} ", kind),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
        ),
        Span::styled(label, Style::default().fg(Color::DarkGray)),
    ])]
}

pub fn render_message(msg: &Message, ctx: &RenderContext) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut pending_text = String::new();

    for block in msg.content_blocks() {
        match block {
            ContentBlock::Text { text } => {
                if !pending_text.is_empty() {
                    pending_text.push('\n');
                }
                pending_text.push_str(&text);
            }
            ContentBlock::Thinking { thinking, .. } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                // Compute a stable hash of the thinking content for per-block expansion tracking
                let thinking_hash = {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut h = DefaultHasher::new();
                    thinking.hash(&mut h);
                    h.finish()
                };
                let expanded = ctx.show_thinking || ctx.expanded_thinking.contains(&thinking_hash);
                lines.extend(prefix_message_lines(
                    render_thinking_block(&thinking, expanded),
                    &msg.role,
                    ctx.width,
                ));
            }
            ContentBlock::RedactedThinking { .. } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                lines.extend(prefix_message_lines(
                    vec![Line::from(vec![Span::styled(
                        "Thinking redacted",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )])],
                    &msg.role,
                    ctx.width,
                ));
            }
            ContentBlock::ToolUse { id, name, input } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                let rendered = render_tool_use_inner(&name, &input);
                // Silence unused-variable warning on id — kept for symmetry with ToolResult lookup.
                let _ = &id;
                lines.extend(prefix_message_lines(rendered, &msg.role, ctx.width));
            }
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                let text = tool_result_text(&content);
                let tool_name = ctx.tool_names.get(&tool_use_id).map(|s| s.as_str());
                let rendered = if is_error.unwrap_or(false) {
                    render_tool_result_error(&text)
                } else {
                    match tool_name {
                        Some("Bash") | Some("PowerShell") => {
                            render_bash_output_block(&text, TOOL_RESULT_MAX_LINES)
                        }
                        Some("Read") => render_file_read_result(&text),
                        Some("Edit") => render_file_op_result(false),
                        Some("Write") => render_file_op_result(true),
                        _ => render_tool_result_success(&text, false),
                    }
                };
                lines.extend(prefix_message_lines(rendered, &msg.role, ctx.width));
            }
            ContentBlock::Image { source } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                // Attempt Kitty graphics protocol rendering.  When the
                // terminal supports it and the source carries inline base64
                // data, `render_image` emits the APC escape sequence directly
                // to stdout and returns `None` — nothing more to do for this
                // block.  Otherwise it returns a human-readable fallback
                // string that we display as a normal styled line.
                if let Some(label) = render_image(&source) {
                    lines.extend(prefix_message_lines(
                        render_attachment_line("Image", label),
                        &msg.role,
                        ctx.width,
                    ));
                }
            }
            ContentBlock::Document { title, context, source, .. } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                let label = title
                    .or(context)
                    .or(source.url)
                    .or(source.media_type)
                    .unwrap_or_else(|| "attached document".to_string());
                lines.extend(prefix_message_lines(
                    render_attachment_line("Document", label),
                    &msg.role,
                    ctx.width,
                ));
            }
            ContentBlock::UserLocalCommandOutput { command, output } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                lines.extend(render_user_local_command_output(&command, &output, 30));
            }
            ContentBlock::UserCommand { name, args } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                lines.extend(render_user_command(&name, &args));
            }
            ContentBlock::UserMemoryInput { key, value } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                lines.extend(render_user_memory_input(&key, &value));
            }
            ContentBlock::SystemAPIError { message, retry_secs } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                lines.extend(render_system_api_error(&message, retry_secs));
            }
            ContentBlock::CollapsedReadSearch { tool_name, paths, n_hidden } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
                lines.extend(render_collapsed_read_search(&tool_name, &path_refs, n_hidden));
            }
            ContentBlock::TaskAssignment { id, subject, description } => {
                flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
                lines.extend(render_task_assignment(&id, &subject, &description));
            }
        }
    }

    flush_text(&mut lines, &msg.role, &mut pending_text, ctx);
    lines.push(Line::from(""));
    lines
}

/// Render a system API error block (red-bordered, first 5 lines with [expand] hint,
/// optional retry countdown).
pub fn render_system_api_error(msg: &str, retry_secs: Option<u32>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        "\u{250c}\u{2500} API Error ",
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    )]));
    let all_lines: Vec<&str> = msg.lines().collect();
    let total = all_lines.len();
    for line in all_lines.iter().take(5) {
        lines.push(Line::from(vec![
            Span::styled("\u{2502} ", Style::default().fg(Color::Red)),
            Span::styled(line.to_string(), Style::default().fg(Color::White)),
        ]));
    }
    if total > 5 {
        lines.push(Line::from(vec![Span::styled(
            format!("\u{2502} ... {} more lines [expand]", total - 5),
            Style::default().fg(Color::DarkGray),
        )]));
    }
    lines.push(Line::from(vec![Span::styled(
        "\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
        Style::default().fg(Color::Red),
    )]));
    if let Some(n) = retry_secs {
        lines.push(Line::from(vec![Span::styled(
            format!("  \u{21bb} Retrying in {}s...", n),
            Style::default().fg(Color::Yellow),
        )]));
    }
    lines
}

/// Render a user command invocation (skill invocation display).
/// Shows: `▸ ` in cyan bold + command name in cyan bold + " " + args in white.
pub fn render_user_command(name: &str, args: &str) -> Vec<Line<'static>> {
    vec![Line::from(vec![
        Span::styled(
            "\u{25b8} ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            name.to_string(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".to_string(), Style::default()),
        Span::styled(args.to_string(), Style::default().fg(Color::White)),
    ])]
}

/// Render a user memory input line.
/// Shows: `# {key}: {value}` in cyan, with an optional `  Got it.` line in dark gray italic.
pub fn render_user_memory_input(key: &str, value: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![Span::styled(
            format!("# {}: {}", key, value),
            Style::default().fg(Color::Cyan),
        )]),
        Line::from(vec![Span::styled(
            "  Got it.",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )]),
    ]
}

/// Render a user local command output block.
/// Header: `  !{command}` in dark gray bold, body up to max_lines in gray,
/// overflow indicator: `  ... N more lines` in dark gray.
pub fn render_user_local_command_output(
    command: &str,
    output: &str,
    max_lines: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        format!("  !{}", command),
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
    )]));
    let total = output.lines().count();
    for line in output.lines().take(max_lines) {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(line.to_string(), Style::default().fg(Color::Gray)),
        ]));
    }
    if total > max_lines {
        lines.push(Line::from(vec![Span::styled(
            format!("  ... {} more lines", total - max_lines),
            Style::default().fg(Color::DarkGray),
        )]));
    }
    lines
}

/// Render a resource update notification line.
/// Shows: `↻ ` in cyan + `{server}: ` in dark gray bold + `{uri}` in white + ` · {reason}` in dark gray.
pub fn render_resource_update(server: &str, uri: &str, reason: &str) -> Vec<Line<'static>> {
    vec![Line::from(vec![
        Span::styled("\u{21bb} ", Style::default().fg(Color::Cyan)),
        Span::styled(
            format!("{}: ", server),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
        ),
        Span::styled(uri.to_string(), Style::default().fg(Color::White)),
        Span::styled(
            format!(" \u{00b7} {}", reason),
            Style::default().fg(Color::DarkGray),
        ),
    ])]
}

/// Render a collapsed read/search tool use summary.
/// Shows: `▸ ` in yellow + `{tool_name} ` in yellow bold + first few paths comma-joined,
/// followed by `(+ {n_hidden} more)` in dark gray if n_hidden > 0.
pub fn render_collapsed_read_search(
    tool_name: &str,
    paths: &[&str],
    n_hidden: usize,
) -> Vec<Line<'static>> {
    let paths_str = paths.join(", ");
    let mut spans = vec![
        Span::styled(
            "\u{25b8} ",
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            format!("{} ", tool_name),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled(paths_str, Style::default().fg(Color::White)),
    ];
    if n_hidden > 0 {
        spans.push(Span::styled(
            format!(" (+ {} more)", n_hidden),
            Style::default().fg(Color::DarkGray),
        ));
    }
    vec![Line::from(spans)]
}

/// Render a transcript task assignment row using the same structured title/subtitle language.
pub fn render_task_assignment(id: &str, subject: &str, desc: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let title = if subject.trim().is_empty() {
        "Assigned task"
    } else {
        subject.trim()
    };
    lines.push(Line::from(vec![
        Span::styled("  ~ ", Style::default().fg(CLAUDE_ORANGE)),
        Span::styled(
            title.to_string(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" · task #{}", id),
            Style::default().fg(TRANSCRIPT_MUTED),
        ),
    ]));
    for line in desc.lines().take(5) {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(
                line.to_string(),
                Style::default().fg(TRANSCRIPT_MUTED),
            ),
        ]));
    }
    lines
}

/// Render a grouped tool use summary.
/// Collapsed: `▸ {n} tool calls` in yellow with first few names comma-joined.
/// Expanded: same header + each tool on its own line with `  • ` prefix.
pub fn render_grouped_tool_use(names: &[&str], expanded: bool) -> Vec<Line<'static>> {
    let n = names.len();
    let preview = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    let header = Line::from(vec![
        Span::styled(
            "\u{25b8} ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} tool call{}", n, if n == 1 { "" } else { "s" }),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", preview),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    if !expanded {
        return vec![header];
    }
    let mut lines = vec![header];
    for name in names {
        lines.push(Line::from(vec![
            Span::styled("  \u{2022} ", Style::default().fg(Color::Yellow)),
            Span::styled(name.to_string(), Style::default().fg(Color::White)),
        ]));
    }
    lines
}

// ---------------------------------------------------------------------------
// Goal event rendering
// ---------------------------------------------------------------------------

/// Returns `true` when a user message was injected by the goal machinery
/// (i.e. it should NOT render as a regular user message bubble).
pub fn is_goal_event_message(text: &str) -> bool {
    text.starts_with("[Goal started]")
        || text.starts_with("[Goal continuation \u{2014}")  // em dash
        || text.starts_with("[Goal continuation -")         // fallback
}

/// Extract the objective text between `<objective>` and `</objective>` tags.
fn extract_goal_objective(text: &str) -> Option<String> {
    let tag = "<objective>";
    let start = text.find(tag)? + tag.len();
    let end = text.find("</objective>")?;
    if end > start {
        Some(text[start..end].trim().to_string())
    } else {
        None
    }
}

/// Extract the turn number from a "[Goal continuation — turn N]" header.
fn extract_goal_turn(text: &str) -> Option<u32> {
    // Find the first [...] bracket, search inside for "turn <N>"
    let open = text.find('[')?;
    let close = text.find(']')?;
    if close <= open { return None; }
    let segment = &text[open..close];
    let tag = "turn ";
    let idx = segment.rfind(tag)? + tag.len();
    segment[idx..].trim().parse().ok()
}

/// Render a goal-event message block.
///
/// `[Goal started]` shows the ◎ badge and the objective text.
/// `[Goal continuation — turn N]` shows a compact inline turn marker.
pub fn render_goal_event(text: &str, width: u16) -> Vec<Line<'static>> {
    if text.starts_with("[Goal continuation —") {
        let turn = extract_goal_turn(text).unwrap_or(0);
        return vec![Line::from(vec![
            Span::styled(
                "  \u{21ba} ".to_string(),  // ↺
                Style::default().fg(GOAL_MUTED),
            ),
            Span::styled(
                format!("goal \u{00b7} turn {}", turn),  // ·
                Style::default().fg(GOAL_MUTED).add_modifier(Modifier::ITALIC),
            ),
        ])];
    }

    // [Goal started] — header + wrapped objective
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            "  \u{25ce} ".to_string(),  // ◎
            Style::default().fg(GOAL_ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "goal started".to_string(),
            Style::default().fg(GOAL_ACCENT).add_modifier(Modifier::BOLD),
        ),
    ]));

    let objective = extract_goal_objective(text).unwrap_or_default();
    if !objective.is_empty() {
        let usable = (width as usize).saturating_sub(6).max(20);
        for line in wrap_plain_text(&objective, usable) {
            lines.push(Line::from(vec![
                Span::styled("    ".to_string(), Style::default()),
                Span::styled(line, Style::default().fg(GOAL_BODY)),
            ]));
        }
    }

    lines
}

/// Simple plain-text word-wrap (no markdown, no indent prefix).
fn wrap_plain_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    for para in text.lines() {
        if para.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in para.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.len() + 1 + word.len() <= max_width {
                current.push(' ');
                current.push_str(word);
            } else {
                out.push(current.clone());
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect::<String>()
    }

    #[test]
    fn render_message_uses_message_families_for_assistant_blocks() {
        let msg = Message::assistant_blocks(vec![
            ContentBlock::Thinking {
                thinking: "reasoning".to_string(),
                signature: "sig".to_string(),
            },
            ContentBlock::Text {
                text: "hello".to_string(),
            },
            ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({ "path": "README.md" }),
            },
            ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: ToolResultContent::Text("file contents".to_string()),
                is_error: Some(false),
            },
        ]);
        let ctx = RenderContext {
            width: 80,
            highlight: true,
            show_thinking: false,
            ..Default::default()
        };

        let rendered = render_message(&msg, &ctx)
            .into_iter()
            .map(|line| line_text(&line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!rendered.contains("◆"));
        assert!(rendered.contains("Thinking"));
        assert!(rendered.contains("read_file"));
        // ToolResult now shows output directly (no "Result" header)
        assert!(rendered.contains("file contents"));
        assert!(rendered.contains("hello"));
    }

    #[test]
    fn render_message_renders_user_text_in_brief_prompt_style() {
        let msg = Message::user("hello from user");
        let ctx = RenderContext::default();

        let rendered = render_message(&msg, &ctx)
            .into_iter()
            .map(|line| line_text(&line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("hello from user"));
        assert!(!rendered.contains("You"));
    }

    #[test]
    fn render_user_text_truncates_large_prompts() {
        let msg = Message::user(format!("{}\nquestion", "a".repeat(12_000)));
        let ctx = RenderContext::default();

        let rendered = render_message(&msg, &ctx)
            .into_iter()
            .map(|line| line_text(&line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("question"));
        assert!(rendered.contains(&"a".repeat(40)));
    }

    #[test]
    fn test_render_tool_result_cancelled() {
        let result = render_tool_result_cancelled("Bash");
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains("Bash"));
        assert!(text.contains("cancelled"));
    }

    #[test]
    fn test_render_tool_result_rejected() {
        let result = render_tool_result_rejected("Edit", "user pressed ctrl-c");
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains("Edit"));
        assert!(text.contains("interrupted"));
        let reason = line_text(&result[1]);
        assert!(reason.contains("user pressed ctrl-c"));
    }

    #[test]
    fn test_render_attachment_message() {
        let result = render_attachment_message("skill_listing", "5 tools available: Bash, Read", 80);
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains("skill_listing"));
        assert!(text.contains("5 tools"));
    }

    #[test]
    fn test_render_attachment_message_truncates_long_content() {
        let long = "x".repeat(200);
        let result = render_attachment_message("kind", &long, 80);
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains('\u{2026}') || text.len() < long.len(), "expected truncation");
    }

    #[test]
    fn test_render_advisor_message_loading() {
        let result = render_advisor_message(true, Some("claude-3"));
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains("Advising"));
        assert!(text.contains("claude-3"));
    }

    #[test]
    fn test_render_advisor_message_done() {
        let result = render_advisor_message(false, None);
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains("Advisor reviewed"));
    }

    #[test]
    fn test_render_agent_notification() {
        let result = render_agent_notification("Planner", "Starting task analysis...");
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains("Planner"));
        assert!(text.contains("Starting task analysis"));
    }

    #[test]
    fn test_render_shutdown_message() {
        let result = render_shutdown_message("max turns reached");
        assert!(!result.is_empty());
        let combined = result.iter().map(|l| line_text(l)).collect::<Vec<_>>().join("\n");
        assert!(combined.contains("Session ended"));
        assert!(combined.contains("max turns reached"));
    }

    #[test]
    fn test_render_bash_input_line() {
        let result = render_bash_input_line("ls -la");
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains("$"));
        assert!(text.contains("ls -la"));
    }

    #[test]
    fn test_render_bash_output_block() {
        let output = (0..50).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        let result = render_bash_output_block(&output, 10);
        assert!(!result.is_empty());
        // 10 content lines + 1 overflow indicator
        assert_eq!(result.len(), 11);
        let last = line_text(result.last().unwrap());
        assert!(last.contains("more lines"));
    }

    #[test]
    fn test_render_bash_output_block_no_overflow() {
        let output = "line 1\nline 2\nline 3";
        let result = render_bash_output_block(output, 10);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_render_plan_steps() {
        let steps = vec!["First step".to_string(), "Second step".to_string()];
        let result = render_plan_steps(&steps);
        assert!(!result.is_empty());
        let combined = result.iter().map(|l| line_text(l)).collect::<Vec<_>>().join("\n");
        assert!(combined.contains("Plan:"));
        assert!(combined.contains("1."));
        assert!(combined.contains("First step"));
        assert!(combined.contains("2."));
        assert!(combined.contains("Second step"));
    }

    #[test]
    fn test_render_plan_approval_prompt() {
        let result = render_plan_approval_prompt();
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains("Approve this plan?"));
        assert!(text.contains("[y]"));
        assert!(text.contains("[n]"));
        assert!(text.contains("[e]"));
    }

    #[test]
    fn test_render_tool_result_success_uses_30_lines() {
        let output = (0..50).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        let result = render_tool_result_success(&output, false);
        // 30 content lines + 1 overflow indicator = 31 (no separate header line)
        assert_eq!(result.len(), 31);
        let overflow_text = line_text(result.last().unwrap());
        assert!(overflow_text.contains("more lines"));
        assert!(!overflow_text.contains("ctrl+o"));
    }

    #[test]
    fn bash_tool_use_shows_running_command_title_and_command() {
        let msg = Message::assistant_blocks(vec![ContentBlock::ToolUse {
            id: "tu-1".to_string(),
            name: "Bash".to_string(),
            input: serde_json::json!({"command": "ls -la"}),
        }]);
        let rendered = render_message(&msg, &RenderContext::default())
            .into_iter()
            .map(|l| line_text(&l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("ls -la"), "command should appear in output");
        assert!(rendered.contains("Running command"), "updated tool title should appear");
        assert!(!rendered.contains("ctrl+o"), "legacy expansion hint should be removed");
    }

    #[test]
    fn non_bash_tool_use_shows_reading_file_title_with_summary() {
        let msg = Message::assistant_blocks(vec![ContentBlock::ToolUse {
            id: "tu-2".to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({"file_path": "/tmp/foo.txt"}),
        }]);
        let rendered = render_message(&msg, &RenderContext::default())
            .into_iter()
            .map(|l| line_text(&l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Reading file"), "tool title should appear");
        assert!(rendered.contains("foo.txt"), "file path summary should appear");
        assert!(!rendered.contains("ctrl+o"), "legacy expansion hint should be removed");
    }

    #[test]
    fn task_tool_use_shows_subagent_title_and_description() {
        let msg = Message::assistant_blocks(vec![ContentBlock::ToolUse {
            id: "tu-3".to_string(),
            name: "Task".to_string(),
            input: serde_json::json!({
                "subagent_type": "explore",
                "description": "Trace the auth flow"
            }),
        }]);
        let rendered = render_message(&msg, &RenderContext::default())
            .into_iter()
            .map(|l| line_text(&l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Explore agent"));
        assert!(rendered.contains("Trace the auth flow"));
    }

    #[test]
    fn bash_tool_result_renders_as_bash_output_with_tool_names_context() {
        let mut tool_names = HashMap::new();
        tool_names.insert("tu-bash-1".to_string(), "Bash".to_string());
        let ctx = RenderContext { tool_names, ..Default::default() };

        let msg = Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "tu-bash-1".to_string(),
            content: ToolResultContent::Text("hello world\nline2".to_string()),
            is_error: Some(false),
        }]);
        let rendered = render_message(&msg, &ctx)
            .into_iter()
            .map(|l| line_text(&l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("hello world"), "output should appear");
        // bash_output_block does NOT prefix with "Result" (that's render_tool_result_success)
        assert!(!rendered.contains("Result"), "bash output should NOT show generic 'Result' header");
    }

    #[test]
    fn non_bash_tool_result_shows_content() {
        let msg = Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "tu-read-1".to_string(),
            content: ToolResultContent::Text("file content here".to_string()),
            is_error: Some(false),
        }]);
        // No tool_names → falls back to render_tool_result_success (no separate header)
        let rendered = render_message(&msg, &RenderContext::default())
            .into_iter()
            .map(|l| line_text(&l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("file content here"), "content should appear");
    }

    // ── New function tests ────────────────────────────────────────────────────

    #[test]
    fn test_render_system_api_error_short_message() {
        let result = render_system_api_error("Connection refused", None);
        assert!(!result.is_empty());
        let combined = result.iter().map(|l| line_text(l)).collect::<Vec<_>>().join("\n");
        assert!(combined.contains("API Error"));
        assert!(combined.contains("Connection refused"));
        // No retry line
        assert!(!combined.contains("Retrying"));
    }

    #[test]
    fn test_render_system_api_error_with_retry() {
        let result = render_system_api_error("Timeout", Some(30));
        let combined = result.iter().map(|l| line_text(l)).collect::<Vec<_>>().join("\n");
        assert!(combined.contains("API Error"));
        assert!(combined.contains("Timeout"));
        assert!(combined.contains("Retrying in 30s"));
    }

    #[test]
    fn test_render_system_api_error_long_message_shows_expand_hint() {
        let msg = (0..10).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        let result = render_system_api_error(&msg, None);
        let combined = result.iter().map(|l| line_text(l)).collect::<Vec<_>>().join("\n");
        assert!(combined.contains("[expand]"), "should show [expand] hint when more than 5 lines");
        assert!(combined.contains("5 more lines"));
    }

    #[test]
    fn test_render_user_command() {
        let result = render_user_command("doctor", "--verbose");
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains('\u{25b8}'), "should have ▸ prefix");
        assert!(text.contains("doctor"));
        assert!(text.contains("--verbose"));
    }

    #[test]
    fn test_render_user_memory_input() {
        let result = render_user_memory_input("project", "Claurst");
        assert_eq!(result.len(), 2);
        let first = line_text(&result[0]);
        assert!(first.contains("# project: Claurst"));
        let second = line_text(&result[1]);
        assert!(second.contains("Got it."));
    }

    #[test]
    fn test_render_user_local_command_output_with_overflow() {
        let output = (0..20).map(|i| format!("out {}", i)).collect::<Vec<_>>().join("\n");
        let result = render_user_local_command_output("ls", &output, 5);
        // 1 header + 5 body + 1 overflow = 7
        assert_eq!(result.len(), 7);
        let header = line_text(&result[0]);
        assert!(header.contains("!ls"));
        let overflow = line_text(result.last().unwrap());
        assert!(overflow.contains("15 more lines"));
    }

    #[test]
    fn test_render_user_local_command_output_no_overflow() {
        let output = "line1\nline2";
        let result = render_user_local_command_output("echo", output, 10);
        // 1 header + 2 body = 3
        assert_eq!(result.len(), 3);
        let header = line_text(&result[0]);
        assert!(header.contains("!echo"));
    }

    #[test]
    fn test_render_resource_update() {
        let result = render_resource_update("mcp-server", "file:///tmp/foo.txt", "modified");
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains('\u{21bb}'), "should have ↻ prefix");
        assert!(text.contains("mcp-server"));
        assert!(text.contains("file:///tmp/foo.txt"));
        assert!(text.contains("modified"));
    }

    #[test]
    fn test_render_collapsed_read_search_no_hidden() {
        let paths = vec!["src/lib.rs", "src/main.rs"];
        let result = render_collapsed_read_search("Read", &paths, 0);
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains('\u{25b8}'), "should have ▸ prefix");
        assert!(text.contains("Read"));
        assert!(text.contains("src/lib.rs"));
        assert!(!text.contains("more"), "should not show 'more' when n_hidden is 0");
    }

    #[test]
    fn test_render_collapsed_read_search_with_hidden() {
        let paths = vec!["a.rs", "b.rs"];
        let result = render_collapsed_read_search("Glob", &paths, 3);
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains("(+ 3 more)"));
    }

    #[test]
    fn test_render_task_assignment() {
        let result = render_task_assignment("42", "Implement feature X", "Add the new widget system\nWith multi-line support");
        assert!(!result.is_empty());
        let combined = result.iter().map(|l| line_text(l)).collect::<Vec<_>>().join("\n");
        assert!(combined.contains("Implement feature X"));
        assert!(combined.contains("task #42"));
        assert!(combined.contains("Add the new widget system"));
    }

    #[test]
    fn test_render_task_assignment_truncates_desc_at_5_lines() {
        let desc = (0..10).map(|i| format!("desc line {}", i)).collect::<Vec<_>>().join("\n");
        let result = render_task_assignment("1", "Subject", &desc);
        let combined = result.iter().map(|l| line_text(l)).collect::<Vec<_>>().join("\n");
        // Only first 5 desc lines should appear
        assert!(combined.contains("desc line 4"));
        assert!(!combined.contains("desc line 5"), "should truncate desc at 5 lines");
    }

    #[test]
    fn test_render_grouped_tool_use_collapsed() {
        let names = vec!["Bash", "Read", "Write", "Glob"];
        let result = render_grouped_tool_use(&names, false);
        assert_eq!(result.len(), 1, "collapsed should be a single header line");
        let text = line_text(&result[0]);
        assert!(text.contains("4 tool calls"));
        assert!(text.contains("Bash"));
    }

    #[test]
    fn test_render_grouped_tool_use_expanded() {
        let names = vec!["Bash", "Read"];
        let result = render_grouped_tool_use(&names, true);
        // 1 header + 2 tool lines
        assert_eq!(result.len(), 3);
        let combined = result.iter().map(|l| line_text(l)).collect::<Vec<_>>().join("\n");
        assert!(combined.contains("2 tool calls"));
        assert!(combined.contains("Bash"));
        assert!(combined.contains("Read"));
        assert!(combined.contains('\u{2022}'), "expanded lines should have • prefix");
    }

    #[test]
    fn test_render_rate_limit_with_hint_false() {
        let result = render_rate_limit_with_hint(60, false);
        assert_eq!(result.len(), 2, "without hint should have 2 lines");
        let combined = result.iter().map(|l| line_text(l)).collect::<Vec<_>>().join("\n");
        assert!(combined.contains("Rate limit exceeded"));
        assert!(combined.contains("Retrying in 60s"));
        assert!(!combined.contains("upgrade"));
    }

    #[test]
    fn test_render_rate_limit_with_hint_true() {
        let result = render_rate_limit_with_hint(60, true);
        assert_eq!(result.len(), 3, "with hint should have 3 lines");
        let last = line_text(result.last().unwrap());
        assert!(last.contains("claude.ai/upgrade"));
    }

    #[test]
    fn test_render_rate_limit_banner_is_wrapper() {
        // render_rate_limit_banner must produce identical output to render_rate_limit_with_hint(n, false)
        let banner = render_rate_limit_banner(45);
        let hint_false = render_rate_limit_with_hint(45, false);
        let banner_text: Vec<_> = banner.iter().map(|l| line_text(l)).collect();
        let hint_text: Vec<_> = hint_false.iter().map(|l| line_text(l)).collect();
        assert_eq!(banner_text, hint_text);
    }

    #[test]
    fn test_render_agent_notification_with_severity_info() {
        let result = render_agent_notification_with_severity("Scout", "All clear", "info");
        let text = line_text(&result[0]);
        assert!(text.contains("Scout"));
        assert!(text.contains("All clear"));
    }

    #[test]
    fn test_render_agent_notification_with_severity_warn() {
        let result = render_agent_notification_with_severity("Scout", "Low memory", "warn");
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains("Scout"));
        assert!(text.contains("Low memory"));
    }

    #[test]
    fn test_render_agent_notification_with_severity_error() {
        let result = render_agent_notification_with_severity("Scout", "Crash detected", "error");
        assert!(!result.is_empty());
        let text = line_text(&result[0]);
        assert!(text.contains("Scout"));
        assert!(text.contains("Crash detected"));
    }

    #[test]
    fn test_render_agent_notification_defaults_to_info() {
        // render_agent_notification delegates to severity "info"
        let a = render_agent_notification("Bot", "hello");
        let b = render_agent_notification_with_severity("Bot", "hello", "info");
        let a_text: Vec<_> = a.iter().map(|l| line_text(l)).collect();
        let b_text: Vec<_> = b.iter().map(|l| line_text(l)).collect();
        assert_eq!(a_text, b_text);
    }
}



