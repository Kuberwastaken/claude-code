// elicitation_dialog.rs — MCP Elicitation dialog.
//
// Mirrors src/components/mcp/ElicitationDialog.tsx.
//
// MCP servers can request structured input from the user via the elicitation
// protocol.  The server sends a request containing a JSON Schema that describes
// the form fields; the TUI presents a dialog, the user fills in the values, and
// the TUI sends the response back.
//
// This module handles the TUI side: state management, field rendering, keyboard
// navigation, and validation.  The caller is responsible for:
//   1. Parsing the incoming JSON Schema into `Vec<ElicitationField>`.
//   2. Calling `ElicitationDialogState::show(...)` when a request arrives.
//   3. Polling `ElicitationDialogState::take_result()` after each key event to
//      detect a submitted or cancelled response.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Field kinds
// ---------------------------------------------------------------------------

/// The kind of an elicitation form field.
#[derive(Debug, Clone)]
pub enum ElicitationFieldKind {
    /// Free-text input, with an optional format hint ("email", "date", "uri").
    Text { format: Option<String> },
    /// Single-value selection from a list of (value, label) pairs.
    Enum { options: Vec<(String, String)> },
    /// Multiple-value selection from a list of (value, label, selected) triples.
    MultiEnum {
        options: Vec<(String, String)>,
        checked: Vec<bool>,
    },
    /// Boolean yes/no toggle.
    Boolean,
    /// URL input (text with URL hint).
    Url,
}

// ---------------------------------------------------------------------------
// Field
// ---------------------------------------------------------------------------

/// A single form field in an elicitation dialog.
#[derive(Debug, Clone)]
pub struct ElicitationField {
    /// JSON property name (used as the key in the response).
    pub name: String,
    /// Human-readable label shown above the field.
    pub title: String,
    /// Optional longer description shown below the label.
    pub description: Option<String>,
    /// Field kind and options.
    pub kind: ElicitationFieldKind,
    /// Current text value (for Text/Url fields).  For Enum: the selected value.
    /// For Boolean: "true" or "false".  For MultiEnum: unused (use checked list).
    pub value: String,
    /// Whether a non-empty value is required.
    pub required: bool,
    /// Validation error to show next to the field.
    pub error: Option<String>,
}

impl ElicitationField {
    /// Create a simple required text field.
    pub fn text(name: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            title: title.into(),
            description: None,
            kind: ElicitationFieldKind::Text { format: None },
            value: String::new(),
            required: true,
            error: None,
        }
    }

    /// Create an enum field.
    pub fn enum_field(
        name: impl Into<String>,
        title: impl Into<String>,
        options: Vec<(String, String)>,
    ) -> Self {
        let first_value = options.first().map(|(v, _)| v.clone()).unwrap_or_default();
        Self {
            name: name.into(),
            title: title.into(),
            description: None,
            kind: ElicitationFieldKind::Enum { options },
            value: first_value,
            required: false,
            error: None,
        }
    }

    /// Create a boolean field.
    pub fn boolean(name: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            title: title.into(),
            description: None,
            kind: ElicitationFieldKind::Boolean,
            value: "false".to_string(),
            required: false,
            error: None,
        }
    }

    /// Returns a JSON-compatible value for this field's current state.
    pub fn json_value(&self) -> serde_json::Value {
        match &self.kind {
            ElicitationFieldKind::Boolean => serde_json::Value::Bool(self.value == "true"),
            ElicitationFieldKind::MultiEnum { options, checked } => {
                let selected: Vec<serde_json::Value> = options
                    .iter()
                    .zip(checked.iter())
                    .filter(|(_, &is_checked)| is_checked)
                    .map(|((v, _), _)| serde_json::Value::String(v.clone()))
                    .collect();
                serde_json::Value::Array(selected)
            }
            _ => serde_json::Value::String(self.value.clone()),
        }
    }

    /// Validate the field and set `self.error` if invalid.  Returns true if valid.
    pub fn validate(&mut self) -> bool {
        self.error = None;
        if self.required && self.value.trim().is_empty() {
            if let ElicitationFieldKind::MultiEnum { checked, .. } = &self.kind {
                if !checked.iter().any(|&c| c) {
                    self.error = Some("Required".to_string());
                    return false;
                }
            } else if !matches!(self.kind, ElicitationFieldKind::Boolean) {
                self.error = Some("Required".to_string());
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Dialog result
// ---------------------------------------------------------------------------

/// The outcome produced when the user submits or cancels the dialog.
#[derive(Debug, Clone)]
pub enum ElicitationResult {
    /// User submitted the form; contains the field values keyed by field name.
    Submitted(HashMap<String, serde_json::Value>),
    /// User cancelled (Esc).
    Cancelled,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Full state of the MCP elicitation dialog.
#[derive(Debug, Clone, Default)]
pub struct ElicitationDialogState {
    /// Whether the dialog is currently visible.
    pub visible: bool,
    /// Name of the MCP server that triggered this request.
    pub server_name: String,
    /// Optional message/prompt from the server shown above the form.
    pub request_message: Option<String>,
    /// Form fields.
    pub fields: Vec<ElicitationField>,
    /// Index of the currently focused field.
    pub active_field: usize,
    /// Pending result; `None` until the user submits or cancels.
    result: Option<ElicitationResult>,
}

impl ElicitationDialogState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Show the dialog with the given server name, optional message, and fields.
    pub fn show(
        &mut self,
        server_name: impl Into<String>,
        message: Option<impl Into<String>>,
        fields: Vec<ElicitationField>,
    ) {
        self.server_name = server_name.into();
        self.request_message = message.map(|m| m.into());
        self.fields = fields;
        self.active_field = 0;
        self.result = None;
        self.visible = true;
    }

    /// Take the pending result, leaving `None` in place.
    pub fn take_result(&mut self) -> Option<ElicitationResult> {
        self.result.take()
    }

    /// Cancel the dialog and queue a `Cancelled` result.
    pub fn cancel(&mut self) {
        self.result = Some(ElicitationResult::Cancelled);
        self.visible = false;
    }

    /// Validate all fields and, if valid, submit the form.
    /// Returns `true` if the form was submitted, `false` if validation failed.
    pub fn submit(&mut self) -> bool {
        let valid = self.fields.iter_mut().all(|f| f.validate());
        if !valid {
            return false;
        }
        let values: HashMap<String, serde_json::Value> = self
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.json_value()))
            .collect();
        self.result = Some(ElicitationResult::Submitted(values));
        self.visible = false;
        true
    }

    /// Move focus to the next field.
    pub fn next_field(&mut self) {
        if !self.fields.is_empty() {
            self.active_field = (self.active_field + 1) % self.fields.len();
        }
    }

    /// Move focus to the previous field.
    pub fn prev_field(&mut self) {
        if !self.fields.is_empty() {
            self.active_field = (self.active_field + self.fields.len() - 1) % self.fields.len();
        }
    }

    /// Append a character to the active text/url/enum-typeahead field.
    pub fn insert_char(&mut self, ch: char) {
        let Some(field) = self.fields.get_mut(self.active_field) else {
            return;
        };
        field.error = None;
        match &mut field.kind {
            ElicitationFieldKind::Text { .. } | ElicitationFieldKind::Url => {
                field.value.push(ch);
            }
            ElicitationFieldKind::Enum { options } => {
                // Typeahead: find first option whose label starts with the current
                // value + new char (case-insensitive).
                let candidate = format!("{}{}", field.value, ch);
                let lower = candidate.to_lowercase();
                if let Some((found_v, _)) = options.iter().find(|(val, lbl)| {
                    lbl.to_lowercase().starts_with(&lower) || val.to_lowercase().starts_with(&lower)
                }) {
                    field.value = found_v.clone();
                }
            }
            _ => {}
        }
    }

    /// Delete the last character from the active text/url field.
    pub fn backspace(&mut self) {
        let Some(field) = self.fields.get_mut(self.active_field) else {
            return;
        };
        match &field.kind {
            ElicitationFieldKind::Text { .. } | ElicitationFieldKind::Url => {
                field.value.pop();
            }
            _ => {}
        }
    }

    /// For enum fields: cycle to the next option.
    pub fn cycle_enum_next(&mut self) {
        let Some(field) = self.fields.get_mut(self.active_field) else {
            return;
        };
        if let ElicitationFieldKind::Enum { options } = &field.kind {
            let options = options.clone();
            let idx = options
                .iter()
                .position(|(v, _)| *v == field.value)
                .unwrap_or(0);
            let next = (idx + 1) % options.len();
            field.value = options[next].0.clone();
        }
    }

    /// For enum fields: cycle to the previous option.
    pub fn cycle_enum_prev(&mut self) {
        let Some(field) = self.fields.get_mut(self.active_field) else {
            return;
        };
        if let ElicitationFieldKind::Enum { options } = &field.kind {
            let options = options.clone();
            let idx = options
                .iter()
                .position(|(v, _)| *v == field.value)
                .unwrap_or(0);
            let prev = if idx == 0 { options.len() - 1 } else { idx - 1 };
            field.value = options[prev].0.clone();
        }
    }

    /// For boolean or multi-enum fields: toggle the current selection.
    pub fn toggle_active(&mut self) {
        let Some(field) = self.fields.get_mut(self.active_field) else {
            return;
        };
        match &mut field.kind {
            ElicitationFieldKind::Boolean => {
                field.value = if field.value == "true" {
                    "false".to_string()
                } else {
                    "true".to_string()
                };
            }
            ElicitationFieldKind::MultiEnum { checked, .. } => {
                // Find the focused option (we track a sub-cursor via the value field)
                let sub_idx: usize = field.value.parse().unwrap_or(0);
                if let Some(c) = checked.get_mut(sub_idx) {
                    *c = !*c;
                }
            }
            _ => {}
        }
    }

    /// For multi-enum fields: move the sub-cursor down.
    pub fn multi_enum_next(&mut self) {
        let Some(field) = self.fields.get_mut(self.active_field) else {
            return;
        };
        if let ElicitationFieldKind::MultiEnum { checked, .. } = &field.kind {
            let n = checked.len();
            if n == 0 {
                return;
            }
            let cur: usize = field.value.parse().unwrap_or(0);
            field.value = ((cur + 1) % n).to_string();
        }
    }

    /// For multi-enum fields: move the sub-cursor up.
    pub fn multi_enum_prev(&mut self) {
        let Some(field) = self.fields.get_mut(self.active_field) else {
            return;
        };
        if let ElicitationFieldKind::MultiEnum { checked, .. } = &field.kind {
            let n = checked.len();
            if n == 0 {
                return;
            }
            let cur: usize = field.value.parse().unwrap_or(0);
            field.value = (if cur == 0 { n - 1 } else { cur - 1 }).to_string();
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the elicitation dialog as a centered modal overlay.
pub fn render_elicitation_dialog(state: &ElicitationDialogState, area: Rect, buf: &mut Buffer) {
    if !state.visible || area.height < 10 || area.width < 40 {
        return;
    }

    // Compute dialog size (wider if there are many fields)
    let field_count = state.fields.len() as u16;
    let needed_h = (6 + field_count * 3).min(area.height.saturating_sub(2));
    let dialog_h = needed_h.max(12).min(area.height);
    let dialog_w = 64u16.min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(dialog_w)) / 2;
    let y = area.y + (area.height.saturating_sub(dialog_h)) / 2;
    let dialog_area = Rect {
        x,
        y,
        width: dialog_w,
        height: dialog_h,
    };

    Clear.render(dialog_area, buf);

    let title = format!(" {} — Input Required ", state.server_name);
    Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .render(dialog_area, buf);

    let inner = Rect {
        x: dialog_area.x + 2,
        y: dialog_area.y + 1,
        width: dialog_area.width.saturating_sub(4),
        height: dialog_area.height.saturating_sub(2),
    };

    let mut lines: Vec<Line> = Vec::new();

    // Optional request message
    if let Some(msg) = &state.request_message {
        lines.push(Line::from(""));
        for chunk in wrap_str(msg, inner.width as usize) {
            lines.push(Line::from(vec![Span::styled(
                chunk,
                Style::default().fg(Color::White),
            )]));
        }
        lines.push(Line::from(""));
    } else {
        lines.push(Line::from(""));
    }

    // Fields
    for (idx, field) in state.fields.iter().enumerate() {
        let focused = idx == state.active_field;
        let label_style = if focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };

        // Label line: "> Label  [required]"
        let mut label_spans = vec![
            Span::styled(if focused { "> " } else { "  " }, label_style),
            Span::styled(field.title.clone(), label_style),
        ];
        if field.required {
            label_spans.push(Span::styled(" *", Style::default().fg(Color::Red)));
        }
        if let Some(err) = &field.error {
            label_spans.push(Span::styled(
                format!("  ← {err}"),
                Style::default().fg(Color::Red),
            ));
        }
        lines.push(Line::from(label_spans));

        // Value line
        let value_line = render_field_value_line(field, focused, inner.width as usize);
        lines.push(value_line);

        // Optional description
        if let Some(desc) = &field.description {
            lines.push(Line::from(vec![Span::styled(
                format!("   {desc}"),
                Style::default().fg(Color::DarkGray),
            )]));
        }
    }

    // Hint line
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "  Tab: next field   Enter: submit   Esc: cancel",
        Style::default().fg(Color::DarkGray),
    )]));

    // Render with scroll if needed
    let total = lines.len();
    let visible_h = inner.height as usize;
    let scroll = if total > visible_h {
        total - visible_h
    } else {
        0
    };
    let visible_lines: Vec<Line> = lines.into_iter().skip(scroll).collect();
    Paragraph::new(visible_lines).render(inner, buf);
}

fn render_field_value_line<'a>(
    field: &'a ElicitationField,
    focused: bool,
    width: usize,
) -> Line<'a> {
    let input_bg = if focused {
        Color::Rgb(30, 30, 60)
    } else {
        Color::Rgb(20, 20, 30)
    };
    let input_fg = if focused { Color::White } else { Color::Gray };

    match &field.kind {
        ElicitationFieldKind::Text { .. } | ElicitationFieldKind::Url => {
            let display = if field.value.is_empty() && !focused {
                "(empty)".to_string()
            } else {
                // Show cursor at end when focused
                let v = field.value.clone();
                if focused {
                    format!("{v}_")
                } else {
                    v
                }
            };
            let padded = format!("   {:<width$}", display, width = width.saturating_sub(3));
            Line::from(vec![Span::styled(
                padded,
                Style::default().fg(input_fg).bg(input_bg),
            )])
        }

        ElicitationFieldKind::Enum { options } => {
            let label = options
                .iter()
                .find(|(v, _)| *v == field.value)
                .map(|(_, l)| l.as_str())
                .unwrap_or(field.value.as_str());
            let hint = if focused {
                " ◀ ▶ arrows to change"
            } else {
                ""
            };
            Line::from(vec![
                Span::styled("   ", Style::default()),
                Span::styled(
                    format!(" {label} "),
                    Style::default()
                        .fg(Color::White)
                        .bg(input_bg)
                        .add_modifier(if focused {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled(hint, Style::default().fg(Color::DarkGray)),
            ])
        }

        ElicitationFieldKind::MultiEnum { options, checked } => {
            let sub_cursor: usize = field.value.parse().unwrap_or(0);
            let mut spans: Vec<Span> = vec![Span::raw("   ")];
            for (i, ((v, lbl), &is_checked)) in options.iter().zip(checked.iter()).enumerate() {
                let on_cursor = focused && i == sub_cursor;
                let check = if is_checked { "[x] " } else { "[ ] " };
                let style = if on_cursor {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else if is_checked {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let _ = v; // suppress unused warning
                spans.push(Span::styled(format!("{check}{lbl}  "), style));
            }
            Line::from(spans)
        }

        ElicitationFieldKind::Boolean => {
            let checked = field.value == "true";
            let (yes_style, no_style) = if checked {
                (
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                    Style::default().fg(Color::DarkGray),
                )
            } else {
                (
                    Style::default().fg(Color::DarkGray),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Red)
                        .add_modifier(Modifier::BOLD),
                )
            };
            let hint = if focused {
                " Space/Enter to toggle"
            } else {
                ""
            };
            Line::from(vec![
                Span::raw("   "),
                Span::styled(" Yes ", yes_style),
                Span::raw("  "),
                Span::styled(" No ", no_style),
                Span::styled(hint, Style::default().fg(Color::DarkGray)),
            ])
        }
    }
}

/// Simple word-wrap helper for the request message.
fn wrap_str(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![s.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in s.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
        } else if current.len() + 1 + word.len() <= width {
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
    lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn make_dialog() -> ElicitationDialogState {
        let mut s = ElicitationDialogState::new();
        s.show(
            "test-server",
            Some("Please provide your credentials"),
            vec![
                ElicitationField::text("username", "Username"),
                ElicitationField::boolean("remember", "Remember me"),
                ElicitationField::enum_field(
                    "env",
                    "Environment",
                    vec![
                        ("prod".to_string(), "Production".to_string()),
                        ("dev".to_string(), "Development".to_string()),
                        ("staging".to_string(), "Staging".to_string()),
                    ],
                ),
            ],
        );
        s
    }

    #[test]
    fn elicitation_show_sets_visible() {
        let s = make_dialog();
        assert!(s.visible);
        assert_eq!(s.server_name, "test-server");
        assert_eq!(s.fields.len(), 3);
    }

    #[test]
    fn elicitation_cancel_produces_result() {
        let mut s = make_dialog();
        s.cancel();
        assert!(!s.visible);
        let result = s.take_result();
        assert!(matches!(result, Some(ElicitationResult::Cancelled)));
    }

    #[test]
    fn elicitation_submit_with_required_empty_fails() {
        let mut s = make_dialog();
        // username is required and empty — submit should fail
        let ok = s.submit();
        assert!(!ok, "submit should fail when required text field is empty");
        assert!(s.visible, "dialog should stay open after failed submit");
        assert!(s.fields[0].error.is_some());
    }

    #[test]
    fn elicitation_submit_with_required_filled_succeeds() {
        let mut s = make_dialog();
        s.fields[0].value = "alice".to_string();
        let ok = s.submit();
        assert!(ok);
        assert!(!s.visible);
        let result = s.take_result();
        if let Some(ElicitationResult::Submitted(map)) = result {
            assert_eq!(
                map.get("username"),
                Some(&serde_json::Value::String("alice".to_string()))
            );
        } else {
            panic!("expected Submitted result");
        }
    }

    #[test]
    fn elicitation_boolean_toggle() {
        let mut s = make_dialog();
        s.active_field = 1; // remember field (boolean)
        assert_eq!(s.fields[1].value, "false");
        s.toggle_active();
        assert_eq!(s.fields[1].value, "true");
        s.toggle_active();
        assert_eq!(s.fields[1].value, "false");
    }

    #[test]
    fn elicitation_boolean_json_value() {
        let mut f = ElicitationField::boolean("confirm", "Confirm");
        f.value = "true".to_string();
        assert_eq!(f.json_value(), serde_json::Value::Bool(true));
        f.value = "false".to_string();
        assert_eq!(f.json_value(), serde_json::Value::Bool(false));
    }

    #[test]
    fn elicitation_enum_cycle_next() {
        let mut s = make_dialog();
        s.active_field = 2; // env enum
        assert_eq!(s.fields[2].value, "prod");
        s.cycle_enum_next();
        assert_eq!(s.fields[2].value, "dev");
        s.cycle_enum_next();
        assert_eq!(s.fields[2].value, "staging");
        s.cycle_enum_next(); // wraps back to first
        assert_eq!(s.fields[2].value, "prod");
    }

    #[test]
    fn elicitation_enum_cycle_prev() {
        let mut s = make_dialog();
        s.active_field = 2;
        s.cycle_enum_prev(); // wraps to last
        assert_eq!(s.fields[2].value, "staging");
        s.cycle_enum_prev();
        assert_eq!(s.fields[2].value, "dev");
    }

    #[test]
    fn elicitation_field_navigation() {
        let mut s = make_dialog();
        assert_eq!(s.active_field, 0);
        s.next_field();
        assert_eq!(s.active_field, 1);
        s.next_field();
        assert_eq!(s.active_field, 2);
        s.next_field(); // wraps
        assert_eq!(s.active_field, 0);
        s.prev_field(); // wraps backward
        assert_eq!(s.active_field, 2);
    }

    #[test]
    fn elicitation_text_insert_backspace() {
        let mut s = make_dialog();
        s.active_field = 0;
        s.insert_char('a');
        s.insert_char('l');
        s.insert_char('i');
        s.insert_char('c');
        s.insert_char('e');
        assert_eq!(s.fields[0].value, "alice");
        s.backspace();
        assert_eq!(s.fields[0].value, "alic");
    }

    #[test]
    fn elicitation_multi_enum() {
        let mut s = ElicitationDialogState::new();
        s.show(
            "srv",
            None::<String>,
            vec![ElicitationField {
                name: "opts".to_string(),
                title: "Options".to_string(),
                description: None,
                kind: ElicitationFieldKind::MultiEnum {
                    options: vec![
                        ("a".to_string(), "Alpha".to_string()),
                        ("b".to_string(), "Beta".to_string()),
                        ("c".to_string(), "Gamma".to_string()),
                    ],
                    checked: vec![false, false, false],
                },
                value: "0".to_string(), // sub-cursor at index 0
                required: false,
                error: None,
            }],
        );
        s.active_field = 0;
        // Toggle item 0
        s.toggle_active();
        if let ElicitationFieldKind::MultiEnum { checked, .. } = &s.fields[0].kind {
            assert!(checked[0]);
            assert!(!checked[1]);
        }
        // Move sub-cursor to item 2 and toggle
        s.multi_enum_next();
        s.multi_enum_next();
        s.toggle_active();
        if let ElicitationFieldKind::MultiEnum { checked, .. } = &s.fields[0].kind {
            assert!(checked[0]);
            assert!(!checked[1]);
            assert!(checked[2]);
        }
        // Check JSON value
        let v = s.fields[0].json_value();
        if let serde_json::Value::Array(arr) = v {
            assert_eq!(arr.len(), 2);
            assert!(arr.contains(&serde_json::Value::String("a".to_string())));
            assert!(arr.contains(&serde_json::Value::String("c".to_string())));
        } else {
            panic!("expected array");
        }
    }

    #[test]
    fn elicitation_render_smoke() {
        let s = make_dialog();
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        };
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_elicitation_dialog(&s, area, &mut buf);
        let rendered = buf
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(rendered.contains("test-server") || rendered.contains("Input Required"));
    }

    #[test]
    fn elicitation_not_rendered_when_invisible() {
        let s = ElicitationDialogState::new();
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        };
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_elicitation_dialog(&s, area, &mut buf);
        let rendered = buf
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(!rendered.contains("Input Required"));
    }

    #[test]
    fn wrap_str_short_text_unchanged() {
        let result = wrap_str("hello world", 40);
        assert_eq!(result, vec!["hello world"]);
    }

    #[test]
    fn wrap_str_wraps_at_width() {
        let result = wrap_str("hello world foo bar baz", 12);
        // "hello world" = 11 chars, "foo bar baz" needs further wrapping
        assert!(result.len() > 1);
        for line in &result {
            assert!(line.len() <= 12, "line too long: {line:?}");
        }
    }
}
