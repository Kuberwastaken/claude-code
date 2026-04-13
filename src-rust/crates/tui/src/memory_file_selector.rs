// memory_file_selector.rs — Memory file selector overlay mirroring TS MemoryFileSelector.tsx

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::overlays::{
    centered_rect, render_dark_overlay_buf, render_dialog_bg_buf, CLAURST_ACCENT, CLAURST_MUTED,
    CLAURST_PANEL_BG, CLAURST_TEXT,
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryFileType {
    User,
    Project,
    Local,
}

pub struct MemoryFile {
    pub path: String,
    pub display_path: String,
    pub file_type: MemoryFileType,
    pub exists: bool,
}

pub struct MemoryFileSelectorState {
    pub visible: bool,
    pub files: Vec<MemoryFile>,
    pub selected: usize,
    pub project_root: std::path::PathBuf,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl MemoryFileSelectorState {
    pub fn new() -> Self {
        Self {
            visible: false,
            files: Vec::new(),
            selected: 0,
            project_root: std::path::PathBuf::new(),
        }
    }

    /// Open the selector for the given project root.
    ///
    /// Populates the file list with:
    /// - User:    `~/.claurst/AGENTS.md`
    /// - Project: `{project_root}/AGENTS.md`
    /// - Local:   `{project_root}/.claurst/AGENTS.md`
    ///
    /// Each entry is marked `exists = true/false` based on the filesystem.
    pub fn open(&mut self, project_root: &std::path::Path) {
        self.project_root = project_root.to_path_buf();
        self.selected = 0;
        self.files.clear();

        // User-level: ~/.claurst/AGENTS.md
        let user_path = claurst_core::config::Settings::config_dir().join("AGENTS.md");
        let user_display = {
            let home = dirs::home_dir().unwrap_or_default();
            let rel = user_path.strip_prefix(&home).unwrap_or(&user_path);
            format!("~/{}", rel.display())
        };
        self.files.push(MemoryFile {
            exists: user_path.exists(),
            path: user_path.to_string_lossy().into_owned(),
            display_path: user_display,
            file_type: MemoryFileType::User,
        });

        // Project-level: {project_root}/AGENTS.md
        let project_path = project_root.join("AGENTS.md");
        let project_display = project_path.display().to_string();
        self.files.push(MemoryFile {
            exists: project_path.exists(),
            path: project_path.to_string_lossy().into_owned(),
            display_path: project_display,
            file_type: MemoryFileType::Project,
        });

        // Local-level: {project_root}/.claurst/AGENTS.md
        let local_path = project_root.join(".claurst").join("AGENTS.md");
        let local_display = local_path.display().to_string();
        self.files.push(MemoryFile {
            exists: local_path.exists(),
            path: local_path.to_string_lossy().into_owned(),
            display_path: local_display,
            file_type: MemoryFileType::Local,
        });

        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn select_prev(&mut self) {
        let count = self.files.len();
        if count == 0 {
            return;
        }
        if self.selected == 0 {
            self.selected = count - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn select_next(&mut self) {
        let count = self.files.len();
        if count == 0 {
            return;
        }
        self.selected = (self.selected + 1) % count;
    }

    /// Return the path of the currently highlighted file, if any.
    pub fn selected_path(&self) -> Option<&str> {
        self.files.get(self.selected).map(|f| f.path.as_str())
    }
}

impl Default for MemoryFileSelectorState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the memory file selector as a centered floating dialog.
pub fn render_memory_file_selector(state: &MemoryFileSelectorState, area: Rect, buf: &mut Buffer) {
    if !state.visible {
        return;
    }

    // Height: 2 border + 1 blank + N files + 1 blank + 1 footer = N + 5
    let dialog_height = (state.files.len() as u16 + 6).max(8);
    let dialog_area = centered_rect(70, dialog_height, area);
    render_dark_overlay_buf(buf, area);
    render_dialog_bg_buf(buf, dialog_area);

    let inner = Rect {
        x: dialog_area.x + 2,
        y: dialog_area.y + 1,
        width: dialog_area.width.saturating_sub(4),
        height: dialog_area.height.saturating_sub(2),
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            " Memory",
            Style::default()
                .fg(CLAURST_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" — choose a file", Style::default().fg(CLAURST_MUTED)),
        Span::styled(
            format!(
                "{:>width$}",
                "Esc close",
                width = inner.width.saturating_sub(24) as usize
            ),
            Style::default().fg(CLAURST_MUTED),
        ),
    ]));
    lines.push(Line::from(""));

    for (i, file) in state.files.iter().enumerate() {
        let type_label = match file.file_type {
            MemoryFileType::User => "User    ",
            MemoryFileType::Project => "Project ",
            MemoryFileType::Local => "Local   ",
        };

        let new_tag = if !file.exists {
            Span::styled(" (new)", Style::default().fg(CLAURST_MUTED))
        } else {
            Span::raw("")
        };

        if i == state.selected {
            lines.push(Line::from(vec![Span::styled(
                pad_line(
                    &format!("  \u{203a} {type_label} {}", file.display_path),
                    inner.width,
                ),
                Style::default()
                    .fg(Color::Black)
                    .bg(CLAURST_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("    {type_label} {}", file.display_path),
                    Style::default().fg(CLAURST_TEXT),
                ),
                new_tag,
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "  \u{2191}\u{2193} navigate  Enter select  Esc close",
        Style::default().fg(CLAURST_MUTED),
    )]));

    let para = Paragraph::new(lines)
        .style(Style::default().bg(CLAURST_PANEL_BG).fg(CLAURST_TEXT))
        .alignment(Alignment::Left);

    use ratatui::widgets::Widget;
    para.render(inner, buf);
}

fn pad_line(text: &str, width: u16) -> String {
    let max_width = width as usize;
    let mut clipped: String = text.chars().take(max_width).collect();
    let visible = clipped.chars().count();
    if visible < max_width {
        clipped.push_str(&" ".repeat(max_width - visible));
    }
    clipped
}
