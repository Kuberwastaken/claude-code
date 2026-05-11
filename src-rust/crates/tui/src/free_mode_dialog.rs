// free_mode_dialog.rs — Setup dialog for the composite "Free" provider.
//
// Walks the user through the free-mode caveats (worse context management,
// rate-limited free pool, $10 OpenRouter top-up tip) and collects two API
// keys in sequence: OpenCode Zen (primary) and OpenRouter (fallback).
//
// Layout:
//   ┌─ Connect Free (Zen → OpenRouter) ────────────── esc ┐
//   │                                                     │
//   │  Free mode chains OpenCode Zen → OpenRouter free.   │
//   │  ⚠  Context management is worse than paid models.   │
//   │     Long sessions will truncate aggressively.       │
//   │                                                     │
//   │  TIP — Depositing $10 on OpenRouter still keeps     │
//   │  the free models free but unlocks much higher       │
//   │  per-minute quotas.                                 │
//   │                                                     │
//   │  OpenCode Zen API key:                              │
//   │   ••••••••AbCd_                                     │
//   │   get one at  opencode.ai/auth                      │
//   │                                                     │
//   │  OpenRouter API key:                                │
//   │   paste your API key here...                        │
//   │   get one at  openrouter.ai/keys                    │
//   │                                                     │
//   │  tab switch field   enter confirm                   │
//   └─────────────────────────────────────────────────────┘

use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::overlays::{centered_rect, render_dark_overlay, render_dialog_bg, CLAURST_PANEL_BG};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreeModeField {
    ZenKey,
    OpenRouterKey,
}

pub struct FreeModeDialogState {
    pub visible: bool,
    pub zen_key: String,
    pub openrouter_key: String,
    pub active_field: FreeModeField,
}

impl Default for FreeModeDialogState {
    fn default() -> Self {
        Self::new()
    }
}

impl FreeModeDialogState {
    pub fn new() -> Self {
        Self {
            visible: false,
            zen_key: String::new(),
            openrouter_key: String::new(),
            active_field: FreeModeField::ZenKey,
        }
    }

    pub fn open(&mut self, zen_existing: Option<String>, or_existing: Option<String>) {
        self.visible = true;
        self.zen_key = zen_existing.unwrap_or_default();
        self.openrouter_key = or_existing.unwrap_or_default();
        // Start on whichever field is still empty.
        self.active_field = if self.zen_key.is_empty() {
            FreeModeField::ZenKey
        } else {
            FreeModeField::OpenRouterKey
        };
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.zen_key.clear();
        self.openrouter_key.clear();
        self.active_field = FreeModeField::ZenKey;
    }

    pub fn switch_field(&mut self) {
        self.active_field = match self.active_field {
            FreeModeField::ZenKey => FreeModeField::OpenRouterKey,
            FreeModeField::OpenRouterKey => FreeModeField::ZenKey,
        };
    }

    pub fn insert_char(&mut self, c: char) {
        match self.active_field {
            FreeModeField::ZenKey => self.zen_key.push(c),
            FreeModeField::OpenRouterKey => self.openrouter_key.push(c),
        }
    }

    pub fn backspace(&mut self) {
        match self.active_field {
            FreeModeField::ZenKey => {
                self.zen_key.pop();
            }
            FreeModeField::OpenRouterKey => {
                self.openrouter_key.pop();
            }
        }
    }

    /// At least one key must be present to enable the provider — the
    /// composite still functions if just one upstream is authenticated
    /// (the other side just gets skipped on fallback).
    pub fn can_submit(&self) -> bool {
        !self.zen_key.trim().is_empty() || !self.openrouter_key.trim().is_empty()
    }

    /// Consume the dialog state, returning `(zen_key, openrouter_key)`.
    pub fn take_values(&mut self) -> (String, String) {
        let zen = self.zen_key.trim().to_string();
        let or = self.openrouter_key.trim().to_string();
        self.close();
        (zen, or)
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn mask_key(input: &str) -> String {
    if input.is_empty() {
        "paste your API key here...".to_string()
    } else {
        let chars: Vec<char> = input.chars().collect();
        if chars.len() <= 4 {
            input.to_string()
        } else {
            let tail: String = chars[chars.len() - 4..].iter().collect();
            format!("{}{}", "\u{2022}".repeat(chars.len() - 4), tail)
        }
    }
}

pub fn render_free_mode_dialog(frame: &mut Frame, state: &FreeModeDialogState, area: Rect) {
    if !state.visible {
        return;
    }

    let pink = Color::Rgb(233, 30, 99);
    let dim = Color::Rgb(90, 90, 90);
    let muted = Color::Rgb(180, 180, 180);
    let warn = Color::Rgb(255, 175, 60);
    let tip = Color::Rgb(120, 210, 150);
    let dialog_bg = CLAURST_PANEL_BG;

    render_dark_overlay(frame, area);

    let width = 76u16.min(area.width.saturating_sub(4));
    let height = 22u16.min(area.height.saturating_sub(2));
    let dialog_area = centered_rect(width, height, area);
    render_dialog_bg(frame, dialog_area);

    let inner = Rect {
        x: dialog_area.x + 1,
        y: dialog_area.y + 1,
        width: dialog_area.width.saturating_sub(2),
        height: dialog_area.height.saturating_sub(2),
    };

    let title_text = "Connect Free (Zen \u{2192} OpenRouter)";
    let title_pad = inner
        .width
        .saturating_sub(title_text.chars().count() as u16 + 5) as usize;

    let zen_style = if state.active_field == FreeModeField::ZenKey {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let or_style = if state.active_field == FreeModeField::OpenRouterKey {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let zen_masked = mask_key(&state.zen_key);
    let or_masked = mask_key(&state.openrouter_key);

    let zen_input_style = if state.zen_key.is_empty() { Style::default().fg(dim) } else { zen_style };
    let or_input_style = if state.openrouter_key.is_empty() { Style::default().fg(dim) } else { or_style };

    let confirm_hint = if state.can_submit() {
        " enter confirm"
    } else {
        " paste at least one key"
    };

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Title row
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
    lines.push(Line::from(""));

    // Description
    lines.push(Line::from(vec![Span::styled(
        " Free mode chains OpenCode Zen (primary) \u{2192} OpenRouter free (fallback).",
        Style::default().fg(muted),
    )]));

    // Warning
    lines.push(Line::from(vec![
        Span::styled(" \u{26a0} ", Style::default().fg(warn).add_modifier(Modifier::BOLD)),
        Span::styled(
            "Context management is worse than paid models \u{2014} long sessions",
            Style::default().fg(warn),
        ),
    ]));
    lines.push(Line::from(vec![Span::styled(
        "   will truncate aggressively, and free pools are rate-limited.",
        Style::default().fg(warn),
    )]));
    lines.push(Line::from(""));

    // OpenRouter $10 tip
    lines.push(Line::from(vec![
        Span::styled(" TIP ", Style::default().fg(tip).add_modifier(Modifier::BOLD)),
        Span::styled(
            "Depositing $10 on OpenRouter keeps the free models free but",
            Style::default().fg(tip),
        ),
    ]));
    lines.push(Line::from(vec![Span::styled(
        "     unlocks much higher per-minute quotas \u{2014} worth it for daily use.",
        Style::default().fg(tip),
    )]));
    lines.push(Line::from(""));

    // Zen field
    lines.push(Line::from(vec![Span::styled(
        " OpenCode Zen API key:",
        Style::default().fg(muted),
    )]));
    lines.push(Line::from(vec![
        Span::styled(format!("  {}", zen_masked), zen_input_style),
        Span::styled(
            if state.active_field == FreeModeField::ZenKey { "_" } else { "" },
            Style::default().fg(pink),
        ),
    ]));
    lines.push(Line::from(vec![Span::styled(
        "  get one at  opencode.ai/auth",
        Style::default().fg(dim),
    )]));
    lines.push(Line::from(""));

    // OpenRouter field
    lines.push(Line::from(vec![Span::styled(
        " OpenRouter API key:",
        Style::default().fg(muted),
    )]));
    lines.push(Line::from(vec![
        Span::styled(format!("  {}", or_masked), or_input_style),
        Span::styled(
            if state.active_field == FreeModeField::OpenRouterKey { "_" } else { "" },
            Style::default().fg(pink),
        ),
    ]));
    lines.push(Line::from(vec![Span::styled(
        "  get one at  openrouter.ai/keys",
        Style::default().fg(dim),
    )]));
    lines.push(Line::from(""));

    // Footer
    lines.push(Line::from(vec![
        Span::styled(" tab", Style::default().fg(dim)),
        Span::styled(" switch field   ", Style::default().fg(dim)),
        Span::styled(confirm_hint, Style::default().fg(dim)),
    ]));

    let para = Paragraph::new(lines).bg(dialog_bg);
    frame.render_widget(para, inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_hidden() {
        let s = FreeModeDialogState::new();
        assert!(!s.visible);
        assert_eq!(s.active_field, FreeModeField::ZenKey);
    }

    #[test]
    fn open_starts_on_zen_when_empty() {
        let mut s = FreeModeDialogState::new();
        s.open(None, None);
        assert!(s.visible);
        assert_eq!(s.active_field, FreeModeField::ZenKey);
    }

    #[test]
    fn open_skips_to_openrouter_when_zen_present() {
        let mut s = FreeModeDialogState::new();
        s.open(Some("zen-key".into()), None);
        assert_eq!(s.active_field, FreeModeField::OpenRouterKey);
    }

    #[test]
    fn switch_field_toggles() {
        let mut s = FreeModeDialogState::new();
        s.open(None, None);
        assert_eq!(s.active_field, FreeModeField::ZenKey);
        s.switch_field();
        assert_eq!(s.active_field, FreeModeField::OpenRouterKey);
        s.switch_field();
        assert_eq!(s.active_field, FreeModeField::ZenKey);
    }

    #[test]
    fn insert_char_writes_to_active_field() {
        let mut s = FreeModeDialogState::new();
        s.open(None, None);
        s.insert_char('a');
        s.insert_char('b');
        assert_eq!(s.zen_key, "ab");
        s.switch_field();
        s.insert_char('x');
        assert_eq!(s.openrouter_key, "x");
        assert_eq!(s.zen_key, "ab");
    }

    #[test]
    fn backspace_removes_last_from_active_field() {
        let mut s = FreeModeDialogState::new();
        s.open(None, None);
        s.insert_char('a');
        s.insert_char('b');
        s.backspace();
        assert_eq!(s.zen_key, "a");
    }

    #[test]
    fn can_submit_requires_at_least_one_key() {
        let mut s = FreeModeDialogState::new();
        s.open(None, None);
        assert!(!s.can_submit());
        s.insert_char('k');
        assert!(s.can_submit());
    }

    #[test]
    fn take_values_returns_trimmed_pair_and_closes() {
        let mut s = FreeModeDialogState::new();
        s.open(None, None);
        s.insert_char(' ');
        s.insert_char('a');
        s.insert_char(' ');
        s.switch_field();
        s.insert_char('b');
        let (zen, or) = s.take_values();
        assert_eq!(zen, "a");
        assert_eq!(or, "b");
        assert!(!s.visible);
    }

    #[test]
    fn mask_key_hides_all_but_last_four() {
        assert_eq!(mask_key(""), "paste your API key here...");
        assert_eq!(mask_key("abc"), "abc");
        assert_eq!(mask_key("abcdefgh"), "\u{2022}\u{2022}\u{2022}\u{2022}efgh");
    }
}
