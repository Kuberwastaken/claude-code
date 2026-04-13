// invalid_config_dialog.rs — Startup dialog for malformed settings.json or AGENTS.md.
//
// Mirrors TS `InvalidConfigDialog` / `InvalidSettingsDialog`:
// - Displayed on startup when config parsing fails.
// - Shows a red-bordered box with the error message.
// - Dismissed by pressing Enter or Escape; user can then fix the file and restart.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};
use ratatui::Frame;

use crate::overlays::centered_rect;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// State for the invalid-config startup dialog.
#[derive(Debug, Default)]
pub struct InvalidConfigDialogState {
    /// Whether the dialog is currently visible.
    pub visible: bool,
    /// Kind of config error.
    pub kind: InvalidConfigKind,
    /// Human-readable error message (may be multi-line).
    pub error_message: String,
    /// Scroll offset for long error messages.
    pub scroll: u16,
}

/// What kind of configuration is broken.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum InvalidConfigKind {
    #[default]
    Settings,
    ClaudeMd,
    Generic,
}

impl InvalidConfigDialogState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Show the dialog with a settings.json error.
    pub fn show_settings_error(error: &str) -> Self {
        Self {
            visible: true,
            kind: InvalidConfigKind::Settings,
            error_message: error.to_string(),
            scroll: 0,
        }
    }

    /// Show the dialog with a AGENTS.md parse error.
    pub fn show_claude_md_error(error: &str) -> Self {
        Self {
            visible: true,
            kind: InvalidConfigKind::ClaudeMd,
            error_message: error.to_string(),
            scroll: 0,
        }
    }

    pub fn dismiss(&mut self) {
        self.visible = false;
        self.scroll = 0;
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn scroll_down(&mut self, max: u16) {
        if self.scroll + 1 < max {
            self.scroll += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the invalid-config dialog over the frame.
pub fn render_invalid_config_dialog(
    frame: &mut Frame,
    state: &InvalidConfigDialogState,
    area: Rect,
) {
    if !state.visible {
        return;
    }

    let dialog_width = 80u16.min(area.width.saturating_sub(4));
    let dialog_height = 24u16.min(area.height.saturating_sub(4));
    let dialog_area = centered_rect(dialog_width, dialog_height, area);

    frame.render_widget(Clear, dialog_area);

    let title = match state.kind {
        InvalidConfigKind::Settings => " Invalid Settings ",
        InvalidConfigKind::ClaudeMd => " Invalid AGENTS.md ",
        InvalidConfigKind::Generic => " Configuration Error ",
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![Span::styled(
            title,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )]))
        .border_style(Style::default().fg(Color::Red));

    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    // Build content lines
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Subtitle
    let subtitle = match state.kind {
        InvalidConfigKind::Settings => "~/.claurst/settings.json could not be parsed.",
        InvalidConfigKind::ClaudeMd => "AGENTS.md could not be parsed.",
        InvalidConfigKind::Generic => "A configuration file could not be parsed.",
    };
    lines.push(Line::from(vec![Span::styled(
        subtitle.to_string(),
        Style::default().fg(Color::Yellow),
    )]));
    lines.push(Line::from(""));

    // Error detail
    lines.push(Line::from(vec![Span::styled(
        "Error:".to_string(),
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    )]));
    for error_line in state.error_message.lines() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(error_line.to_string(), Style::default().fg(Color::White)),
        ]));
    }
    lines.push(Line::from(""));

    // Instructions
    lines.push(Line::from(vec![Span::styled(
        "To resolve:".to_string(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]));
    let instructions = match state.kind {
        InvalidConfigKind::Settings => vec![
            "  1. Open ~/.claurst/settings.json in a text editor.",
            "  2. Fix the JSON syntax error shown above.",
            "  3. Restart Claurst.",
        ],
        InvalidConfigKind::ClaudeMd => vec![
            "  1. Open the AGENTS.md file shown above in a text editor.",
            "  2. Fix the syntax error.",
            "  3. Restart Claurst.",
        ],
        InvalidConfigKind::Generic => vec![
            "  1. Fix the configuration file shown above.",
            "  2. Restart Claurst.",
        ],
    };
    for instr in instructions {
        lines.push(Line::from(vec![Span::styled(
            instr.to_string(),
            Style::default().fg(Color::Gray),
        )]));
    }
    lines.push(Line::from(""));

    // Dismiss hint
    lines.push(Line::from(vec![Span::styled(
        "  Press Enter or Escape to dismiss and continue with defaults.",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )]));

    let total_lines = lines.len() as u16;
    let visible_height = inner.height;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = state.scroll.min(max_scroll);

    Paragraph::new(lines)
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false })
        .render(inner, frame.buffer_mut());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn invalid_config_dialog_state_defaults() {
        let state = InvalidConfigDialogState::new();
        assert!(!state.visible);
        assert_eq!(state.kind, InvalidConfigKind::Settings);
        assert!(state.error_message.is_empty());
    }

    #[test]
    fn invalid_config_dialog_show_settings_error() {
        let state = InvalidConfigDialogState::show_settings_error("unexpected token at line 3");
        assert!(state.visible);
        assert_eq!(state.kind, InvalidConfigKind::Settings);
        assert!(state.error_message.contains("unexpected token"));
    }

    #[test]
    fn invalid_config_dialog_dismiss() {
        let mut state = InvalidConfigDialogState::show_settings_error("err");
        state.dismiss();
        assert!(!state.visible);
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn invalid_config_dialog_renders_without_panic() {
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let state = InvalidConfigDialogState::show_settings_error("JSON parse error: unexpected ,");

        terminal
            .draw(|frame| {
                let area = frame.area();
                render_invalid_config_dialog(frame, &state, area);
            })
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("Invalid Settings") || content.contains("Configuration"));
    }

    #[test]
    fn invalid_config_dialog_shows_error_text() {
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let state = InvalidConfigDialogState::show_settings_error("missing field `model`");

        terminal
            .draw(|frame| {
                let area = frame.area();
                render_invalid_config_dialog(frame, &state, area);
            })
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("missing field") || content.contains("Error"));
    }

    #[test]
    fn invalid_config_dialog_hidden_by_default_renders_nothing() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let state = InvalidConfigDialogState::new(); // visible = false
        let snapshot_before = terminal.backend().buffer().clone();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render_invalid_config_dialog(frame, &state, area);
            })
            .unwrap();

        // Buffer should be unchanged since dialog is hidden
        let buf = terminal.backend().buffer().clone();
        assert_eq!(buf.content(), snapshot_before.content());
    }
}
