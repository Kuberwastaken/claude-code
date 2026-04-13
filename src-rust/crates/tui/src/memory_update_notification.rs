// memory_update_notification.rs — MemoryUpdateNotification surface.
//
// Mirrors src/components/memory/MemoryUpdateNotification.tsx.
// Shown briefly in the message area when Claurst updates a memory file
// (e.g. ~/.claurst/AGENTS.md or a project-local AGENTS.md).
//
// Displays: "Memory updated in {relative_path} · /memory to edit"
//
// The surface is a single-row dismissable banner. The caller is responsible
// for showing it at the right time (e.g. after a memory write tool result).

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget};

use crate::overlays::{CLAURST_ACCENT, CLAURST_MUTED, CLAURST_PANEL_BG, CLAURST_TEXT};

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Compute the shortest display path for a memory file: prefer `~/…` or `./…`
/// over the absolute path, mirroring `getRelativeMemoryPath` in TS.
pub fn get_relative_memory_path(path: &str) -> String {
    // Try home-relative (~/)
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();

    let home_rel = if !home.is_empty() && path.starts_with(&home) {
        let rest = &path[home.len()..];
        let rest = rest.trim_start_matches(['/', '\\']);
        if rest.is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.replace('\\', "/"))
        }
    } else {
        String::new()
    };

    // Try cwd-relative (./)
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_default();

    let cwd_rel = if !cwd.is_empty() && path.starts_with(&cwd) {
        let rest = &path[cwd.len()..];
        let rest = rest.trim_start_matches(['/', '\\']);
        if rest.is_empty() {
            "./".to_string()
        } else {
            format!("./{}", rest.replace('\\', "/"))
        }
    } else {
        String::new()
    };

    // Return shortest, fall back to normalized absolute path
    match (home_rel.is_empty(), cwd_rel.is_empty()) {
        (false, false) => {
            if home_rel.len() <= cwd_rel.len() {
                home_rel
            } else {
                cwd_rel
            }
        }
        (false, true) => home_rel,
        (true, false) => cwd_rel,
        (true, true) => path.replace('\\', "/"),
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Memory update notification banner state.
#[derive(Debug, Clone, Default)]
pub struct MemoryUpdateNotificationState {
    /// Whether the notification is visible.
    pub visible: bool,
    /// Absolute path to the memory file that was updated.
    pub memory_path: String,
    /// Whether the user has dismissed this notification.
    dismissed: bool,
    expires_at: Option<Instant>,
}

impl MemoryUpdateNotificationState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Show the notification for the given memory file path.
    /// After `dismiss()` is called, re-showing is allowed (unlike the upsell
    /// banners which are session-persistent dismissals).
    pub fn show(&mut self, path: &str) {
        self.memory_path = path.to_string();
        self.visible = true;
        self.dismissed = false;
        self.expires_at = Some(Instant::now() + Duration::from_secs(6));
    }

    /// Dismiss the notification.
    pub fn dismiss(&mut self) {
        self.visible = false;
        self.dismissed = true;
        self.expires_at = None;
    }

    /// Height the notification occupies (0 if not visible).
    pub fn height(&self) -> u16 {
        if self.visible {
            1
        } else {
            0
        }
    }

    pub fn tick(&mut self) {
        if self.visible
            && self
                .expires_at
                .is_some_and(|expires_at| Instant::now() >= expires_at)
        {
            self.dismiss();
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the memory update notification into `area`.
pub fn render_memory_update_notification(
    state: &MemoryUpdateNotificationState,
    area: Rect,
    buf: &mut Buffer,
) {
    if !state.visible || area.height == 0 {
        return;
    }

    let notif_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };

    Clear.render(notif_area, buf);

    let display_path = get_relative_memory_path(&state.memory_path);

    let line = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled("\u{1f9e0} ", Style::default().fg(CLAURST_ACCENT)),
        Span::styled("Memory updated in ", Style::default().fg(CLAURST_TEXT)),
        Span::styled(
            display_path,
            Style::default()
                .fg(CLAURST_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" \u{00b7} ", Style::default().fg(CLAURST_MUTED)),
        Span::styled(
            "/memory",
            Style::default()
                .fg(CLAURST_ACCENT)
                .add_modifier(Modifier::UNDERLINED),
        ),
        Span::styled(" to edit", Style::default().fg(CLAURST_MUTED)),
        Span::styled("  Esc dismiss", Style::default().fg(CLAURST_MUTED)),
    ]);

    Paragraph::new(line)
        .style(Style::default().bg(CLAURST_PANEL_BG).fg(CLAURST_TEXT))
        .render(notif_area, buf);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn memory_notif_show_and_dismiss() {
        let mut state = MemoryUpdateNotificationState::new();
        assert!(!state.visible);
        state.show("/home/user/.claurst/AGENTS.md");
        assert!(state.visible);
        assert_eq!(state.memory_path, "/home/user/.claurst/AGENTS.md");
        state.dismiss();
        assert!(!state.visible);
    }

    #[test]
    fn memory_notif_can_reshown_after_dismiss() {
        let mut state = MemoryUpdateNotificationState::new();
        state.show("/tmp/AGENTS.md");
        state.dismiss();
        assert!(!state.visible);
        state.show("/tmp/other/AGENTS.md");
        assert!(state.visible);
        assert_eq!(state.memory_path, "/tmp/other/AGENTS.md");
    }

    #[test]
    fn memory_notif_height() {
        let mut state = MemoryUpdateNotificationState::new();
        assert_eq!(state.height(), 0);
        state.show("/tmp/AGENTS.md");
        assert_eq!(state.height(), 1);
    }

    #[test]
    fn memory_notif_expires_after_tick() {
        let mut state = MemoryUpdateNotificationState::new();
        state.show("/tmp/AGENTS.md");
        state.expires_at = Some(Instant::now() - Duration::from_secs(1));
        state.tick();
        assert!(!state.visible);
    }

    #[test]
    fn get_relative_memory_path_absolute_fallback() {
        // When path doesn't match HOME or cwd, return normalized absolute path.
        let result = get_relative_memory_path("/completely/unrelated/path.md");
        assert!(result.contains("completely") || result.contains("unrelated"));
    }

    #[test]
    fn get_relative_memory_path_home_relative() {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/home/testuser".to_string());
        let path = format!("{}/AGENTS.md", home);
        let result = get_relative_memory_path(&path);
        assert!(result.starts_with("~/"), "expected ~/…, got: {result}");
        assert!(result.contains("AGENTS.md"));
    }

    #[test]
    fn memory_notif_render_smoke() {
        let mut state = MemoryUpdateNotificationState::new();
        state.show("/home/user/.claurst/AGENTS.md");
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 4,
        };
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_memory_update_notification(&state, area, &mut buf);
        let rendered = buf
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(rendered.contains("Memory updated in"));
    }

    #[test]
    fn memory_notif_not_rendered_when_invisible() {
        let state = MemoryUpdateNotificationState::new();
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 4,
        };
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_memory_update_notification(&state, area, &mut buf);
        let rendered = buf
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(!rendered.contains("Memory updated"));
    }
}
