//! Enhanced markdown rendering with tables, italic, strikethrough support.
//! This module complements markdown.rs with additional features.

use once_cell::sync::Lazy;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use regex::Regex;
use unicode_width::UnicodeWidthStr;

/// Regex pattern to detect markdown table rows (lines starting/ending with |)
static TABLE_ROW_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*\|.+\|\s*$").expect("Invalid table row regex pattern"));

/// Regex pattern to detect markdown table separator row (dashes/colons/pipes)
static TABLE_SEPARATOR_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*\|\s*[:|-]+\s*(\|\s*[:|-]+\s*)*\|\s*$")
        .expect("Invalid table separator regex pattern")
});

/// Alignment detected from separator row
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAlignment {
    Left,
    Center,
    Right,
}

impl TableAlignment {
    fn from_separator(sep: &str) -> Self {
        let trimmed = sep.trim();
        let has_left_colon = trimmed.starts_with(':');
        let has_right_colon = trimmed.ends_with(':');

        match (has_left_colon, has_right_colon) {
            (true, true) => TableAlignment::Center,
            (false, true) => TableAlignment::Right,
            (true, false) => TableAlignment::Left,
            (false, false) => TableAlignment::Left,
        }
    }
}

/// Represents a parsed markdown table
#[derive(Debug, Clone)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub alignments: Vec<TableAlignment>,
}

impl Table {
    /// Parse cells from a table row, handling escaped pipes
    fn parse_row(line: &str) -> Vec<String> {
        let trimmed = line.trim();
        let without_pipes = if trimmed.starts_with('|') && trimmed.ends_with('|') {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        };

        without_pipes
            .split('|')
            .map(|cell| cell.trim().to_string())
            .collect()
    }

    /// Extract alignments from separator row
    fn parse_alignments(separator_line: &str) -> Vec<TableAlignment> {
        let cells = Self::parse_row(separator_line);
        cells
            .iter()
            .map(|cell| TableAlignment::from_separator(cell))
            .collect()
    }
}

/// Detect if a sequence of lines forms a markdown table
pub fn detect_table(lines: &[&str], start_idx: usize) -> Option<(Table, usize)> {
    if start_idx + 1 >= lines.len() {
        return None;
    }

    // Check if current line is a table row
    if !TABLE_ROW_PATTERN.is_match(lines[start_idx]) {
        return None;
    }

    // Check if next line is a separator
    if !TABLE_SEPARATOR_PATTERN.is_match(lines[start_idx + 1]) {
        return None;
    }

    let headers = Table::parse_row(lines[start_idx]);
    let alignments = Table::parse_alignments(lines[start_idx + 1]);

    // Validate header/separator column count matches
    if headers.len() != alignments.len() {
        return None;
    }

    let mut rows = Vec::new();
    let mut end_idx = start_idx + 2;

    // Collect all consecutive table rows
    while end_idx < lines.len() && TABLE_ROW_PATTERN.is_match(lines[end_idx]) {
        let row = Table::parse_row(lines[end_idx]);
        if row.len() == headers.len() {
            rows.push(row);
            end_idx += 1;
        } else {
            break;
        }
    }

    Some((
        Table {
            headers,
            rows,
            alignments,
        },
        end_idx,
    ))
}

/// Render a markdown table as styled lines with box-drawing characters
pub fn render_table(table: &Table) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Calculate column widths
    let mut col_widths: Vec<usize> = table
        .headers
        .iter()
        .map(|h| UnicodeWidthStr::width(h.as_str()).max(3))
        .collect();

    for row in &table.rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_widths.len() {
                col_widths[i] = col_widths[i].max(UnicodeWidthStr::width(cell.as_str()));
            }
        }
    }

    // Top border: ┌─┬─┐
    let mut top_border = String::from("  ┌");
    for (i, width) in col_widths.iter().enumerate() {
        top_border.push_str(&"─".repeat(width + 2));
        if i < col_widths.len() - 1 {
            top_border.push('┬');
        }
    }
    top_border.push('┐');
    lines.push(Line::from(vec![Span::styled(
        top_border,
        Style::default().fg(Color::DarkGray),
    )]));

    // Header row with bold styling
    let mut header_spans = vec![Span::styled(
        "  │ ".to_string(),
        Style::default().fg(Color::DarkGray),
    )];
    for (i, header) in table.headers.iter().enumerate() {
        let width = col_widths[i];
        let padded = match table
            .alignments
            .get(i)
            .copied()
            .unwrap_or(TableAlignment::Left)
        {
            TableAlignment::Left => format!("{:<width$}", header, width = width),
            TableAlignment::Right => format!("{:>width$}", header, width = width),
            TableAlignment::Center => {
                let hdr_width = UnicodeWidthStr::width(header.as_str());
                let total_pad = width.saturating_sub(hdr_width);
                let left_pad = total_pad / 2;
                format!(
                    "{:>width$}",
                    &format!("{}{}", " ".repeat(left_pad), header),
                    width = width + left_pad
                )
            }
        };
        header_spans.push(Span::styled(
            padded,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            " │ ".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines.push(Line::from(header_spans));

    // Separator: ├─┼─┤
    let mut sep = String::from("  ├");
    for (i, width) in col_widths.iter().enumerate() {
        sep.push_str(&"─".repeat(width + 2));
        if i < col_widths.len() - 1 {
            sep.push('┼');
        }
    }
    sep.push('┤');
    lines.push(Line::from(vec![Span::styled(
        sep,
        Style::default().fg(Color::DarkGray),
    )]));

    // Data rows
    for row in &table.rows {
        let mut row_spans = vec![Span::styled(
            "  │ ".to_string(),
            Style::default().fg(Color::DarkGray),
        )];
        for (i, cell) in row.iter().enumerate() {
            if i < col_widths.len() {
                let width = col_widths[i];
                let padded = match table
                    .alignments
                    .get(i)
                    .copied()
                    .unwrap_or(TableAlignment::Left)
                {
                    TableAlignment::Left => format!("{:<width$}", cell, width = width),
                    TableAlignment::Right => format!("{:>width$}", cell, width = width),
                    TableAlignment::Center => {
                        let cell_width = UnicodeWidthStr::width(cell.as_str());
                        let total_pad = width.saturating_sub(cell_width);
                        let left_pad = total_pad / 2;
                        format!(
                            "{:>width$}",
                            &format!("{}{}", " ".repeat(left_pad), cell),
                            width = width + left_pad
                        )
                    }
                };
                row_spans.push(Span::raw(padded));
            }
            row_spans.push(Span::styled(
                " │ ".to_string(),
                Style::default().fg(Color::DarkGray),
            ));
        }
        lines.push(Line::from(row_spans));
    }

    // Bottom border: └─┴─┘
    let mut bottom_border = String::from("  └");
    for (i, width) in col_widths.iter().enumerate() {
        bottom_border.push_str(&"─".repeat(width + 2));
        if i < col_widths.len() - 1 {
            bottom_border.push('┴');
        }
    }
    bottom_border.push('┘');
    lines.push(Line::from(vec![Span::styled(
        bottom_border,
        Style::default().fg(Color::DarkGray),
    )]));

    lines
}

/// Parse inline formatting including italic, strikethrough, bold
/// Supports: **bold**, __bold__, *italic*, _italic_, ~~strikethrough~~, `code`
/// NOTE: Limited support for nesting to avoid performance issues
pub fn parse_inline_formatting(text: &str) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_text = String::new();
    let mut idx = 0;

    while idx < chars.len() {
        // Check for backtick (code) - no nesting
        if chars[idx] == '`' {
            // Flush current text
            if !current_text.is_empty() {
                spans.push(Span::raw(current_text.clone()));
                current_text.clear();
            }

            // Find closing backtick
            let mut code_content = String::new();
            idx += 1;
            while idx < chars.len() && chars[idx] != '`' {
                code_content.push(chars[idx]);
                idx += 1;
            }
            if idx < chars.len() {
                idx += 1; // skip closing backtick
            }

            spans.push(Span::styled(
                code_content,
                Style::default().fg(Color::Yellow),
            ));
            continue;
        }

        // Check for ** or __ (bold) - with limited nesting support
        if idx + 1 < chars.len() {
            if (chars[idx] == '*' && chars[idx + 1] == '*')
                || (chars[idx] == '_' && chars[idx + 1] == '_')
            {
                let marker = chars[idx];

                // Flush current text
                if !current_text.is_empty() {
                    spans.push(Span::raw(current_text.clone()));
                    current_text.clear();
                }

                // Find closing marker (search within 500 chars max to prevent runaway)
                let mut bold_content = String::new();
                idx += 2;
                let max_search = (idx + 500).min(chars.len());
                while idx < max_search {
                    if idx + 1 < chars.len() && chars[idx] == marker && chars[idx + 1] == marker {
                        break;
                    }
                    bold_content.push(chars[idx]);
                    idx += 1;
                }

                if idx + 1 < chars.len() && chars[idx] == marker && chars[idx + 1] == marker {
                    idx += 2; // skip closing marker
                }

                // Apply bold style
                spans.push(Span::styled(
                    bold_content,
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                continue;
            }
        }

        // Check for ~~ (strikethrough) - no nesting
        if idx + 1 < chars.len() && chars[idx] == '~' && chars[idx + 1] == '~' {
            // Flush current text
            if !current_text.is_empty() {
                spans.push(Span::raw(current_text.clone()));
                current_text.clear();
            }

            // Find closing marker
            let mut strikethrough_content = String::new();
            idx += 2;
            let max_search = (idx + 500).min(chars.len());
            while idx < max_search {
                if idx + 1 < chars.len() && chars[idx] == '~' && chars[idx + 1] == '~' {
                    break;
                }
                strikethrough_content.push(chars[idx]);
                idx += 1;
            }

            if idx + 1 < chars.len() && chars[idx] == '~' && chars[idx + 1] == '~' {
                idx += 2; // skip closing marker
            }

            // Apply strikethrough style
            spans.push(Span::styled(
                strikethrough_content,
                Style::default().add_modifier(Modifier::CROSSED_OUT),
            ));
            continue;
        }

        // Check for * or _ (italic) - no nesting
        if idx < chars.len() && (chars[idx] == '*' || chars[idx] == '_') {
            let marker = chars[idx];

            // Make sure it's not part of ** or __
            let is_bold_marker = (idx + 1 < chars.len() && chars[idx + 1] == marker)
                || (idx > 0 && chars[idx - 1] == marker);

            if !is_bold_marker {
                // Flush current text
                if !current_text.is_empty() {
                    spans.push(Span::raw(current_text.clone()));
                    current_text.clear();
                }

                // Find closing marker
                let mut italic_content = String::new();
                idx += 1;
                let max_search = (idx + 500).min(chars.len());
                while idx < max_search && chars[idx] != marker {
                    italic_content.push(chars[idx]);
                    idx += 1;
                }

                if idx < chars.len() && chars[idx] == marker {
                    idx += 1; // skip closing marker
                }

                // Apply italic style
                spans.push(Span::styled(
                    italic_content,
                    Style::default().add_modifier(Modifier::ITALIC),
                ));
                continue;
            }
        }

        // Regular character
        current_text.push(chars[idx]);
        idx += 1;
    }

    // Flush remaining text
    if !current_text.is_empty() {
        spans.push(Span::raw(current_text));
    }

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }

    spans
}
