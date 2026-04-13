// notifications.rs — Notification / banner system for the TUI.

use std::collections::VecDeque;
use std::time::Instant;

use crate::overlays::{CLAURST_ACCENT, CLAURST_MUTED, CLAURST_PANEL_BORDER, CLAURST_TEXT};

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

/// Render the topmost notification as a floating toast at the top-right of `area`.
///
/// Layout (3 rows):
///   row 0: ▐ [icon] [message truncated]          [Esc] ▌
///   row 1: ▐ [progress bar for timed notifs]            ▌
///   row 2: (bottom border row, blank)
pub fn render_notification_banner(frame: &mut Frame, queue: &NotificationQueue, area: Rect) {
    let notif = match queue.current() {
        Some(n) => n,
        None => return,
    };

    // Toast width: 48 cols max, right-aligned with a 2-col right margin.
    let toast_width = 52u16.min(area.width.saturating_sub(4));
    if area.height < 4 || toast_width < 20 {
        return;
    }
    let toast_height = 3u16;
    let toast_area = Rect {
        x: area.x + area.width.saturating_sub(toast_width + 2),
        y: area.y + 1,
        width: toast_width,
        height: toast_height,
    };

    let color = notif.kind.color();
    let icon = notif.kind.icon();
    let bg = Color::Rgb(18, 18, 22); // slightly elevated from terminal bg

    // Clear the area so the toast has a distinct background.
    frame.render_widget(Clear, toast_area);

    // ── Row 0: icon + message + optional "Esc" hint ──
    let inner_w = toast_width.saturating_sub(4) as usize; // 2 side bars + 1 pad each side
    let esc_hint = "  esc";
    let esc_len = if notif.dismissible { esc_hint.len() } else { 0 };
    let msg_budget = inner_w.saturating_sub(3 + esc_len); // 3 = " X " icon+spaces
    let message = if notif.message.chars().count() > msg_budget {
        format!(
            "{}…",
            notif
                .message
                .chars()
                .take(msg_budget.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        notif.message.clone()
    };

    let mut row0_spans = vec![
        Span::styled(
            format!(" {} ", icon),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(message, Style::default().fg(CLAURST_TEXT)),
    ];
    if notif.dismissible {
        row0_spans.push(Span::styled(esc_hint, Style::default().fg(CLAURST_MUTED)));
    }

    // ── Row 1: thin progress bar for timed notifications ──
    let progress_line = if let Some(exp) = notif.expires_at {
        let now = Instant::now();
        let remaining = if exp > now {
            (exp - now).as_millis()
        } else {
            0
        };
        // We don't store total duration, so derive from a fixed 5s assumption.
        // Clamp to [0,1] so the bar can't overflow.
        let frac = (remaining as f64 / 5_000.0).min(1.0);
        let bar_w = (inner_w as f64 * frac) as usize;
        let bar_w = bar_w.min(inner_w);
        let filled: String = "─".repeat(bar_w);
        let empty: String = " ".repeat(inner_w.saturating_sub(bar_w));
        Line::from(vec![
            Span::styled(format!(" {}", filled), Style::default().fg(color)),
            Span::styled(empty, Style::default().fg(CLAURST_MUTED)),
            Span::raw(" "),
        ])
    } else {
        Line::from(Span::styled(
            format!(" {}", "─".repeat(inner_w)),
            Style::default().fg(CLAURST_PANEL_BORDER),
        ))
    };

    // Render rows
    let buf = frame.buffer_mut();

    // Helper: paint a full row with bg color
    let paint_row = |buf: &mut ratatui::buffer::Buffer, row: u16| {
        for col in 0..toast_width {
            if let Some(cell) = buf.cell_mut((toast_area.x + col, toast_area.y + row)) {
                cell.set_bg(bg);
            }
        }
    };
    paint_row(buf, 0);
    paint_row(buf, 1);
    paint_row(buf, 2);

    // Left accent bar (all 3 rows)
    for row in 0..toast_height {
        if let Some(cell) = buf.cell_mut((toast_area.x, toast_area.y + row)) {
            cell.set_bg(bg);
            cell.set_fg(color);
            cell.set_char('▌');
        }
    }
    // Right border bar (all 3 rows)
    for row in 0..toast_height {
        if let Some(cell) = buf.cell_mut((toast_area.x + toast_width - 1, toast_area.y + row)) {
            cell.set_bg(bg);
            cell.set_fg(CLAURST_PANEL_BORDER);
            cell.set_char('▐');
        }
    }

    // Row 0: message
    let msg_rect = Rect {
        x: toast_area.x + 1,
        y: toast_area.y,
        width: toast_width.saturating_sub(2),
        height: 1,
    };
    let para0 = Paragraph::new(Line::from(row0_spans)).style(Style::default().bg(bg));
    frame.render_widget(para0, msg_rect);

    // Row 1: progress / divider
    let prog_rect = Rect {
        x: toast_area.x + 1,
        y: toast_area.y + 1,
        width: toast_width.saturating_sub(2),
        height: 1,
    };
    let para1 = Paragraph::new(progress_line).style(Style::default().bg(bg));
    frame.render_widget(para1, prog_rect);

    // Row 2: blank bottom padding
    let pad_rect = Rect {
        x: toast_area.x + 1,
        y: toast_area.y + 2,
        width: toast_width.saturating_sub(2),
        height: 1,
    };
    frame.render_widget(Paragraph::new("").style(Style::default().bg(bg)), pad_rect);
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
