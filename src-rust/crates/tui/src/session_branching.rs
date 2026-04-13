//! Session branching overlay — allows creating and switching between branches of a conversation.
//! Each branch is an independent copy of the conversation from a chosen message point forward.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Widget, Wrap};

use crate::overlays::centered_rect;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A branch of the conversation tree.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    /// Unique identifier for this branch.
    pub id: String,
    /// Human-readable name (user-provided).
    pub name: String,
    /// Message index at which the branch was created.
    pub branch_at_message: usize,
    /// Number of messages in this branch beyond the branch point.
    pub message_count: usize,
    /// When the branch was created (relative time string, e.g. "2 min ago").
    pub created_at: String,
    /// Whether this is the currently active branch.
    pub is_current: bool,
}

/// Interaction mode for the branch overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchBrowserMode {
    /// List view — navigate branches with arrow keys.
    Browse,
    /// User is typing a name for a new branch.
    CreateNew,
    /// Waiting for confirmation to delete the selected branch.
    ConfirmDelete,
}

/// State for the session branching UI overlay.
pub struct SessionBranchingState {
    /// Whether the overlay is visible.
    pub visible: bool,
    /// Currently selected branch index.
    pub selected_idx: usize,
    /// List of available branches.
    pub branches: Vec<BranchInfo>,
    /// Current interaction mode.
    pub mode: BranchBrowserMode,
    /// Input buffer used while creating a new branch.
    pub create_input: String,
    /// The message index at which a new branch should be created (Ctrl+B context).
    pub branch_at_message: usize,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl SessionBranchingState {
    /// Create a new, hidden branching state.
    pub fn new() -> Self {
        Self {
            visible: false,
            selected_idx: 0,
            branches: Vec::new(),
            mode: BranchBrowserMode::Browse,
            create_input: String::new(),
            branch_at_message: 0,
        }
    }

    /// Open the browser with the provided branch list.
    /// `branch_at` is the message index where a new branch would be created.
    pub fn open(&mut self, branches: Vec<BranchInfo>, branch_at: usize) {
        self.branches = branches;
        self.selected_idx = 0;
        self.mode = BranchBrowserMode::Browse;
        self.create_input.clear();
        self.branch_at_message = branch_at;
        self.visible = true;
    }

    /// Close the overlay.
    pub fn close(&mut self) {
        self.visible = false;
        self.mode = BranchBrowserMode::Browse;
        self.create_input.clear();
    }

    /// Move selection up one row, wrapping to the end.
    pub fn select_prev(&mut self) {
        let count = self.branches.len();
        if count == 0 {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = count - 1;
        } else {
            self.selected_idx -= 1;
        }
    }

    /// Move selection down one row, wrapping to the start.
    pub fn select_next(&mut self) {
        let count = self.branches.len();
        if count == 0 {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % count;
    }

    /// Get a reference to the currently selected branch, if any.
    pub fn selected_branch(&self) -> Option<&BranchInfo> {
        self.branches.get(self.selected_idx)
    }

    /// Get a mutable reference to the currently selected branch, if any.
    pub fn selected_branch_mut(&mut self) -> Option<&mut BranchInfo> {
        self.branches.get_mut(self.selected_idx)
    }

    /// Start creating a new branch from the current point.
    pub fn start_create_new(&mut self) {
        self.mode = BranchBrowserMode::CreateNew;
        self.create_input.clear();
    }

    /// Append a character to the creation input buffer.
    pub fn push_create_char(&mut self, c: char) {
        if self.mode == BranchBrowserMode::CreateNew {
            self.create_input.push(c);
        }
    }

    /// Remove the last character from the creation input buffer.
    pub fn pop_create_char(&mut self) {
        if self.mode == BranchBrowserMode::CreateNew {
            self.create_input.pop();
        }
    }

    /// Confirm creating a new branch. Returns `(name, at_message)` if valid.
    /// Resets to browse mode.
    pub fn confirm_create_new(&mut self) -> Option<(String, usize)> {
        if self.mode != BranchBrowserMode::CreateNew {
            return None;
        }
        let name = self.create_input.trim().to_string();
        if name.is_empty() {
            return None;
        }
        // Validate that the name doesn't conflict with existing branches.
        if self.branches.iter().any(|b| b.name == name) {
            return None; // Name conflict
        }
        self.mode = BranchBrowserMode::Browse;
        self.create_input.clear();
        Some((name, self.branch_at_message))
    }

    /// Start the delete confirmation flow.
    pub fn start_delete_confirm(&mut self) {
        if self.selected_branch().is_some() {
            self.mode = BranchBrowserMode::ConfirmDelete;
        }
    }

    /// Confirm deletion of the selected branch.
    /// Returns the branch ID if confirmed, None otherwise.
    pub fn confirm_delete(&mut self) -> Option<String> {
        if self.mode != BranchBrowserMode::ConfirmDelete {
            return None;
        }
        let branch_id = self.selected_branch()?.id.clone();
        self.branches.retain(|b| b.id != branch_id);
        self.mode = BranchBrowserMode::Browse;
        if self.selected_idx >= self.branches.len() && self.selected_idx > 0 {
            self.selected_idx -= 1;
        }
        Some(branch_id)
    }

    /// Cancel the current mode:
    /// - In `CreateNew` or `ConfirmDelete`: return to `Browse`.
    /// - In `Browse`: close the overlay.
    pub fn cancel(&mut self) {
        match self.mode {
            BranchBrowserMode::Browse => self.close(),
            BranchBrowserMode::CreateNew | BranchBrowserMode::ConfirmDelete => {
                self.mode = BranchBrowserMode::Browse;
                self.create_input.clear();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the session branching overlay.
pub fn render_session_branching(state: &SessionBranchingState, area: Rect, buf: &mut Buffer) {
    if !state.visible {
        return;
    }

    let popup_area = centered_rect(80, 70, area);
    let title = "Session Branches";

    let border_color = match state.mode {
        BranchBrowserMode::Browse => Color::Cyan,
        BranchBrowserMode::CreateNew => Color::Yellow,
        BranchBrowserMode::ConfirmDelete => Color::Red,
    };

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(popup_area);
    block.render(popup_area, buf);

    match state.mode {
        BranchBrowserMode::Browse => {
            render_branch_list(state, inner, buf);
        }
        BranchBrowserMode::CreateNew => {
            render_create_branch(state, inner, buf);
        }
        BranchBrowserMode::ConfirmDelete => {
            render_confirm_delete(state, inner, buf);
        }
    }
}

/// Render the branch list view.
fn render_branch_list(state: &SessionBranchingState, area: Rect, buf: &mut Buffer) {
    if area.height < 4 {
        return; // Too small to display
    }

    let help_height = 2;
    let list_area = Rect {
        height: area.height.saturating_sub(help_height),
        ..area
    };

    // Build list items
    let items: Vec<ListItem> = state
        .branches
        .iter()
        .enumerate()
        .map(|(idx, branch)| {
            let marker = if branch.is_current { " >" } else { "  " };
            let badge = if branch.is_current {
                Span::styled(" [ACTIVE]", Style::default().fg(Color::Green))
            } else {
                Span::raw("")
            };
            let line = Line::from(vec![
                Span::raw(format!(
                    "{}[{}] {} ({})",
                    marker,
                    idx + 1,
                    branch.name,
                    branch.created_at
                )),
                badge,
            ]);
            ListItem::new(line)
        })
        .collect();

    let selected_style = Style::default()
        .bg(Color::DarkGray)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let list = List::new(items)
        .highlight_style(selected_style)
        .highlight_symbol("→ ");

    let mut list_state = ratatui::widgets::ListState::default();
    list_state.select(Some(state.selected_idx));

    ratatui::widgets::StatefulWidget::render(list, list_area, buf, &mut list_state);

    // Render help text
    let help_area = Rect {
        y: area.y + list_area.height,
        height: help_height.min(area.height),
        ..area
    };

    let help_text = "↑↓: navigate | Enter: switch | N: new | D: delete | Esc: close";
    let help_para = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    help_para.render(help_area, buf);
}

/// Render the create new branch input view.
fn render_create_branch(state: &SessionBranchingState, area: Rect, buf: &mut Buffer) {
    let prompt = "Branch name: ";
    let lines = vec![
        Line::from(Span::raw("Create a new branch from message point")),
        Line::from(""),
        Line::from(vec![
            Span::raw(prompt),
            Span::styled(
                &state.create_input,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("_"),
        ]),
    ];

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .alignment(Alignment::Center);
    para.render(area, buf);
}

/// Render the delete confirmation dialog.
fn render_confirm_delete(state: &SessionBranchingState, area: Rect, buf: &mut Buffer) {
    let branch_name = state
        .selected_branch()
        .map(|b| b.name.as_str())
        .unwrap_or("(unknown)");

    let lines = vec![
        Line::from(format!("Delete branch '{}'?", branch_name)),
        Line::from(""),
        Line::from(Span::styled(
            "This will remove the branch but keep all other branches intact.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Y",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to confirm | "),
            Span::styled("Esc", Style::default().fg(Color::Cyan)),
            Span::raw(" to cancel"),
        ]),
    ];

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .alignment(Alignment::Center);
    para.render(area, buf);
}
