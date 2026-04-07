//! Task progress overlay — displays task status with inline toggle capability.
//!
//! Shows:
//! - Title with counts: "Tasks (N pending, M in_progress, K completed)"
//! - Per-task line with status badge: `[⏳] Task-001: Write documentation`
//! - Scrollable list if >20 tasks
//!
//! Keyboard:
//! - Arrow up/down: navigate task list
//! - Enter: toggle status (pending→in_progress→completed→pending)
//! - Escape/Q: close overlay

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::sync::Arc;

use crate::overlays::centered_rect;
use chrono;
use claurst_tools::TaskStatus;

// ---------------------------------------------------------------------------
// Helper functions for TaskStatus (defined in cc_tools)
// ---------------------------------------------------------------------------

/// Get the status badge symbol shown in the list.
fn status_badge(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "⏳",
        TaskStatus::InProgress => "🟡",
        TaskStatus::Completed => "✅",
        TaskStatus::Deleted => "🗑",
        TaskStatus::Running => "▶",
        TaskStatus::Failed => "❌",
    }
}

/// Cycle to next status in the UI flow: pending→in_progress→completed→pending.
/// Running, Failed, and Deleted are treated specially (skip to Pending when cycling).
pub fn next_status(status: &TaskStatus) -> TaskStatus {
    match status {
        TaskStatus::Pending => TaskStatus::InProgress,
        TaskStatus::InProgress => TaskStatus::Completed,
        TaskStatus::Completed | TaskStatus::Running | TaskStatus::Failed | TaskStatus::Deleted => {
            TaskStatus::Pending
        }
    }
}

// ---------------------------------------------------------------------------
// Task display model (minimal version for the overlay)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TaskDisplay {
    pub id: String,
    pub subject: String,
    pub status: TaskStatus,
}

// ---------------------------------------------------------------------------
// Tasks overlay state
// ---------------------------------------------------------------------------

/// State for the tasks progress overlay (Ctrl+T).
pub struct TasksOverlay {
    pub visible: bool,
    pub tasks: Vec<TaskDisplay>,
    pub selected_idx: usize,
    pub scroll_offset: u16,
    /// Timestamp of last refresh to debounce reloads.
    pub last_refresh: Option<std::time::Instant>,
}

impl TasksOverlay {
    /// Create a new overlay in hidden state.
    pub fn new() -> Self {
        Self {
            visible: false,
            tasks: Vec::new(),
            selected_idx: 0,
            scroll_offset: 0,
            last_refresh: None,
        }
    }

    /// Toggle visibility (open/close).
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        if !self.visible {
            self.scroll_offset = 0;
            self.selected_idx = 0;
        }
    }

    /// Close the overlay.
    pub fn close(&mut self) {
        self.visible = false;
        self.scroll_offset = 0;
        self.selected_idx = 0;
    }

    /// Update the task list from the global task store.
    /// Converts tasks from the cc_tools task store to display models.
    ///
    /// This should be called periodically (e.g., every frame) to keep the
    /// overlay in sync with the global task state.
    pub fn refresh_tasks(
        &mut self,
        task_store: &Arc<dashmap::DashMap<String, claurst_tools::Task>>,
    ) {
        self.tasks.clear();

        for entry in task_store.iter() {
            let task = entry.value();
            self.tasks.push(TaskDisplay {
                id: task.id.clone(),
                subject: task.subject.clone(),
                status: task.status.clone(),
            });
        }

        // Sort: pending first, then in_progress, then completed
        self.tasks.sort_by(|a, b| {
            let priority = |status: &TaskStatus| -> u8 {
                match status {
                    TaskStatus::Pending => 0,
                    TaskStatus::InProgress => 1,
                    TaskStatus::Completed => 2,
                    _ => 3, // Deleted, Running, Failed
                }
            };
            let a_pri = priority(&a.status);
            let b_pri = priority(&b.status);
            a_pri.cmp(&b_pri).then_with(|| a.subject.cmp(&b.subject))
        });

        // Clamp selection to valid range
        if self.tasks.is_empty() {
            self.selected_idx = 0;
        } else if self.selected_idx >= self.tasks.len() {
            self.selected_idx = self.tasks.len() - 1;
        }

        self.last_refresh = Some(std::time::Instant::now());
    }

    /// Navigate to previous task in the list.
    pub fn select_prev(&mut self) {
        if !self.tasks.is_empty() {
            if self.selected_idx == 0 {
                self.selected_idx = self.tasks.len() - 1;
            } else {
                self.selected_idx -= 1;
            }
            self.ensure_visible();
        }
    }

    /// Navigate to next task in the list.
    pub fn select_next(&mut self) {
        if !self.tasks.is_empty() {
            self.selected_idx = (self.selected_idx + 1) % self.tasks.len();
            self.ensure_visible();
        }
    }

    /// Ensure the selected item is within the scrolling viewport.
    fn ensure_visible(&mut self) {
        const VIEWPORT_HEIGHT: usize = 20;
        if self.selected_idx < self.scroll_offset as usize {
            self.scroll_offset = self.selected_idx as u16;
        } else if self.selected_idx >= self.scroll_offset as usize + VIEWPORT_HEIGHT {
            self.scroll_offset = (self.selected_idx - VIEWPORT_HEIGHT + 1) as u16;
        }
    }

    /// Get the status of the currently selected task, if any.
    pub fn selected_status(&self) -> Option<TaskStatus> {
        self.tasks.get(self.selected_idx).map(|t| t.status.clone())
    }

    /// Cycle the selected task's status to the next state.
    /// Returns the new status if successful.
    pub fn cycle_selected_status(&mut self) -> Option<TaskStatus> {
        if let Some(task) = self.tasks.get_mut(self.selected_idx) {
            task.status = next_status(&task.status);
            Some(task.status.clone())
        } else {
            None
        }
    }

    /// Cycle the selected task's status and persist it to the global task store.
    /// Returns the task ID and new status if successful.
    pub fn cycle_and_persist_status(&mut self) -> Option<(String, TaskStatus)> {
        if let Some(task) = self.tasks.get(self.selected_idx) {
            let task_id = task.id.clone();
            let new_status = next_status(&task.status);

            // Update the global task store
            if let Some(mut global_task) = claurst_tools::TASK_STORE.get_mut(&task_id) {
                global_task.status = new_status.clone();
                global_task.updated_at = chrono::Utc::now();
            }

            // Update local display
            if let Some(local_task) = self.tasks.get_mut(self.selected_idx) {
                local_task.status = new_status.clone();
            }

            Some((task_id, new_status))
        } else {
            None
        }
    }

    /// Get statistics for the title bar.
    fn stats(&self) -> (usize, usize, usize) {
        let pending = self
            .tasks
            .iter()
            .filter(|t| matches!(t.status, TaskStatus::Pending))
            .count();
        let in_progress = self
            .tasks
            .iter()
            .filter(|t| matches!(t.status, TaskStatus::InProgress))
            .count();
        let completed = self
            .tasks
            .iter()
            .filter(|t| matches!(t.status, TaskStatus::Completed))
            .count();
        (pending, in_progress, completed)
    }
}

impl Default for TasksOverlay {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the tasks overlay into the frame.
pub fn render_tasks_overlay(frame: &mut Frame, overlay: &TasksOverlay, area: Rect) {
    if !overlay.visible {
        return;
    }

    let dialog_width = 80u16.min(area.width.saturating_sub(2));
    let dialog_height = 28u16.min(area.height.saturating_sub(2));
    let dialog_area = centered_rect(dialog_width, dialog_height, area);

    frame.render_widget(Clear, dialog_area);

    // Get statistics for title
    let (pending, in_progress, completed) = overlay.stats();
    let title_text = format!(
        "Tasks ({} pending, {} in_progress, {} completed)",
        pending, in_progress, completed
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", title_text))
        .border_style(Style::default().fg(Color::Cyan));

    frame.render_widget(block, dialog_area);

    let inner = Rect {
        x: dialog_area.x + 1,
        y: dialog_area.y + 1,
        width: dialog_area.width.saturating_sub(2),
        height: dialog_area.height.saturating_sub(3), // Leave room for hint
    };

    // Build task lines
    let mut lines: Vec<Line<'static>> = Vec::new();

    if overlay.tasks.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no tasks)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let viewport_height = inner.height as usize;
        let end_idx = (overlay.scroll_offset as usize + viewport_height).min(overlay.tasks.len());

        for (i, task) in overlay
            .tasks
            .iter()
            .enumerate()
            .skip(overlay.scroll_offset as usize)
            .take(viewport_height)
        {
            let is_selected = i == overlay.selected_idx;
            let bg = if is_selected {
                Color::DarkGray
            } else {
                Color::Reset
            };

            let badge = Span::styled(
                format!("[{}]", status_badge(&task.status)),
                Style::default().fg(Color::Yellow),
            );

            let id = Span::raw(format!(" {} ", task.id));
            let sep = Span::raw(": ");
            let subject = Span::raw(task.subject.clone());

            let line = if is_selected {
                Line::from(vec![badge, id, sep, subject]).style(
                    Style::default()
                        .bg(bg)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                )
            } else {
                Line::from(vec![badge, id, sep, subject])
            };

            lines.push(line);
        }

        // Show scroll indicator if needed
        if end_idx < overlay.tasks.len() {
            lines.push(Line::from(Span::styled(
                "  ... more tasks below",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);

    // Hint footer
    let hint_area = Rect {
        x: dialog_area.x + 1,
        y: dialog_area.y + dialog_area.height - 2,
        width: dialog_area.width.saturating_sub(2),
        height: 1,
    };

    let hint = Line::from(vec![
        Span::styled("↑↓ ", Style::default().fg(Color::DarkGray)),
        Span::raw("Navigate  "),
        Span::styled("Enter ", Style::default().fg(Color::DarkGray)),
        Span::raw("Toggle  "),
        Span::styled("Esc/Q ", Style::default().fg(Color::DarkGray)),
        Span::raw("Close"),
    ]);

    frame.render_widget(Paragraph::new(hint), hint_area);
}
