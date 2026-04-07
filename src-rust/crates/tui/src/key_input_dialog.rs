// key_input_dialog.rs — Masked text input overlay for entering API keys.
//
// Provides a modal dialog that collects an API key from the user with
// masked display (showing only the last 4 characters).

use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::overlays::{centered_rect, render_dark_overlay, render_dialog_bg, CLAURST_PANEL_BG};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// State for the API key input dialog.
pub struct KeyInputDialogState {
    pub visible: bool,
    pub provider_id: String,
    pub provider_name: String,
    pub input: String,
    pub cursor_pos: usize,
}

impl KeyInputDialogState {
    pub fn new() -> Self {
        Self {
            visible: false,
            provider_id: String::new(),
            provider_name: String::new(),
            input: String::new(),
            cursor_pos: 0,
        }
    }

    /// Open the dialog for a specific provider.
    pub fn open(&mut self, provider_id: String, provider_name: String) {
        self.visible = true;
        self.provider_id = provider_id;
        self.provider_name = provider_name;
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// Close and clear the dialog.
    pub fn close(&mut self) {
        self.visible = false;
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            // Find the previous char boundary
            let prev = self.input[..self.cursor_pos]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.remove(prev);
            self.cursor_pos = prev;
        }
    }

    /// Take the entered key and close the dialog.
    pub fn take_key(&mut self) -> String {
        let key = self.input.clone();
        self.close();
        key
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the key input dialog overlay — OpenCode-style: dark overlay, no
/// border, minimal and polished.
pub fn render_key_input_dialog(frame: &mut Frame, state: &KeyInputDialogState, area: Rect) {
    if !state.visible {
        return;
    }

    let pink = Color::Rgb(233, 30, 99);
    let dim = Color::Rgb(90, 90, 90);
    let dialog_bg = CLAURST_PANEL_BG;

    // ── Darken the entire background ──
    render_dark_overlay(frame, area);

    // ── Dialog size ──
    let width = 60u16.min(area.width.saturating_sub(4));
    let height = 9u16;
    let dialog_area = centered_rect(width, height, area);

    // ── Fill dialog background (no border) ──
    render_dialog_bg(frame, dialog_area);

    let inner = Rect {
        x: dialog_area.x + 1,
        y: dialog_area.y + 1,
        width: dialog_area.width.saturating_sub(2),
        height: dialog_area.height.saturating_sub(2),
    };

    // ── Build lines ──
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Title row: "Connect {provider}" on left, "esc" on right
    let title_text = format!("Connect {}", state.provider_name);
    let title_pad = inner.width.saturating_sub(title_text.len() as u16 + 5) as usize;
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {}", title_text),
            Style::default().fg(pink).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>width$}", "esc ", width = title_pad),
            Style::default().fg(dim),
        ),
    ]));

    // Blank line
    lines.push(Line::from(""));

    // "API Key:" label
    lines.push(Line::from(vec![Span::styled(
        " API Key:",
        Style::default().fg(Color::Rgb(180, 180, 180)),
    )]));

    // Masked key display (show last 4 chars, mask the rest)
    let masked = if state.input.is_empty() {
        "paste your API key here...".to_string()
    } else {
        let len = state.input.len();
        if len <= 4 {
            state.input.clone()
        } else {
            format!("{}{}", "\u{2022}".repeat(len - 4), &state.input[len - 4..])
        }
    };

    let input_style = if state.input.is_empty() {
        Style::default().fg(dim)
    } else {
        Style::default().fg(Color::White)
    };

    lines.push(Line::from(vec![
        Span::styled(format!(" {}", masked), input_style),
        Span::styled("_", Style::default().fg(pink)), // cursor
    ]));

    // Blank line
    lines.push(Line::from(""));

    // Hint row
    lines.push(Line::from(vec![
        Span::styled(" enter", Style::default().fg(dim)),
        Span::styled(" confirm", Style::default().fg(dim)),
    ]));

    let para = Paragraph::new(lines).bg(dialog_bg);
    frame.render_widget(para, inner);
}
