// ask_user_dialog.rs — TUI overlay for model-initiated questions.
//
// Rendered when the model calls the `AskUserQuestion` tool.  The dialog
// shows the question text, an optional list of predefined choices that the
// user can navigate with arrow keys or number shortcuts, and a free-text
// input line for a custom answer.
//
// Layout:
//   ┌─ Question ──────────────────────────────────────┐
//   │                                                 │
//   │  How should the tests be run?                   │
//   │                                                 │
//   │  ▶ 1  cargo test --workspace                    │
//   │    2  cargo test -p claurst-api                 │
//   │    3  cargo test --features dev_full            │
//   │                                                 │
//   │  ❯ _                              (custom)      │
//   │                                                 │
//   │  Tab/↑↓: navigate   Enter: confirm   Esc: skip  │
//   └─────────────────────────────────────────────────┘

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::overlays::{centered_rect, CLAURST_PANEL_BG};

const BORDER_FG: Color = Color::Rgb(120, 120, 170);
const TITLE_FG: Color = Color::Rgb(200, 160, 255);
const QUESTION_FG: Color = Color::Rgb(230, 230, 230);
const OPTION_FG: Color = Color::Rgb(190, 190, 210);
const SELECTED_FG: Color = Color::Rgb(255, 255, 255);
const SELECTED_BG: Color = Color::Rgb(55, 55, 90);
const HINT_FG: Color = Color::Rgb(100, 100, 130);
const INPUT_FG: Color = Color::Rgb(200, 255, 200);
const NUMBER_FG: Color = Color::Rgb(150, 150, 200);

/// State for the ask-user question dialog overlay.
pub struct AskUserDialogState {
    /// Whether the dialog is currently visible.
    pub visible: bool,
    /// The question text from the model.
    pub question: String,
    /// Optional predefined choices.
    pub options: Option<Vec<String>>,
    /// Index of the currently highlighted option (0 = custom-text row when
    /// options is None, or indices into options vec, with the custom row last).
    pub selected_idx: usize,
    /// Custom text the user is typing (if they choose not to pick an option).
    pub custom_text: String,
    /// Whether cursor is in the custom-text input row.
    pub in_custom_input: bool,
    /// Pending reply channel sender — set when the dialog opens, consumed on submit.
    pub(crate) reply_tx: Option<tokio::sync::oneshot::Sender<String>>,
}

impl Default for AskUserDialogState {
    fn default() -> Self {
        Self {
            visible: false,
            question: String::new(),
            options: None,
            selected_idx: 0,
            custom_text: String::new(),
            in_custom_input: false,
            reply_tx: None,
        }
    }
}

impl AskUserDialogState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the dialog with a question and optional choices.
    pub fn open(
        &mut self,
        question: String,
        options: Option<Vec<String>>,
        reply_tx: tokio::sync::oneshot::Sender<String>,
    ) {
        self.question = question;
        self.options = options;
        self.selected_idx = 0;
        self.custom_text.clear();
        self.in_custom_input = self.options.is_none();
        self.reply_tx = Some(reply_tx);
        self.visible = true;
    }

    /// Navigate selection up.
    pub fn select_prev(&mut self) {
        let n = self.option_count();
        if n == 0 {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = n; // wrap to custom row
            self.in_custom_input = true;
        } else {
            self.selected_idx -= 1;
            self.in_custom_input = self.selected_idx >= self.options_len();
        }
    }

    /// Navigate selection down.
    pub fn select_next(&mut self) {
        let n = self.option_count();
        if n == 0 {
            return;
        }
        if self.selected_idx >= n {
            self.selected_idx = 0;
            self.in_custom_input = false;
        } else {
            self.selected_idx += 1;
            self.in_custom_input = self.selected_idx >= self.options_len();
        }
    }

    /// Select an option directly by 1-based number key.
    pub fn select_by_number(&mut self, n: usize) {
        if let Some(ref opts) = self.options {
            if n >= 1 && n <= opts.len() {
                self.selected_idx = n - 1;
                self.in_custom_input = false;
            }
        }
    }

    /// Append a character to the custom-text input.
    ///
    /// Any printable character auto-switches to the custom row regardless of
    /// where the selection currently is — so the user can just start typing
    /// without having to navigate down with Tab/↓ first.
    pub fn push_char(&mut self, c: char) {
        self.custom_text.push(c);
        self.in_custom_input = true;
        self.selected_idx = self.options_len();
    }

    /// Backspace in the custom-text input.
    pub fn pop_char(&mut self) {
        if self.in_custom_input || self.options.is_none() {
            self.custom_text.pop();
        }
    }

    /// Confirm the current selection and send the answer.
    ///
    /// Returns `true` if the dialog was successfully submitted (i.e. a reply
    /// channel was present).
    pub fn confirm(&mut self) -> bool {
        let answer = if self.in_custom_input || self.options.is_none() {
            self.custom_text.clone()
        } else if let Some(ref opts) = self.options {
            opts.get(self.selected_idx).cloned().unwrap_or_default()
        } else {
            self.custom_text.clone()
        };

        self.send_reply(answer)
    }

    /// Dismiss without answering (sends an empty string so the tool result
    /// signals "user dismissed").
    pub fn dismiss(&mut self) -> bool {
        self.send_reply(String::new())
    }

    fn send_reply(&mut self, answer: String) -> bool {
        self.visible = false;
        if let Some(tx) = self.reply_tx.take() {
            let _ = tx.send(answer);
            true
        } else {
            false
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn options_len(&self) -> usize {
        self.options.as_ref().map(|v| v.len()).unwrap_or(0)
    }

    /// Total number of selectable rows: options + custom-text row.
    fn option_count(&self) -> usize {
        self.options_len() + 1
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the ask-user question dialog into the terminal buffer.
///
/// Call this only when `state.visible` is true; typically from `render_app`.
pub fn render_ask_user_dialog(state: &AskUserDialogState, area: Rect, buf: &mut Buffer) {
    if !state.visible {
        return;
    }

    // ---- size estimate ----
    let question_lines = word_wrap(&state.question, 52).len() as u16;
    let options_lines = state.options.as_ref().map(|v| v.len() as u16 + 1).unwrap_or(0);
    let height = (5 + question_lines + options_lines + 3).min(area.height.saturating_sub(2));
    let width = 58u16.min(area.width.saturating_sub(4));
    let modal_area = centered_rect(width, height, area);

    // ---- background ----
    for y in modal_area.top()..modal_area.bottom() {
        for x in modal_area.left()..modal_area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(' ');
                cell.set_bg(CLAURST_PANEL_BG);
            }
        }
    }

    // ---- border ----
    let border_style = Style::default().fg(BORDER_FG).bg(CLAURST_PANEL_BG);
    let inner_w = modal_area.width.saturating_sub(2) as usize;
    for y in modal_area.top()..modal_area.bottom() {
        let is_top = y == modal_area.top();
        let is_bot = y == modal_area.bottom() - 1;
        for x in modal_area.left()..modal_area.right() {
            let is_left = x == modal_area.left();
            let is_right = x == modal_area.right() - 1;
            if let Some(cell) = buf.cell_mut((x, y)) {
                let ch = match (is_top, is_bot, is_left, is_right) {
                    (true, _, true, _) => '╭',
                    (true, _, _, true) => '╮',
                    (_, true, true, _) => '╰',
                    (_, true, _, true) => '╯',
                    (true, _, _, _) | (_, true, _, _) => '─',
                    (_, _, true, _) | (_, _, _, true) => '│',
                    _ => continue,
                };
                cell.set_char(ch);
                cell.set_style(border_style);
            }
        }
    }

    // ---- title ----
    let title = " Question ";
    let title_x = modal_area.left() + 2;
    let title_style = Style::default().fg(TITLE_FG).bg(CLAURST_PANEL_BG).add_modifier(Modifier::BOLD);
    for (i, ch) in title.chars().enumerate() {
        let x = title_x + i as u16;
        if x < modal_area.right() - 1 {
            if let Some(cell) = buf.cell_mut((x, modal_area.top())) {
                cell.set_char(ch);
                cell.set_style(title_style);
            }
        }
    }

    // ---- inner content area ----
    let inner = Rect {
        x: modal_area.x + 1,
        y: modal_area.y + 1,
        width: modal_area.width.saturating_sub(2),
        height: modal_area.height.saturating_sub(2),
    };

    let mut row = inner.y;

    macro_rules! write_line {
        ($row:expr, $line:expr) => {{
            if $row < inner.y + inner.height {
                let r = Rect { x: inner.x, y: $row, width: inner.width, height: 1 };
                Paragraph::new($line).render(r, buf);
            }
        }};
    }

    // Question text
    row += 1; // top padding
    for wrap_line in word_wrap(&state.question, inner_w) {
        write_line!(
            row,
            Line::from(Span::styled(wrap_line, Style::default().fg(QUESTION_FG).bg(CLAURST_PANEL_BG)))
        );
        row += 1;
        if row >= inner.y + inner.height {
            return;
        }
    }

    // Spacer
    row += 1;

    // Option rows
    if let Some(ref opts) = state.options {
        for (i, opt) in opts.iter().enumerate() {
            if row >= inner.y + inner.height - 2 {
                break;
            }
            let is_sel = !state.in_custom_input && state.selected_idx == i;
            let prefix = if is_sel { "▶ " } else { "  " };
            let num_str = format!("{}", i + 1);
            let label = format!(" {}", opt);
            let style_bg = if is_sel { SELECTED_BG } else { CLAURST_PANEL_BG };
            write_line!(row, Line::from(vec![
                Span::styled(prefix, Style::default().fg(if is_sel { SELECTED_FG } else { HINT_FG }).bg(style_bg)),
                Span::styled(num_str, Style::default().fg(NUMBER_FG).bg(style_bg)),
                Span::styled(label, Style::default().fg(if is_sel { SELECTED_FG } else { OPTION_FG }).bg(style_bg).add_modifier(if is_sel { Modifier::BOLD } else { Modifier::empty() })),
            ]));
            row += 1;
        }
        row += 1; // spacer before custom row
    }

    // Custom input row
    if row < inner.y + inner.height - 1 {
        let is_sel = state.in_custom_input || state.options.is_none();
        let prefix = if is_sel { "❯ " } else { "  " };
        let cursor = if is_sel { "█" } else { "" };
        let style_bg = if is_sel { SELECTED_BG } else { CLAURST_PANEL_BG };
        let mut spans = vec![
            Span::styled(prefix, Style::default().fg(if is_sel { SELECTED_FG } else { HINT_FG }).bg(style_bg)),
        ];
        if state.custom_text.is_empty() && !is_sel && state.options.is_some() {
            // Not yet active: show a subtle prompt so user knows they can type
            spans.push(Span::styled(
                "type to fill custom answer…",
                Style::default().fg(HINT_FG).bg(style_bg),
            ));
        } else {
            let display_text = format!("{}{}", state.custom_text, cursor);
            spans.push(Span::styled(display_text, Style::default().fg(INPUT_FG).bg(style_bg)));
        }
        write_line!(row, Line::from(spans));
        row += 1;
    }

    // Hint row
    row += 1;
    if row < inner.y + inner.height {
        let hint = if state.options.is_some() {
            "  type: custom   ↑↓/Tab: options   Enter: confirm   Esc: skip"
        } else {
            "  Type answer, then Enter to confirm   Esc: skip"
        };
        write_line!(row, Line::from(Span::styled(hint, Style::default().fg(HINT_FG).bg(CLAURST_PANEL_BG))));
    }

    let _ = row;
}

// ---------------------------------------------------------------------------
// Word-wrap helper
// ---------------------------------------------------------------------------

fn word_wrap(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.len() + 1 + word.len() <= max_width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(current.clone());
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines
}
