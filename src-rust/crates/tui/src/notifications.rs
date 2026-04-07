// notifications.rs — Notification / banner system for the TUI.

use std::collections::VecDeque;
use std::time::Instant;

use crate::overlays::{
    CLAURST_ACCENT, CLAURST_MUTED, CLAURST_PANEL_BG, CLAURST_PANEL_BORDER, CLAURST_TEXT,
};

/// Severity / visual style of a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationKind {
    Info,
    Warning,
    Error,
    Success,
}

/// A single notification entry.
#[derive(Debug, Clone)]
pub struct Notification {
    /// Unique identifier (used for dismissal).
    pub id: String,
    pub kind: NotificationKind,
    pub message: String,
    /// When `Some`, the notification auto-expires at this instant.
    pub expires_at: Option<Instant>,
    /// Whether the user can manually dismiss this notification.
    pub dismissible: bool,
}

/// A FIFO queue of active notifications.
#[derive(Debug, Default)]
pub struct NotificationQueue {
    pub notifications: VecDeque<Notification>,
    next_id: u64,
}

impl NotificationQueue {
    pub fn new() -> Self {
        Self {
            notifications: VecDeque::new(),
            next_id: 0,
        }
    }

    /// Push a new notification.
    ///
    /// * `duration_secs` — `None` for persistent, `Some(n)` for auto-expire after *n* seconds.
    pub fn push(&mut self, kind: NotificationKind, msg: String, duration_secs: Option<u64>) {
        let expires_at =
            duration_secs.map(|secs| Instant::now() + std::time::Duration::from_secs(secs));
        self.notifications
            .retain(|n| !(n.kind == kind && n.message == msg));
        let id = format!("notif-{}", self.next_id);
        self.next_id += 1;
        self.notifications.push_back(Notification {
            id,
            kind,
            message: msg,
            expires_at,
            dismissible: true,
        });
    }

    /// Dismiss the notification with the given `id`.
    pub fn dismiss(&mut self, id: &str) {
        self.notifications.retain(|n| n.id != id);
    }

    /// Remove all expired notifications.  Call this once per render frame.
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.notifications
            .retain(|n| n.expires_at.map_or(true, |exp| exp > now));
    }

    /// Return the currently visible (most recent) notification, if any.
    pub fn current(&self) -> Option<&Notification> {
        self.notifications.back()
    }

    /// Dismiss the currently visible notification.
    pub fn dismiss_current(&mut self) {
        if let Some(n) = self.notifications.back().cloned() {
            if n.dismissible {
                self.notifications.pop_back();
            }
        }
    }

    /// Return `true` if there are no active notifications.
    pub fn is_empty(&self) -> bool {
        self.notifications.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

impl NotificationKind {
    pub fn color(&self) -> Color {
        match self {
            NotificationKind::Info => CLAURST_ACCENT,
            NotificationKind::Warning => Color::Yellow,
            NotificationKind::Error => Color::Red,
            NotificationKind::Success => Color::Rgb(80, 200, 120),
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            NotificationKind::Info => "ℹ",
            NotificationKind::Warning => "⚠",
            NotificationKind::Error => "✗",
            NotificationKind::Success => "✓",
        }
    }
}

/// Render the topmost notification as a floating banner at the top of `area`.
pub fn render_notification_banner(frame: &mut Frame, queue: &NotificationQueue, area: Rect) {
    let notif = match queue.current() {
        Some(n) => n,
        None => return,
    };

    // One-line banner across the top of the provided area, inset slightly.
    let banner_width = area.width.saturating_sub(6);
    let banner_area = Rect {
        x: area.x + 3,
        y: area.y + 1,
        width: banner_width,
        height: 1,
    };

    // Only draw if there's room
    if area.height < 3 || banner_width < 16 {
        return;
    }

    let color = notif.kind.color();
    let icon = notif.kind.icon();

    let mut spans = vec![
        Span::styled(
            format!(" {} ", icon),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            notif.message.clone(),
            Style::default()
                .fg(CLAURST_TEXT)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if notif.dismissible {
        spans.push(Span::styled(
            "  Esc dismiss",
            Style::default().fg(CLAURST_MUTED),
        ));
    }

    let content_width = spans.iter().map(|span| span.content.len()).sum::<usize>();
    if content_width > banner_width.saturating_sub(2) as usize {
        spans = vec![
            Span::styled(
                format!(" {} ", icon),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "{}…",
                    notif
                        .message
                        .chars()
                        .take(banner_width.saturating_sub(6) as usize)
                        .collect::<String>()
                ),
                Style::default()
                    .fg(CLAURST_TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
    }
    let line = Line::from(spans);

    frame.render_widget(Clear, banner_area);
    let para = Paragraph::new(line).style(
        Style::default()
            .bg(CLAURST_PANEL_BG)
            .fg(CLAURST_TEXT)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(para, banner_area);

    if banner_area.width >= 3 {
        if let Some(cell) = frame.buffer_mut().cell_mut((banner_area.x, banner_area.y)) {
            cell.set_bg(CLAURST_PANEL_BG);
            cell.set_fg(CLAURST_PANEL_BORDER);
            cell.set_char('▌');
        }
        if let Some(cell) = frame.buffer_mut().cell_mut((
            banner_area.x.saturating_add(banner_area.width - 1),
            banner_area.y,
        )) {
            cell.set_bg(CLAURST_PANEL_BG);
            cell.set_fg(CLAURST_PANEL_BORDER);
            cell.set_char('▐');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_current() {
        let mut q = NotificationQueue::new();
        assert!(q.current().is_none());
        q.push(NotificationKind::Info, "hello".to_string(), None);
        assert_eq!(q.current().unwrap().message, "hello");
    }

    #[test]
    fn dismiss_by_id() {
        let mut q = NotificationQueue::new();
        q.push(NotificationKind::Warning, "warn".to_string(), None);
        let id = q.current().unwrap().id.clone();
        q.dismiss(&id);
        assert!(q.is_empty());
    }

    #[test]
    fn current_prefers_latest_notification() {
        let mut q = NotificationQueue::new();
        q.push(NotificationKind::Warning, "older".to_string(), None);
        q.push(NotificationKind::Info, "newer".to_string(), Some(3));
        assert_eq!(q.current().unwrap().message, "newer");
        q.dismiss_current();
        assert_eq!(q.current().unwrap().message, "older");
    }

    #[test]
    fn duplicate_notification_is_refreshed_not_duplicated() {
        let mut q = NotificationQueue::new();
        q.push(NotificationKind::Info, "same".to_string(), Some(3));
        q.push(NotificationKind::Info, "same".to_string(), Some(5));
        assert_eq!(q.notifications.len(), 1);
    }

    #[test]
    fn tick_removes_expired() {
        let mut q = NotificationQueue::new();
        // Push a notification that expired in the past
        q.notifications.push_back(super::Notification {
            id: "x".to_string(),
            kind: NotificationKind::Info,
            message: "gone".to_string(),
            expires_at: Some(Instant::now() - std::time::Duration::from_secs(1)),
            dismissible: true,
        });
        assert!(!q.is_empty());
        q.tick();
        assert!(q.is_empty());
    }

    #[test]
    fn persistent_notification_survives_tick() {
        let mut q = NotificationQueue::new();
        q.push(NotificationKind::Success, "persistent".to_string(), None);
        q.tick();
        assert!(!q.is_empty());
    }
}
