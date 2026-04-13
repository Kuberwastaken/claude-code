// bypass_permissions_dialog.rs — Startup confirmation dialog for --dangerously-skip-permissions.
//
// Mirrors TS `BypassPermissionsModeDialog.tsx`:
// - Displayed at startup when the session was launched with bypass-permissions mode.
// - Shows a red-bordered warning explaining the risks.
// - User must explicitly accept ("Yes, I accept") or decline ("No, exit").
// - If declined the app exits immediately.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};
use ratatui::Frame;

use crate::overlays::centered_rect;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// State for the bypass-permissions startup confirmation dialog.
#[derive(Debug, Default, Clone)]
pub struct BypassPermissionsDialogState {
    /// Whether the dialog is currently visible.
    pub visible: bool,
    /// 0 = "No, exit" selected; 1 = "Yes, I accept" selected
    pub selected: usize,
}

impl BypassPermissionsDialogState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Show the dialog (called at startup when bypass mode is active).
    pub fn show(&mut self) {
        self.visible = true;
        self.selected = 0;
    }

    /// Move selection up (wraps).
    pub fn select_prev(&mut self) {
        self.selected = if self.selected == 0 { 1 } else { 0 };
    }

    /// Move selection down (wraps).
    pub fn select_next(&mut self) {
        self.selected = if self.selected == 1 { 0 } else { 1 };
    }

    /// Returns `true` if the currently-selected option is "Yes, I accept".
    pub fn is_accept_selected(&self) -> bool {
        self.selected == 1
    }

    pub fn dismiss(&mut self) {
        self.visible = false;
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the bypass-permissions confirmation dialog over the frame.
pub fn render_bypass_permissions_dialog(
    frame: &mut Frame,
    state: &BypassPermissionsDialogState,
    area: Rect,
) {
    if !state.visible {
        return;
    }

    let dialog_width = 72u16.min(area.width.saturating_sub(4));
    let dialog_height = 22u16.min(area.height.saturating_sub(4));
    let dialog_area = centered_rect(dialog_width, dialog_height, area);

    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![Span::styled(
            " WARNING: Bypass Permissions Mode ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )]))
        .border_style(Style::default().fg(Color::Red));

    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Body text (matches TS dialog copy)
    lines.push(Line::from(vec![Span::styled(
        "Claurst running in Bypass Permissions mode",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "In Bypass Permissions mode, Claurst will NOT ask for your",
        Style::default().fg(Color::White),
    )]));
    lines.push(Line::from(vec![Span::styled(
        "approval before running potentially dangerous commands.",
        Style::default().fg(Color::White),
    )]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "This mode should only be used in a sandboxed container or VM",
        Style::default().fg(Color::DarkGray),
    )]));
    lines.push(Line::from(vec![Span::styled(
        "that has restricted internet access and can easily be restored",
        Style::default().fg(Color::DarkGray),
    )]));
    lines.push(Line::from(vec![Span::styled(
        "if damaged.",
        Style::default().fg(Color::DarkGray),
    )]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "By proceeding, you accept all responsibility for actions taken",
        Style::default().fg(Color::DarkGray),
    )]));
    lines.push(Line::from(vec![Span::styled(
        "while running in Bypass Permissions mode.",
        Style::default().fg(Color::DarkGray),
    )]));
    lines.push(Line::from(""));
    lines.push(Line::from(""));

    // Options
    let opt_no_style = if state.selected == 0 {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(Color::White)
    };
    let opt_yes_style = if state.selected == 1 {
        Style::default()
            .fg(Color::Red)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(Color::Red)
    };

    lines.push(Line::from(vec![
        Span::styled("  [1] ", Style::default().fg(Color::DarkGray)),
        Span::styled("No, exit", opt_no_style),
        Span::raw("        "),
        Span::styled("  [2] ", Style::default().fg(Color::DarkGray)),
        Span::styled("Yes, I accept", opt_yes_style),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "  ↑↓ or 1/2 to select  ·  Enter to confirm",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )]));

    Paragraph::new(lines)
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
    fn bypass_dialog_defaults_hidden() {
        let state = BypassPermissionsDialogState::new();
        assert!(!state.visible);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn bypass_dialog_show_sets_visible() {
        let mut state = BypassPermissionsDialogState::new();
        state.show();
        assert!(state.visible);
        assert_eq!(state.selected, 0); // "No, exit" selected by default
    }

    #[test]
    fn bypass_dialog_navigate() {
        let mut state = BypassPermissionsDialogState::new();
        state.show();
        assert!(!state.is_accept_selected());
        state.select_next();
        assert!(state.is_accept_selected());
        state.select_prev();
        assert!(!state.is_accept_selected());
    }

    #[test]
    fn bypass_dialog_navigate_wraps() {
        let mut state = BypassPermissionsDialogState::new();
        state.show();
        state.select_prev(); // wrap from 0 → 1
        assert_eq!(state.selected, 1);
        state.select_next(); // wrap from 1 → 0
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn bypass_dialog_dismiss() {
        let mut state = BypassPermissionsDialogState::new();
        state.show();
        state.dismiss();
        assert!(!state.visible);
    }

    #[test]
    fn bypass_dialog_renders_without_panic() {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut state = BypassPermissionsDialogState::new();
        state.show();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_bypass_permissions_dialog(frame, &state, area);
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("WARNING") || content.contains("Bypass"));
    }

    #[test]
    fn bypass_dialog_shows_both_options() {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut state = BypassPermissionsDialogState::new();
        state.show();
        terminal
            .draw(|frame| {
                render_bypass_permissions_dialog(frame, &state, frame.area());
            })
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("No") || content.contains("exit"));
        assert!(content.contains("accept") || content.contains("Yes"));
    }

    #[test]
    fn bypass_dialog_hidden_renders_nothing() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let state = BypassPermissionsDialogState::new(); // visible = false
        let before = terminal.backend().buffer().clone();
        terminal
            .draw(|frame| {
                render_bypass_permissions_dialog(frame, &state, frame.area());
            })
            .unwrap();
        assert_eq!(terminal.backend().buffer().content(), before.content());
    }
}
