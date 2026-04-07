// voice_mode_notice.rs — VoiceModeNotice surface.
//
// Shown when the user's account has voice mode available but it isn't yet
// enabled. Appears as a one-time dismissable notice below the welcome header.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Voice mode availability notice.
#[derive(Debug, Clone, Default)]
pub struct VoiceModeNoticeState {
    /// Whether the notice is visible.
    pub visible: bool,
    /// Whether voice mode is currently enabled by the user.
    pub voice_enabled: bool,
    /// Whether the user has dismissed this notice.
    dismissed: bool,
}

impl VoiceModeNoticeState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Show the notice if voice mode is available but not yet enabled.
    pub fn show_if_available(&mut self, voice_available: bool, voice_enabled: bool) {
        if self.dismissed {
            return;
        }
        self.voice_enabled = voice_enabled;
        self.visible = voice_available && !voice_enabled;
    }

    /// Update voice-enabled status (called when user toggles voice).
    pub fn update_voice_enabled(&mut self, enabled: bool) {
        self.voice_enabled = enabled;
        if enabled {
            // Auto-dismiss when user enables voice
            self.visible = false;
            self.dismissed = true;
        }
    }

    /// Dismiss the notice for this session.
    pub fn dismiss(&mut self) {
        self.visible = false;
        self.dismissed = true;
    }

    /// Height the notice occupies (0 if not visible).
    pub fn height(&self) -> u16 {
        if self.visible {
            2
        } else {
            0
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the voice mode availability notice.
pub fn render_voice_mode_notice(state: &VoiceModeNoticeState, area: Rect, buf: &mut Buffer) {
    if !state.visible || area.height == 0 {
        return;
    }

    let notice_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: state.height().min(area.height),
    };

    Clear.render(notice_area, buf);

    let lines = vec![
        Line::from(vec![
            Span::styled(" \u{1f3a4} ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "Voice mode is available! Use ",
                Style::default().fg(Color::White),
            ),
            Span::styled(
                "Alt+V",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " to start recording, or ",
                Style::default().fg(Color::White),
            ),
            Span::styled(
                "/voice",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to configure.  ", Style::default().fg(Color::White)),
            Span::styled("[Esc to dismiss]", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
    ];

    Paragraph::new(lines)
        .style(Style::default().bg(Color::Rgb(30, 30, 50)))
        .render(notice_area, buf);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn voice_notice_show_when_available_not_enabled() {
        let mut state = VoiceModeNoticeState::new();
        state.show_if_available(true, false);
        assert!(state.visible);
    }

    #[test]
    fn voice_notice_hidden_when_already_enabled() {
        let mut state = VoiceModeNoticeState::new();
        state.show_if_available(true, true);
        assert!(!state.visible);
    }

    #[test]
    fn voice_notice_hidden_when_not_available() {
        let mut state = VoiceModeNoticeState::new();
        state.show_if_available(false, false);
        assert!(!state.visible);
    }

    #[test]
    fn voice_notice_dismiss() {
        let mut state = VoiceModeNoticeState::new();
        state.show_if_available(true, false);
        state.dismiss();
        assert!(!state.visible);
        // Should not re-show after dismiss
        state.show_if_available(true, false);
        assert!(!state.visible);
    }

    #[test]
    fn voice_notice_auto_dismiss_on_enable() {
        let mut state = VoiceModeNoticeState::new();
        state.show_if_available(true, false);
        assert!(state.visible);
        state.update_voice_enabled(true);
        assert!(!state.visible);
        // Should stay dismissed
        state.show_if_available(true, false);
        assert!(!state.visible);
    }

    #[test]
    fn voice_notice_render_smoke() {
        let mut state = VoiceModeNoticeState::new();
        state.show_if_available(true, false);
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 4,
        };
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_voice_mode_notice(&state, area, &mut buf);
        let rendered = buf
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(rendered.contains("Voice mode"));
        assert!(rendered.contains("Alt+V"));
    }

    #[test]
    fn voice_notice_not_rendered_when_invisible() {
        let state = VoiceModeNoticeState::new();
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 4,
        };
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_voice_mode_notice(&state, area, &mut buf);
        let rendered = buf
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(!rendered.contains("Voice"));
    }
}
