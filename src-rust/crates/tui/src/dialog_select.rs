// dialog_select.rs — Reusable fuzzy-search selection dialog widget.
//
// Used for the /connect provider picker and potentially for future
// selection dialogs (models, commands, sessions).

use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::cell::{Cell, RefCell};

use crate::overlays::{
    centered_rect, modal_search_line, render_dark_overlay, render_dialog_bg, CLAURST_PANEL_BG,
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single selectable item in the dialog.
#[derive(Debug, Clone)]
pub struct SelectItem {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub badge: Option<String>, // e.g., "FREE", "LOCAL", "NEW"
}

/// State for the DialogSelect overlay.
#[derive(Debug, Clone)]
pub struct DialogSelectState {
    pub visible: bool,
    pub title: String,
    pub items: Vec<SelectItem>,
    pub selected_index: usize,
    pub filter: String,
    filtered_indices: Vec<usize>,
    /// The area where this dialog was last rendered (for mouse hit testing).
    pub last_render_area: Cell<Rect>,
    /// Maps absolute screen row → filtered item index. Built during render.
    row_to_item: RefCell<Vec<(u16, usize)>>,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl DialogSelectState {
    pub fn new(title: impl Into<String>, items: Vec<SelectItem>) -> Self {
        let count = items.len();
        let filtered: Vec<usize> = (0..count).collect();
        Self {
            visible: false,
            title: title.into(),
            items,
            selected_index: 0,
            filter: String::new(),
            filtered_indices: filtered,
            last_render_area: Cell::new(Rect::default()),
            row_to_item: RefCell::new(Vec::new()),
        }
    }

    pub fn open(&mut self) {
        self.visible = true;
        self.selected_index = 0;
        self.filter.clear();
        self.refilter(); // ensure filtered_indices matches current items
        self.last_render_area.set(Rect::default());
        self.row_to_item.borrow_mut().clear();
        self.refilter();
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn move_up(&mut self) {
        let count = self.filtered_indices.len();
        if count == 0 {
            return;
        }
        if self.selected_index == 0 {
            self.selected_index = count - 1;
        } else {
            self.selected_index -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let count = self.filtered_indices.len();
        if count == 0 {
            return;
        }
        self.selected_index = (self.selected_index + 1) % count;
    }

    pub fn page_up(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(10);
    }

    pub fn page_down(&mut self) {
        self.selected_index =
            (self.selected_index + 10).min(self.filtered_indices.len().saturating_sub(1));
    }

    pub fn move_home(&mut self) {
        self.selected_index = 0;
    }

    pub fn move_end(&mut self) {
        self.selected_index = self.filtered_indices.len().saturating_sub(1);
    }

    /// Get the currently selected item (if any).
    pub fn selected(&self) -> Option<&SelectItem> {
        self.filtered_indices
            .get(self.selected_index)
            .and_then(|&idx| self.items.get(idx))
    }

    /// Type a character into the filter.
    pub fn filter_push(&mut self, c: char) {
        self.filter.push(c);
        self.refilter();
    }

    /// Backspace in the filter.
    pub fn filter_pop(&mut self) {
        self.filter.pop();
        self.refilter();
    }

    /// Check if a mouse position is inside the last rendered dialog area.
    pub fn contains(&self, column: u16, row: u16) -> bool {
        let area = self.last_render_area.get();
        area.width > 0
            && area.height > 0
            && column >= area.x
            && column < area.x.saturating_add(area.width)
            && row >= area.y
            && row < area.y.saturating_add(area.height)
    }

    /// Handle a mouse click at the given absolute screen row.
    /// Uses the row→item map built during the last render for pixel-accurate selection.
    /// Returns `true` if an item was selected, `false` otherwise.
    pub fn handle_mouse_click(&mut self, row: u16) -> bool {
        let map = self.row_to_item.borrow();
        for &(screen_row, item_idx) in map.iter() {
            if row == screen_row {
                self.selected_index = item_idx;
                return true;
            }
        }
        false
    }

    fn refilter(&mut self) {
        if self.filter.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            let query = self.filter.to_lowercase();
            self.filtered_indices = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    item.title.to_lowercase().contains(&query)
                        || item.description.to_lowercase().contains(&query)
                        || item.category.to_lowercase().contains(&query)
                })
                .map(|(i, _)| i)
                .collect();
        }
        // Clamp selection
        if self.selected_index >= self.filtered_indices.len() {
            self.selected_index = self.filtered_indices.len().saturating_sub(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the DialogSelect overlay — OpenCode-style: dark overlay, no border,
/// full-width highlight bar on selected item, minimal and polished.
pub fn render_dialog_select(frame: &mut Frame, state: &DialogSelectState, area: Rect) {
    if !state.visible {
        return;
    }

    let dim = Color::Rgb(90, 90, 90);
    let dialog_bg = CLAURST_PANEL_BG;
    let highlight_bg = Color::Rgb(233, 30, 99); // pink highlight bar
    let highlight_fg = Color::White;
    let category_fg = Color::Rgb(233, 30, 99); // pink category names

    // ── Darken the entire background ──
    render_dark_overlay(frame, area);

    // ── Dialog size: 65 wide, fit content ──
    let width = 65u16.min(area.width.saturating_sub(6));
    let max_height = (area.height as f32 * 0.75) as u16;
    // Count visible lines: header(2) + items + category gaps + footer(0)
    let item_lines: u16 = state.filtered_indices.len() as u16;
    let category_count = if state.filter.is_empty() {
        let mut sections = 0u16;
        let mut last_category: Option<&str> = None;
        for &idx in &state.filtered_indices {
            let category = state.items[idx].category.as_str();
            if last_category != Some(category) {
                sections += 1;
                last_category = Some(category);
            }
        }
        sections
    } else {
        0
    };
    let content_height = 3 + item_lines + category_count * 2; // search + blank + items + cat headers + gaps
    let height = content_height.min(max_height).max(8);
    let dialog_area = centered_rect(width, height, area);

    state.last_render_area.set(dialog_area);

    // ── Fill dialog background (no border) ──
    render_dialog_bg(frame, dialog_area);

    let inner = Rect {
        x: dialog_area.x + 1,
        y: dialog_area.y + 1,
        width: dialog_area.width.saturating_sub(2),
        height: dialog_area.height.saturating_sub(2),
    };

    let header_height = 3u16.min(inner.height);
    let header_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: header_height,
    };
    let body_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(header_height),
        width: inner.width,
        height: inner.height.saturating_sub(header_height),
    };

    // ── Fixed header ──
    let mut header_lines: Vec<Line<'static>> = Vec::new();

    // Title row: "Connect a provider" on left, "esc" on right
    let title_pad = inner.width.saturating_sub(state.title.len() as u16 + 4) as usize;
    header_lines.push(Line::from(vec![
        Span::styled(
            format!(" {}", state.title),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>width$}", "esc ", width = title_pad),
            Style::default().fg(dim),
        ),
    ]));

    // Search field
    header_lines.push(Line::from(""));
    header_lines.push(modal_search_line(
        &state.filter,
        "Search",
        dim,
        Color::White,
    ));

    frame.render_widget(Paragraph::new(header_lines).bg(dialog_bg), header_area);

    if body_area.height == 0 {
        return;
    }

    // ── Scrollable items ──
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut row_map: Vec<(u16, usize)> = Vec::new();
    let mut current_line: u16 = 0;
    let mut last_category = String::new();

    for (display_idx, &item_idx) in state.filtered_indices.iter().enumerate() {
        let item = &state.items[item_idx];
        let is_selected = display_idx == state.selected_index;

        // Category header (only when not filtering)
        if item.category != last_category && state.filter.is_empty() {
            lines.push(Line::from(""));
            current_line += 1;
            lines.push(Line::from(vec![Span::styled(
                format!(" {}", item.category),
                Style::default()
                    .fg(category_fg)
                    .add_modifier(Modifier::BOLD),
            )]));
            current_line += 1;
            last_category = item.category.clone();
        }

        // Item — full-width highlight bar when selected
        let (item_fg, item_bg) = if is_selected {
            (highlight_fg, highlight_bg)
        } else {
            (Color::White, dialog_bg)
        };

        let mut spans = vec![Span::styled(
            format!(" {}", item.title),
            Style::default().fg(item_fg).bg(item_bg),
        )];

        // Auth hint in parens, dimmed
        if !item.description.is_empty() {
            spans.push(Span::styled(
                format!(" {}", item.description),
                Style::default()
                    .fg(if is_selected {
                        Color::Rgb(200, 200, 200)
                    } else {
                        dim
                    })
                    .bg(item_bg),
            ));
        }

        let badge_text = item.badge.clone().unwrap_or_default();
        let text_len: usize = spans.iter().map(|s| s.content.len()).sum();
        let badge_len = if badge_text.is_empty() {
            0
        } else {
            badge_text.len() + 1
        };
        let pad = inner
            .width
            .saturating_sub(text_len as u16 + badge_len as u16) as usize;
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), Style::default().bg(item_bg)));
        }
        if !badge_text.is_empty() {
            spans.push(Span::styled(
                format!(" {}", badge_text),
                Style::default()
                    .fg(if is_selected { highlight_fg } else { dim })
                    .bg(item_bg)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        row_map.push((body_area.y + current_line, display_idx));
        lines.push(Line::from(spans));
        current_line += 1;
    }

    if state.filtered_indices.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            " No results found",
            Style::default().fg(dim),
        )]));
    }

    // ── Scroll ──
    let selected_item_line = {
        let map = state.row_to_item.borrow();
        map.iter()
            .find(|(_, idx)| *idx == state.selected_index)
            .map(|(abs_row, _)| abs_row.saturating_sub(body_area.y))
            .unwrap_or(0)
    };
    let total_lines = lines.len() as u16;
    let visible = body_area.height;
    let scroll_y = if total_lines <= visible {
        0u16
    } else if selected_item_line + 3 >= visible {
        (selected_item_line + 3).saturating_sub(visible)
    } else {
        0
    };

    *state.row_to_item.borrow_mut() = row_map
        .into_iter()
        .filter_map(|(row, idx)| {
            let screen_row = row.saturating_sub(scroll_y);
            if screen_row >= body_area.y
                && screen_row < body_area.y.saturating_add(body_area.height)
            {
                Some((screen_row, idx))
            } else {
                None
            }
        })
        .collect();

    let para = Paragraph::new(lines).bg(dialog_bg).scroll((scroll_y, 0));
    frame.render_widget(para, body_area);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn sample_items() -> Vec<SelectItem> {
        vec![
            SelectItem {
                id: "anthropic".into(),
                title: "Anthropic".into(),
                description: "Claude models".into(),
                category: "Recommended".into(),
                badge: None,
            },
            SelectItem {
                id: "openai".into(),
                title: "OpenAI".into(),
                description: "GPT models".into(),
                category: "Recommended".into(),
                badge: None,
            },
            SelectItem {
                id: "ollama".into(),
                title: "Ollama".into(),
                description: "Local models".into(),
                category: "Local".into(),
                badge: Some("LOCAL".into()),
            },
        ]
    }

    #[test]
    fn new_state_is_hidden() {
        let state = DialogSelectState::new("Test", sample_items());
        assert!(!state.visible);
        assert_eq!(state.selected_index, 0);
        assert!(state.filter.is_empty());
    }

    #[test]
    fn open_sets_visible() {
        let mut state = DialogSelectState::new("Test", sample_items());
        state.open();
        assert!(state.visible);
    }

    #[test]
    fn close_hides() {
        let mut state = DialogSelectState::new("Test", sample_items());
        state.open();
        state.close();
        assert!(!state.visible);
    }

    #[test]
    fn move_down_and_up_wrap() {
        let mut state = DialogSelectState::new("Test", sample_items());
        state.open();
        assert_eq!(state.selected_index, 0);
        state.move_down();
        assert_eq!(state.selected_index, 1);
        state.move_down();
        assert_eq!(state.selected_index, 2);
        // Wrap back to first after the last item.
        state.move_down();
        assert_eq!(state.selected_index, 0);
        state.move_up();
        assert_eq!(state.selected_index, 2);
        state.move_up();
        assert_eq!(state.selected_index, 1);
        state.move_up();
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn selected_returns_correct_item() {
        let mut state = DialogSelectState::new("Test", sample_items());
        state.open();
        assert_eq!(state.selected().unwrap().id, "anthropic");
        state.move_down();
        assert_eq!(state.selected().unwrap().id, "openai");
        state.move_down();
        assert_eq!(state.selected().unwrap().id, "ollama");
    }

    #[test]
    fn filter_reduces_results() {
        let mut state = DialogSelectState::new("Test", sample_items());
        state.open();
        state.filter_push('l');
        state.filter_push('o');
        state.filter_push('c');
        state.filter_push('a');
        state.filter_push('l');
        // Only "Ollama" matches "local"
        assert_eq!(state.selected().unwrap().id, "ollama");
    }

    #[test]
    fn filter_pop_restores() {
        let mut state = DialogSelectState::new("Test", sample_items());
        state.open();
        state.filter_push('z');
        state.filter_push('z');
        assert!(state.selected().is_none());
        state.filter_pop();
        state.filter_pop();
        assert_eq!(state.selected().unwrap().id, "anthropic");
    }

    #[test]
    fn page_up_and_down() {
        let mut state = DialogSelectState::new("Test", sample_items());
        state.open();
        state.page_down();
        assert_eq!(state.selected_index, 2); // clamped to last
        state.page_up();
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn home_and_end_jump_to_edges() {
        let mut state = DialogSelectState::new("Test", sample_items());
        state.open();
        state.move_end();
        assert_eq!(state.selected().unwrap().id, "ollama");
        state.move_home();
        assert_eq!(state.selected().unwrap().id, "anthropic");
    }

    #[test]
    fn render_does_not_panic() {
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut state = DialogSelectState::new("Test", sample_items());
        state.open();
        terminal
            .draw(|frame| {
                render_dialog_select(frame, &state, frame.area());
            })
            .unwrap();
    }

    #[test]
    fn render_noop_when_hidden() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let state = DialogSelectState::new("Test", sample_items());
        let before = terminal.backend().buffer().clone();
        terminal
            .draw(|frame| {
                render_dialog_select(frame, &state, frame.area());
            })
            .unwrap();
        assert_eq!(terminal.backend().buffer().content(), before.content());
    }
}
