//! Markdown -> ratatui lines renderer used by transcript message families.

use crate::figures;
use once_cell::sync::Lazy;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use regex::Regex;
use unicode_width::UnicodeWidthStr;

/// Regex pattern to detect URLs (http://, https://, ftp://, www.)
static URL_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:https?|ftp)://\S+|www\.\S+").expect("Invalid URL regex pattern"));

/// Regex pattern to detect email addresses
static EMAIL_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}")
        .expect("Invalid email regex pattern")
});

/// Render markdown text to styled ratatui lines.
pub fn render_markdown(text: &str, width: u16) -> Vec<Line<'static>> {
    let all_lines: Vec<&str> = text.lines().collect();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut idx = 0;

    while idx < all_lines.len() {
        let raw = all_lines[idx];
        if raw.trim_start().starts_with("```") {
            if in_code_block {
                lines.push(Line::from(vec![Span::styled(
                    "  └──────────────────────────────────────────────────".to_string(),
                    Style::default().fg(Color::Yellow),
                )]));
                in_code_block = false;
                code_lang.clear();
            } else {
                in_code_block = true;
                code_lang = raw.trim_start().trim_start_matches('`').trim().to_string();
                let lang_label = if code_lang.is_empty() {
                    String::new()
                } else {
                    format!(" {} ", code_lang)
                };
                lines.push(Line::from(vec![Span::styled(
                    format!("  ┌──────────────────────{}", lang_label),
                    Style::default().fg(Color::Yellow),
                )]));
            }
            idx += 1;
            continue;
        }

        if in_code_block {
            lines.push(Line::from(vec![
                Span::styled("  │ ", Style::default().fg(Color::Yellow)),
                Span::styled(raw.to_string(), Style::default().fg(Color::White)),
            ]));
            idx += 1;
            continue;
        }

        // Check for markdown tables
        if let Some((table, end_idx)) = super::markdown_enhanced::detect_table(&all_lines, idx) {
            lines.extend(super::markdown_enhanced::render_table(&table));
            idx = end_idx;
            continue;
        }

        if raw.starts_with("> ") {
            let quoted = &raw[2..];
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", figures::BLOCKQUOTE_BAR),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(quoted.to_string(), Style::default().fg(Color::DarkGray)),
            ]));
            idx += 1;
            continue;
        }

        if raw.starts_with("### ") {
            lines.push(Line::from(vec![Span::styled(
                format!("  {}", &raw[4..]),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )]));
            idx += 1;
            continue;
        }
        if raw.starts_with("## ") {
            lines.push(Line::from(vec![Span::styled(
                format!("  {}", &raw[3..]),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )]));
            idx += 1;
            continue;
        }
        if raw.starts_with("# ") {
            lines.push(Line::from(vec![Span::styled(
                format!("  {}", &raw[2..]),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED),
            )]));
            idx += 1;
            continue;
        }

        let padded = format!("  {}", raw);
        let effective_width = width.saturating_sub(4) as usize;
        for wrapped_line in word_wrap(&padded, effective_width) {
            let spans = parse_inline_spans(wrapped_line);
            lines.push(Line::from(spans));
        }

        idx += 1;
    }

    if in_code_block {
        lines.push(Line::from(vec![Span::styled(
            "  └──────────────────────────────────────────────────".to_string(),
            Style::default().fg(Color::Yellow),
        )]));
    }

    lines
}

/// Split plain text into spans with URL/email detection and styling.
/// URLs and emails are styled with cyan color and underline.
fn split_and_style_links(text: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut last_end = 0;

    // Check for URLs first
    for url_match in URL_PATTERN.find_iter(text) {
        let match_start = url_match.start();
        let match_end = url_match.end();

        // Add text before the URL
        if match_start > last_end {
            spans.push(Span::raw(text[last_end..match_start].to_string()));
        }

        // Add the URL with special styling (cyan with underline)
        let url_text = url_match.as_str();
        spans.push(Span::styled(
            url_text.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED),
        ));
        last_end = match_end;
    }

    // Check for emails in remaining text (only if no URLs were found)
    if last_end == 0 {
        for email_match in EMAIL_PATTERN.find_iter(text) {
            let match_start = email_match.start();
            let match_end = email_match.end();

            // Add text before the email
            if match_start > last_end {
                spans.push(Span::raw(text[last_end..match_start].to_string()));
            }

            // Add the email with special styling (cyan with underline)
            let email_text = email_match.as_str();
            spans.push(Span::styled(
                email_text.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::UNDERLINED),
            ));
            last_end = match_end;
        }
    }

    // Add any remaining text
    if last_end < text.len() {
        spans.push(Span::raw(text[last_end..].to_string()));
    }

    // If no links/emails were found, return a simple raw span
    if spans.is_empty() {
        spans.push(Span::raw(text.to_string()));
    }

    spans
}

fn parse_inline_spans(text: String) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = text.as_str();

    while !remaining.is_empty() {
        let bold_pos = remaining.find("**");
        let code_pos = remaining.find('`');

        match (bold_pos, code_pos) {
            (None, None) => {
                // No more formatting, but check for links/emails in plain text
                spans.extend(split_and_style_links(remaining));
                break;
            }
            (Some(b), Some(c)) if c < b => {
                // Code block comes first
                if c > 0 {
                    spans.extend(split_and_style_links(&remaining[..c]));
                }
                let after_tick = &remaining[c + 1..];
                if let Some(end) = after_tick.find('`') {
                    spans.push(Span::styled(
                        after_tick[..end].to_string(),
                        Style::default().fg(Color::Yellow),
                    ));
                    remaining = &after_tick[end + 1..];
                } else {
                    spans.push(Span::raw(remaining[c..].to_string()));
                    break;
                }
            }
            (Some(b), _) => {
                // Bold comes first
                if b > 0 {
                    spans.extend(split_and_style_links(&remaining[..b]));
                }
                let after_stars = &remaining[b + 2..];
                if let Some(end) = after_stars.find("**") {
                    spans.push(Span::styled(
                        after_stars[..end].to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    remaining = &after_stars[end + 2..];
                } else {
                    spans.push(Span::raw(remaining[b..].to_string()));
                    break;
                }
            }
            (None, Some(c)) => {
                // Code block (no bold)
                if c > 0 {
                    spans.extend(split_and_style_links(&remaining[..c]));
                }
                let after_tick = &remaining[c + 1..];
                if let Some(end) = after_tick.find('`') {
                    spans.push(Span::styled(
                        after_tick[..end].to_string(),
                        Style::default().fg(Color::Yellow),
                    ));
                    remaining = &after_tick[end + 1..];
                } else {
                    spans.push(Span::raw(remaining[c..].to_string()));
                    break;
                }
            }
        }
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    spans
}

fn word_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 || UnicodeWidthStr::width(text) <= width {
        return vec![text.to_string()];
    }

    let mut result = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0usize;

    for word in text.split_whitespace() {
        let word_w = UnicodeWidthStr::width(word);
        if current_width == 0 {
            current_line.push_str(word);
            current_width = word_w;
        } else if current_width + 1 + word_w <= width {
            current_line.push(' ');
            current_line.push_str(word);
            current_width += 1 + word_w;
        } else {
            result.push(std::mem::take(&mut current_line));
            current_line.push_str(word);
            current_width = word_w;
        }
    }

    if !current_line.is_empty() {
        result.push(current_line);
    }
    if result.is_empty() {
        result.push(text.to_string());
    }
    result
}
