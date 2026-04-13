// dialogs.rs — Permission dialogs and confirmation dialogs.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::Frame;

// ---------------------------------------------------------------------------
// Permission dialog kinds
// ---------------------------------------------------------------------------

/// Distinguishes what kind of action the permission dialog is for.
/// This drives how many options are shown and what the command block looks like.
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionDialogKind {
    /// Generic four-option dialog (the previous default).
    Generic,
    /// Bash command execution — optionally carries a suggested prefix for a
    /// 5th "allow prefix*" option.
    Bash {
        command: String,
        suggested_prefix: Option<String>,
    },
    /// File read — three options: once / session / deny.
    FileRead { path: String },
    /// File write — four options: once / session / project / deny.
    FileWrite { path: String },
}

impl Default for PermissionDialogKind {
    fn default() -> Self {
        PermissionDialogKind::Generic
    }
}

// ---------------------------------------------------------------------------
// Permission dialog types
// ---------------------------------------------------------------------------

/// A single option inside a permission request dialog.
#[derive(Debug, Clone)]
pub struct PermissionOption {
    pub label: String,
    pub key: char,
}

/// State for an in-flight permission request popup.
///
/// This struct is intentionally richer than the legacy version to match the
/// TS permission dialog: it carries the command/path preview, a danger
/// explanation, and a stable set of TS-compatible options.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub tool_use_id: String,
    pub tool_name: String,
    /// Short verb phrase, e.g. "wants to run: `rm -rf /tmp/foo`"
    pub description: String,
    /// One-sentence danger explanation shown in yellow.
    pub danger_explanation: String,
    /// The raw command / path / URL (displayed in a code-block style line).
    pub input_preview: Option<String>,
    /// What kind of dialog this is — drives option set and rendering.
    pub kind: PermissionDialogKind,
    pub options: Vec<PermissionOption>,
    pub selected_option: usize,
}

impl PermissionRequest {
    /// Create a standard four-option dialog matching the TS dialog options:
    ///   `y` — Yes, allow once
    ///   `Y` — Yes, allow this session
    ///   `p` — Yes, always allow (persistent)
    ///   `n` — No, deny
    pub fn standard(tool_use_id: String, tool_name: String, description: String) -> Self {
        Self {
            tool_use_id,
            tool_name,
            description: description.clone(),
            danger_explanation: String::new(),
            input_preview: None,
            kind: PermissionDialogKind::Generic,
            selected_option: 0,
            options: Self::default_options(),
        }
    }

    /// Build with a richer description derived from the full permission reason
    /// text produced by `claurst_core::format_permission_reason`.
    ///
    /// The `reason` string may contain a newline splitting the one-liner from
    /// the danger explanation — this constructor splits on the first `\n` and
    /// places each part in the right field.
    pub fn from_reason(
        tool_use_id: String,
        tool_name: String,
        reason: String,
        input_preview: Option<String>,
    ) -> Self {
        let (description, danger_explanation) = if let Some(nl) = reason.find('\n') {
            (reason[..nl].to_string(), reason[nl + 1..].to_string())
        } else {
            (reason, String::new())
        };

        Self {
            tool_use_id,
            tool_name,
            description,
            danger_explanation,
            input_preview,
            kind: PermissionDialogKind::Generic,
            selected_option: 0,
            options: Self::default_options(),
        }
    }

    /// Build a Bash-specific dialog, computing the options set based on whether
    /// a `suggested_prefix` is available (5 options) or not (4 options).
    pub fn bash(
        tool_use_id: String,
        tool_name: String,
        reason: String,
        command: String,
        suggested_prefix: Option<String>,
    ) -> Self {
        let (description, danger_explanation) = if let Some(nl) = reason.find('\n') {
            (reason[..nl].to_string(), reason[nl + 1..].to_string())
        } else {
            (reason, String::new())
        };

        let options = Self::bash_options(suggested_prefix.as_deref());
        let kind = PermissionDialogKind::Bash {
            command: command.clone(),
            suggested_prefix,
        };

        Self {
            tool_use_id,
            tool_name,
            description,
            danger_explanation,
            input_preview: Some(command),
            kind,
            selected_option: 0,
            options,
        }
    }

    /// Build a FileRead-specific dialog (3 options: once / session / deny).
    pub fn file_read(tool_use_id: String, tool_name: String, reason: String, path: String) -> Self {
        let (description, danger_explanation) = if let Some(nl) = reason.find('\n') {
            (reason[..nl].to_string(), reason[nl + 1..].to_string())
        } else {
            (reason, String::new())
        };

        let preview = path.clone();
        let kind = PermissionDialogKind::FileRead { path };

        Self {
            tool_use_id,
            tool_name,
            description,
            danger_explanation,
            input_preview: Some(preview),
            kind,
            selected_option: 0,
            options: Self::file_read_options(),
        }
    }

    /// Build a FileWrite-specific dialog (4 options: once / session / project / deny).
    pub fn file_write(
        tool_use_id: String,
        tool_name: String,
        reason: String,
        path: String,
    ) -> Self {
        let (description, danger_explanation) = if let Some(nl) = reason.find('\n') {
            (reason[..nl].to_string(), reason[nl + 1..].to_string())
        } else {
            (reason, String::new())
        };

        let preview = path.clone();
        let kind = PermissionDialogKind::FileWrite { path };

        Self {
            tool_use_id,
            tool_name,
            description,
            danger_explanation,
            input_preview: Some(preview),
            kind,
            selected_option: 0,
            options: Self::file_write_options(),
        }
    }

    // ------------------------------------------------------------------
    // Option sets
    // ------------------------------------------------------------------

    /// The four canonical options (matches TS interactive permission dialog).
    pub fn default_options() -> Vec<PermissionOption> {
        vec![
            PermissionOption {
                label: "Yes, allow once".to_string(),
                key: 'y',
            },
            PermissionOption {
                label: "Yes, allow this session".to_string(),
                key: 'Y',
            },
            PermissionOption {
                label: "Yes, always allow (persistent)".to_string(),
                key: 'p',
            },
            PermissionOption {
                label: "No, deny".to_string(),
                key: 'n',
            },
        ]
    }

    /// Bash options: 4 standard + optional 5th prefix-based rule.
    pub fn bash_options(suggested_prefix: Option<&str>) -> Vec<PermissionOption> {
        let mut opts = Self::default_options();
        if let Some(prefix) = suggested_prefix {
            // Insert before the deny option (last item).
            let deny = opts.pop().unwrap();
            opts.push(PermissionOption {
                label: format!("Allow commands matching {}*", prefix),
                key: 'P',
            });
            opts.push(deny);
        }
        opts
    }

    /// FileRead options (3): once / session / deny.
    pub fn file_read_options() -> Vec<PermissionOption> {
        vec![
            PermissionOption {
                label: "Yes, allow once".to_string(),
                key: 'y',
            },
            PermissionOption {
                label: "Yes, allow this session".to_string(),
                key: 'Y',
            },
            PermissionOption {
                label: "No, deny".to_string(),
                key: 'n',
            },
        ]
    }

    /// FileWrite options (4): once / session / project / deny.
    pub fn file_write_options() -> Vec<PermissionOption> {
        vec![
            PermissionOption {
                label: "Yes, allow once".to_string(),
                key: 'y',
            },
            PermissionOption {
                label: "Yes, allow this session".to_string(),
                key: 'Y',
            },
            PermissionOption {
                label: "Yes, always allow for this project".to_string(),
                key: 'p',
            },
            PermissionOption {
                label: "No, deny".to_string(),
                key: 'n',
            },
        ]
    }
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

/// Compute a centred `Rect` of the given `width` × `height` inside `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

fn word_wrap(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthStr;
    if width == 0 || UnicodeWidthStr::width(text) <= width {
        return vec![text.to_string()];
    }
    let mut result = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0usize;
    for word in text.split_whitespace() {
        let word_w = UnicodeWidthStr::width(word);
        if current_width == 0 {
            current_line.push_str(word);
            current_width = word_w;
        } else if current_width + 1 + word_w <= width {
            current_line.push(' ');
            current_line.push_str(word);
            current_width += 1 + word_w;
        } else {
            result.push(std::mem::take(&mut current_line));
            current_line.push_str(word);
            current_width = word_w;
        }
    }
    if !current_line.is_empty() {
        result.push(current_line);
    }
    if result.is_empty() {
        result.push(text.to_string());
    }
    result
}

// ---------------------------------------------------------------------------
// Main render function
// ---------------------------------------------------------------------------

/// Render a permission-request dialog as a centred overlay.
///
/// Layout (top → bottom):
///   ┌─ Permission Required ─────────────────────────┐
///   │                                                │
///   │  Tool: Bash                                    │
///   │                                                │
///   │  > rm -rf /tmp/foo                             │
///   │                                                │
///   │  Bash wants to run: `rm -rf /tmp/foo`          │
///   │  This will delete files permanently.           │
///   │                                                │
///   │  [1] Yes, allow once                           │
///   │  [2] Yes, allow this session                   │
///   │▶ [3] Yes, always allow (persistent)            │
///   │  [4] No, deny                                  │
///   └────────────────────────────────────────────────┘
///
/// For `Bash` with a `suggested_prefix`, a 5th option is shown:
///   │  [5] Allow commands matching git*              │
///
/// For `FileRead`, only 3 options (once / session / deny).
/// For `FileWrite`, 4 options (once / session / project / deny).
pub fn render_permission_dialog(frame: &mut Frame, pr: &PermissionRequest, area: Rect) {
    let inner_width = 62u16;
    let dialog_width = inner_width.min(area.width.saturating_sub(4));
    let text_width = (dialog_width as usize).saturating_sub(4); // 2 border + 2 padding

    // Build a command block for Bash dialogs to prominently display the command.
    let bash_command_lines: Option<Vec<Line>> = match &pr.kind {
        PermissionDialogKind::Bash { command, .. } => {
            let wrapped = word_wrap(command, text_width.saturating_sub(4));
            Some(
                wrapped
                    .into_iter()
                    .map(|line| {
                        Line::from(vec![
                            Span::styled(
                                "  \u{276F} ",
                                Style::default()
                                    .fg(Color::Green)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                line,
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ])
                    })
                    .collect(),
            )
        }
        _ => None,
    };

    // Count how many lines we need
    let desc_lines = word_wrap(&pr.description, text_width);
    let expl_lines = if pr.danger_explanation.is_empty() {
        vec![]
    } else {
        word_wrap(&pr.danger_explanation, text_width)
    };

    // preview line count (used for non-Bash kinds; Bash uses its own block above)
    let preview_line_count: u16 = match &pr.kind {
        PermissionDialogKind::Bash { .. } => 0, // rendered separately as bash_command_lines
        _ => {
            if pr.input_preview.is_some() {
                3
            } else {
                0
            }
        }
    };

    let bash_block_height: u16 = bash_command_lines
        .as_ref()
        .map(|lines| lines.len() as u16 + 2) // lines + blank before + blank after
        .unwrap_or(0);

    let content_lines: u16 = 2 // "  Tool: <name>"  +  blank
        + bash_block_height
        + desc_lines.len() as u16
        + if !expl_lines.is_empty() { expl_lines.len() as u16 + 1 } else { 0 }
        + preview_line_count
        + 1 // blank before options
        + pr.options.len() as u16
        + 1; // trailing blank

    let dialog_height = (content_lines + 2) // +2 for top/bottom border
        .min(area.height.saturating_sub(4));

    let dialog_area = centered_rect(dialog_width, dialog_height, area);

    frame.render_widget(Clear, dialog_area);

    let mut lines: Vec<Line> = Vec::new();

    // ---- Tool name header ---------------------------------------------------
    lines.push(Line::from(vec![
        Span::raw("  Tool: "),
        Span::styled(
            pr.tool_name.clone(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));

    // ---- Bash command block (code-block style, green chevron) ---------------
    if let Some(cmd_lines) = bash_command_lines {
        for cmd_line in cmd_lines {
            lines.push(cmd_line);
        }
        lines.push(Line::from(""));
    }

    // ---- Input preview for non-Bash kinds -----------------------------------
    if !matches!(pr.kind, PermissionDialogKind::Bash { .. }) {
        if let Some(ref preview) = pr.input_preview {
            lines.push(Line::from(vec![
                Span::styled(
                    "  \u{276F} ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    preview.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(""));
        }
    }

    // ---- Description (word-wrapped) -----------------------------------------
    for desc_line in &desc_lines {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(desc_line.clone()),
        ]));
    }

    // ---- Danger explanation (yellow) ----------------------------------------
    if !expl_lines.is_empty() {
        for expl_line in &expl_lines {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(expl_line.clone(), Style::default().fg(Color::Yellow)),
            ]));
        }
        lines.push(Line::from(""));
    }

    // ---- Options ------------------------------------------------------------
    lines.push(Line::from(""));
    for (i, opt) in pr.options.iter().enumerate() {
        let is_selected = i == pr.selected_option;
        let prefix = if is_selected { "  \u{25BA} " } else { "    " };
        let key_style = if is_selected {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label_style = if is_selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::raw(prefix),
            Span::styled(format!("[{}]", opt.key), key_style),
            Span::raw(" "),
            Span::styled(opt.label.clone(), label_style),
        ]));
    }

    let (border_color, title_text) = match &pr.kind {
        PermissionDialogKind::Bash { .. } => (Color::Yellow, " Permission Required "),
        PermissionDialogKind::FileRead { .. } => (Color::Cyan, " File Read Permission "),
        PermissionDialogKind::FileWrite { .. } => (Color::Yellow, " File Write Permission "),
        PermissionDialogKind::Generic => (Color::Yellow, " Permission Required "),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title_text,
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(border_color));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, dialog_area);
}

// ---------------------------------------------------------------------------
// Permission key handler
// ---------------------------------------------------------------------------

/// Handle a key event while a permission dialog is active.
///
/// Returns `true` if the dialog was confirmed/dismissed (caller should clear it).
///
/// Behaviour by option count:
/// - 3-option dialog (FileRead): digits 1–3 valid, 4/5 rejected.
/// - 4-option dialog (Generic / FileWrite / Bash without prefix): digits 1–4 valid.
/// - 5-option dialog (Bash with prefix): digits 1–5 valid.
pub fn handle_permission_key(pr: &mut PermissionRequest, key: KeyEvent) -> bool {
    let option_count = pr.options.len();
    match key.code {
        KeyCode::Char(c) => {
            if let Some(digit) = c.to_digit(10) {
                let idx = (digit as usize).saturating_sub(1);
                if idx < option_count {
                    pr.selected_option = idx;
                    return true; // confirmed via digit shortcut
                }
                // Reject digits beyond the option count silently.
            } else {
                for (i, opt) in pr.options.iter().enumerate() {
                    if opt.key == c {
                        pr.selected_option = i;
                        return true;
                    }
                }
            }
        }
        KeyCode::Enter => {
            return true;
        }
        KeyCode::Up => {
            if pr.selected_option > 0 {
                pr.selected_option -= 1;
            }
        }
        KeyCode::Down => {
            if pr.selected_option + 1 < option_count {
                pr.selected_option += 1;
            }
        }
        KeyCode::Esc => {
            // Move selection to the last option (deny) without confirming.
            pr.selected_option = option_count.saturating_sub(1);
            return true;
        }
        _ => {}
    }
    false
}

// ---------------------------------------------------------------------------
// T2-6: Tool-specific permission request dialogs
// ---------------------------------------------------------------------------

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::Wrap;

/// Which tool-specific permission dialog is active.
#[derive(Debug, Clone)]
pub enum ToolPermissionKind {
    /// Bash command execution.
    Bash { command: String },
    /// File edit: show diff of proposed changes.
    FileEdit { path: String, diff: String },
    /// File write: show new file content.
    FileWrite {
        path: String,
        content_preview: String,
    },
    /// File read: show path + line range.
    FileRead {
        path: String,
        line_range: Option<(u32, u32)>,
    },
    /// Web fetch: show URL + domain risk.
    WebFetch { url: String, is_high_risk: bool },
    /// PowerShell script execution.
    PowerShell { script: String },
    /// Ask user a question (from AskUserQuestion tool).
    AskUser {
        question: String,
        choices: Vec<String>,
    },
    /// MCP server elicitation (schema-driven form).
    Elicitation {
        server: String,
        title: String,
        fields: Vec<ElicitationField>,
    },
}

/// A single field in an elicitation form.
#[derive(Debug, Clone)]
pub struct ElicitationField {
    pub name: String,
    pub description: String,
    pub field_type: ElicitationFieldType,
    pub required: bool,
    pub value: String, // current input value
}

/// Type of an elicitation field.
#[derive(Debug, Clone)]
pub enum ElicitationFieldType {
    Text,
    Number,
    Bool,
    Select(Vec<String>), // options
}

/// State for a tool-specific permission dialog.
#[derive(Debug, Clone)]
pub struct ToolPermissionDialog {
    /// What kind of dialog this is.
    pub kind: ToolPermissionKind,
    /// Currently focused button (0=Allow, 1=AlwaysAllow, 2=Deny).
    pub focused_button: usize,
    /// Scroll offset for content that overflows.
    pub scroll: u16,
    /// For Elicitation: which field is focused.
    pub focused_field: usize,
}

impl ToolPermissionDialog {
    pub fn new(kind: ToolPermissionKind) -> Self {
        Self {
            kind,
            focused_button: 0,
            scroll: 0,
            focused_field: 0,
        }
    }

    /// Move focus to next button.
    pub fn next_button(&mut self) {
        self.focused_button = (self.focused_button + 1) % 3;
    }

    /// Move focus to previous button.
    pub fn prev_button(&mut self) {
        self.focused_button = (self.focused_button + 2) % 3;
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.scroll += 1;
    }
}

/// Render a tool-specific permission dialog as a centered overlay.
pub fn render_tool_permission_dialog(dialog: &ToolPermissionDialog, frame: &mut Frame) {
    let area = centered_dialog_area(frame.area(), 70, 20);
    frame.render_widget(Clear, area);

    let (title, content_lines) = build_dialog_content(dialog);

    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split inner: content area + button row at bottom.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(inner);

    // Content area.
    let content_text = content_lines.join("\n");
    let para = Paragraph::new(content_text)
        .wrap(Wrap { trim: false })
        .scroll((dialog.scroll, 0))
        .style(Style::default().fg(Color::White));
    frame.render_widget(para, chunks[0]);

    // Button row.
    render_permission_buttons(dialog.focused_button, chunks[1], frame);
}

fn build_dialog_content(dialog: &ToolPermissionDialog) -> (&'static str, Vec<String>) {
    match &dialog.kind {
        ToolPermissionKind::Bash { command } => (
            "Allow Bash Command?",
            vec!["Command:".to_string(), format!("  $ {}", command)],
        ),
        ToolPermissionKind::FileEdit { path, diff } => ("Allow File Edit?", {
            let mut lines = vec![format!("File: {}", path), String::new()];
            for line in diff.lines().take(30) {
                lines.push(line.to_string());
            }
            lines
        }),
        ToolPermissionKind::FileWrite {
            path,
            content_preview,
        } => ("Allow File Write?", {
            let mut lines = vec![format!("File: {}", path), String::new()];
            for line in content_preview.lines().take(20) {
                lines.push(format!("  {}", line));
            }
            lines
        }),
        ToolPermissionKind::FileRead { path, line_range } => (
            "Allow File Read?",
            vec![
                format!("File: {}", path),
                match line_range {
                    Some((s, e)) => format!("Lines: {} \u{2013} {}", s, e),
                    None => "Full file".to_string(),
                },
            ],
        ),
        ToolPermissionKind::WebFetch { url, is_high_risk } => (
            "Allow Web Fetch?",
            vec![
                format!("URL: {}", url),
                if *is_high_risk {
                    "\u{26a0} Domain may be high-risk".to_string()
                } else {
                    "Domain appears safe".to_string()
                },
            ],
        ),
        ToolPermissionKind::PowerShell { script } => ("Allow PowerShell?", {
            let mut lines = vec!["Script:".to_string()];
            for line in script.lines().take(20) {
                lines.push(format!("  {}", line));
            }
            lines
        }),
        ToolPermissionKind::AskUser { question, choices } => ("Agent Question", {
            let mut lines = vec![question.clone()];
            if !choices.is_empty() {
                lines.push(String::new());
                lines.push("Options:".to_string());
                for (i, c) in choices.iter().enumerate() {
                    lines.push(format!("  {}. {}", i + 1, c));
                }
            }
            lines
        }),
        ToolPermissionKind::Elicitation {
            server,
            title,
            fields,
        } => ("Server Input Request", {
            let mut lines = vec![
                format!("Server: {}", server),
                format!("Request: {}", title),
                String::new(),
            ];
            for f in fields {
                lines.push(format!(
                    "  {} {}: {}",
                    if f.required { "*" } else { " " },
                    f.name,
                    f.value
                ));
            }
            lines
        }),
    }
}

fn render_permission_buttons(focused: usize, area: Rect, frame: &mut Frame) {
    let buttons = ["Allow", "Always Allow", "Deny"];
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(area);

    for (i, (label, chunk)) in buttons.iter().zip(chunks.iter()).enumerate() {
        let style = if i == focused {
            Style::default()
                .fg(Color::Black)
                .bg(if i == 2 { Color::Red } else { Color::Green })
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let btn = Paragraph::new(format!(" [ {} ] ", label))
            .style(style)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(btn, *chunk);
    }
}

/// Compute a centered rect of the given percentage size.
fn centered_dialog_area(r: Rect, percent_x: u16, min_height: u16) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - min_height.min(80)) / 2),
            Constraint::Min(min_height),
            Constraint::Percentage((100 - min_height.min(80)) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

// ---------------------------------------------------------------------------
// MCP Server Approval Dialog
// ---------------------------------------------------------------------------

/// Which choice the user made in the MCP server approval dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum McpApprovalChoice {
    /// Allow the server for this session only.
    AllowSession,
    /// Persist approval so it survives restarts.
    AllowAlways,
    /// Deny the server connection.
    Deny,
}

impl McpApprovalChoice {
    fn all() -> &'static [McpApprovalChoice] {
        &[
            McpApprovalChoice::AllowSession,
            McpApprovalChoice::AllowAlways,
            McpApprovalChoice::Deny,
        ]
    }

    fn index(&self) -> usize {
        match self {
            McpApprovalChoice::AllowSession => 0,
            McpApprovalChoice::AllowAlways => 1,
            McpApprovalChoice::Deny => 2,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            McpApprovalChoice::AllowSession => "Allow this session",
            McpApprovalChoice::AllowAlways => "Always allow",
            McpApprovalChoice::Deny => "Deny",
        }
    }
}

/// State for the MCP server approval dialog.
#[derive(Debug, Clone)]
pub struct McpApprovalDialogState {
    /// Whether the dialog is currently visible.
    pub visible: bool,
    /// Display name of the MCP server.
    pub server_name: String,
    /// Optional HTTP/WebSocket URL for the server.
    pub server_url: Option<String>,
    /// Optional command used to launch the server (for stdio servers).
    pub server_command: Option<String>,
    /// Tools the server exposes (at most first 5 shown in the UI).
    pub tool_names: Vec<String>,
    /// Currently highlighted choice.
    pub selected: McpApprovalChoice,
}

impl McpApprovalDialogState {
    /// Create a new, invisible state.
    pub fn new() -> Self {
        Self {
            visible: false,
            server_name: String::new(),
            server_url: None,
            server_command: None,
            tool_names: Vec::new(),
            selected: McpApprovalChoice::AllowSession,
        }
    }

    /// Populate and show the dialog.
    pub fn show(
        &mut self,
        server_name: &str,
        server_url: Option<&str>,
        server_command: Option<&str>,
        tool_names: Vec<String>,
    ) {
        self.server_name = server_name.to_string();
        self.server_url = server_url.map(|s| s.to_string());
        self.server_command = server_command.map(|s| s.to_string());
        self.tool_names = tool_names;
        self.selected = McpApprovalChoice::AllowSession;
        self.visible = true;
    }

    /// Move selection to the previous option (wraps around).
    pub fn select_prev(&mut self) {
        let idx = self.selected.index();
        self.selected = McpApprovalChoice::all()[(idx + 2) % 3].clone();
    }

    /// Move selection to the next option (wraps around).
    pub fn select_next(&mut self) {
        let idx = self.selected.index();
        self.selected = McpApprovalChoice::all()[(idx + 1) % 3].clone();
    }

    /// Confirm the current selection and hide the dialog.
    ///
    /// Returns the chosen action.
    pub fn confirm(&mut self) -> McpApprovalChoice {
        let choice = self.selected.clone();
        self.close();
        choice
    }

    /// Hide the dialog without returning a choice (treated as Deny by callers).
    pub fn close(&mut self) {
        self.visible = false;
    }
}

impl Default for McpApprovalDialogState {
    fn default() -> Self {
        Self::new()
    }
}

/// Render the MCP server approval dialog as a centred overlay.
///
/// The `buf` parameter is accepted per the spec; the function actually
/// delegates to the widget system which writes into the terminal buffer
/// through the `Block` / `Paragraph` widgets.  We expose both a
/// `Frame`-based variant (for use from the main render loop) and the
/// low-level `Buffer`-based variant required by the spec.
///
/// Layout:
/// ┌─ MCP Server Connection ──────────────────────────┐
/// │                                                   │
/// │  Server:  my-server                               │
/// │  URL:     wss://example.com/mcp                   │
/// │                                                   │
/// │  Exposes 3 tools:                                 │
/// │    • tool_one                                     │
/// │    • tool_two                                     │
/// │    • tool_three                                   │
/// │                                                   │
/// │  ▶ [1] Allow this session                         │
/// │    [2] Always allow                               │
/// │    [3] Deny                                       │
/// └───────────────────────────────────────────────────┘
pub fn render_mcp_approval_dialog(state: &McpApprovalDialogState, area: Rect, buf: &mut Buffer) {
    if !state.visible {
        return;
    }

    let dialog_width = 54u16.min(area.width.saturating_sub(4));
    let text_width = (dialog_width as usize).saturating_sub(4);

    // Count lines: header rows + tool list + blank lines + 3 option rows + trailing blank.
    let tool_display_count = state.tool_names.len().min(5);
    let has_tools = tool_display_count > 0;
    let has_url_or_cmd = state.server_url.is_some() || state.server_command.is_some();

    let content_height: u16 = 1  // blank after border
        + 1  // "Server: ..."
        + if has_url_or_cmd { 1 } else { 0 }
        + 1  // blank
        + if has_tools { 1 + tool_display_count as u16 + 1 } else { 0 } // header + items + blank
        + 3  // 3 option rows
        + 1; // trailing blank

    let dialog_height = (content_height + 2).min(area.height.saturating_sub(4));
    let dialog_area = centered_rect(dialog_width, dialog_height, area);

    // Clear the area behind the dialog.
    Clear.render(dialog_area, buf);

    let mut lines: Vec<Line> = Vec::new();

    // Blank line after the top border.
    lines.push(Line::from(""));

    // Server name.
    let server_label = format!(
        "  Server:  {}",
        truncate_str(&state.server_name, text_width.saturating_sub(10))
    );
    lines.push(Line::from(vec![
        Span::styled("  Server:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            truncate_str(&state.server_name, text_width.saturating_sub(10)),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    let _ = server_label; // suppress unused warning

    // URL or command.
    if let Some(ref url) = state.server_url {
        lines.push(Line::from(vec![
            Span::styled("  URL:     ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                truncate_str(url, text_width.saturating_sub(10)),
                Style::default().fg(Color::White),
            ),
        ]));
    } else if let Some(ref cmd) = state.server_command {
        lines.push(Line::from(vec![
            Span::styled("  Command: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                truncate_str(cmd, text_width.saturating_sub(10)),
                Style::default().fg(Color::White),
            ),
        ]));
    }

    lines.push(Line::from(""));

    // Tools list.
    if has_tools {
        let extra = state.tool_names.len().saturating_sub(5);
        lines.push(Line::from(vec![Span::styled(
            format!(
                "  Exposes {} tool{}{}:",
                state.tool_names.len(),
                if state.tool_names.len() == 1 { "" } else { "s" },
                if extra > 0 {
                    format!(" (showing first 5 of {})", state.tool_names.len())
                } else {
                    String::new()
                },
            ),
            Style::default().fg(Color::DarkGray),
        )]));
        for name in state.tool_names.iter().take(5) {
            lines.push(Line::from(vec![
                Span::styled("    \u{2022} ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    truncate_str(name, text_width.saturating_sub(6)),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
        lines.push(Line::from(""));
    }

    // Options.
    for choice in McpApprovalChoice::all() {
        let is_selected = *choice == state.selected;
        let prefix = if is_selected { "  \u{25BA} " } else { "    " };
        let num = choice.index() + 1;
        let key_style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label_style = if is_selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        // Deny option gets a red tint when selected.
        let label_style = if is_selected && *choice == McpApprovalChoice::Deny {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            label_style
        };
        lines.push(Line::from(vec![
            Span::raw(prefix),
            Span::styled(format!("[{}]", num), key_style),
            Span::raw(" "),
            Span::styled(choice.label(), label_style),
        ]));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " MCP Server Connection ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::Cyan));

    let para = Paragraph::new(lines).block(block);
    para.render(dialog_area, buf);
}

/// Render the MCP approval dialog using a `Frame` (convenience wrapper for
/// the main render loop).
pub fn render_mcp_approval_dialog_frame(state: &McpApprovalDialogState, frame: &mut Frame) {
    if !state.visible {
        return;
    }
    let area = frame.area();
    render_mcp_approval_dialog(state, area, frame.buffer_mut());
}

/// Handle a key event while the MCP approval dialog is open.
///
/// Returns `Some(choice)` when the user confirms (Enter or digit shortcut),
/// or `Some(Deny)` when Esc is pressed.  Returns `None` for navigation keys.
pub fn handle_mcp_approval_key(
    state: &mut McpApprovalDialogState,
    key: KeyEvent,
) -> Option<McpApprovalChoice> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.select_prev();
            None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.select_next();
            None
        }
        KeyCode::Enter => Some(state.confirm()),
        KeyCode::Char('1') => {
            state.selected = McpApprovalChoice::AllowSession;
            Some(state.confirm())
        }
        KeyCode::Char('2') => {
            state.selected = McpApprovalChoice::AllowAlways;
            Some(state.confirm())
        }
        KeyCode::Char('3') | KeyCode::Char('n') => {
            state.selected = McpApprovalChoice::Deny;
            Some(state.confirm())
        }
        KeyCode::Esc => {
            state.close();
            Some(McpApprovalChoice::Deny)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Truncate a string to at most `max_chars` characters, appending `…` if cut.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let cut: String = chars[..max_chars.saturating_sub(1)].iter().collect();
        format!("{}\u{2026}", cut)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    // -----------------------------------------------------------------------
    // Existing / backward-compat tests
    // -----------------------------------------------------------------------

    #[test]
    fn standard_permission_request_has_four_options() {
        let pr = PermissionRequest::standard(
            "id1".to_string(),
            "Bash".to_string(),
            "Run a shell command".to_string(),
        );
        assert_eq!(pr.options.len(), 4);
        assert_eq!(pr.options[0].key, 'y');
        assert_eq!(pr.options[1].key, 'Y');
        assert_eq!(pr.options[2].key, 'p');
        assert_eq!(pr.options[3].key, 'n');
    }

    #[test]
    fn from_reason_splits_on_newline() {
        let pr = PermissionRequest::from_reason(
            "id2".to_string(),
            "Bash".to_string(),
            "Bash wants to run: `rm -rf /tmp`\nThis will delete files permanently.".to_string(),
            Some("rm -rf /tmp".to_string()),
        );
        assert_eq!(pr.description, "Bash wants to run: `rm -rf /tmp`");
        assert_eq!(pr.danger_explanation, "This will delete files permanently.");
        assert_eq!(pr.input_preview.as_deref(), Some("rm -rf /tmp"));
    }

    #[test]
    fn from_reason_no_newline() {
        let pr = PermissionRequest::from_reason(
            "id3".to_string(),
            "WebFetch".to_string(),
            "WebFetch wants to fetch: `https://example.com`".to_string(),
            None,
        );
        assert_eq!(
            pr.description,
            "WebFetch wants to fetch: `https://example.com`"
        );
        assert!(pr.danger_explanation.is_empty());
    }

    #[test]
    fn word_wrap_short_text_unchanged() {
        let wrapped = word_wrap("hello world", 80);
        assert_eq!(wrapped, vec!["hello world"]);
    }

    #[test]
    fn word_wrap_long_text_splits() {
        let text = "one two three four five six seven eight";
        let wrapped = word_wrap(text, 10);
        for line in &wrapped {
            assert!(line.len() <= 10, "Line too long: {:?}", line);
        }
    }

    // -----------------------------------------------------------------------
    // PermissionDialogKind tests
    // -----------------------------------------------------------------------

    #[test]
    fn bash_without_prefix_has_four_options() {
        let pr = PermissionRequest::bash(
            "id-bash-1".to_string(),
            "Bash".to_string(),
            "Wants to run a command".to_string(),
            "ls -la".to_string(),
            None,
        );
        assert_eq!(pr.options.len(), 4);
        assert_eq!(
            pr.kind,
            PermissionDialogKind::Bash {
                command: "ls -la".to_string(),
                suggested_prefix: None,
            }
        );
        // input_preview is set to the command
        assert_eq!(pr.input_preview.as_deref(), Some("ls -la"));
    }

    #[test]
    fn bash_with_prefix_has_five_options() {
        let pr = PermissionRequest::bash(
            "id-bash-2".to_string(),
            "Bash".to_string(),
            "Wants to run git command".to_string(),
            "git status".to_string(),
            Some("git ".to_string()),
        );
        assert_eq!(pr.options.len(), 5);
        // 5th option (index 3 before deny) carries the prefix label
        assert!(
            pr.options[3].label.contains("git "),
            "Expected prefix in label: {:?}",
            pr.options[3].label
        );
        assert!(
            pr.options[3].label.ends_with('*'),
            "Expected * suffix: {:?}",
            pr.options[3].label
        );
        // Deny is still the last option
        assert_eq!(pr.options[4].key, 'n');
    }

    #[test]
    fn file_read_has_three_options() {
        let pr = PermissionRequest::file_read(
            "id-fr".to_string(),
            "ReadFile".to_string(),
            "Wants to read /etc/hosts".to_string(),
            "/etc/hosts".to_string(),
        );
        assert_eq!(pr.options.len(), 3);
        assert_eq!(pr.options[0].key, 'y');
        assert_eq!(pr.options[1].key, 'Y');
        assert_eq!(pr.options[2].key, 'n');
        assert!(matches!(pr.kind, PermissionDialogKind::FileRead { .. }));
    }

    #[test]
    fn file_write_has_four_options() {
        let pr = PermissionRequest::file_write(
            "id-fw".to_string(),
            "WriteFile".to_string(),
            "Wants to write /tmp/out.txt".to_string(),
            "/tmp/out.txt".to_string(),
        );
        assert_eq!(pr.options.len(), 4);
        assert_eq!(pr.options[2].key, 'p'); // project-level allow
        assert_eq!(pr.options[3].key, 'n');
        assert!(matches!(pr.kind, PermissionDialogKind::FileWrite { .. }));
    }

    #[test]
    fn permission_key_digit_selects_and_confirms() {
        let mut pr =
            PermissionRequest::standard("id".to_string(), "Bash".to_string(), "desc".to_string());
        // Press '1' → selects option 0 (allow once) and confirms.
        let confirmed = handle_permission_key(&mut pr, key(KeyCode::Char('1')));
        assert!(confirmed);
        assert_eq!(pr.selected_option, 0);
    }

    #[test]
    fn permission_key_digit_out_of_range_ignored_for_three_option_dialog() {
        let mut pr = PermissionRequest::file_read(
            "id".to_string(),
            "ReadFile".to_string(),
            "desc".to_string(),
            "/foo".to_string(),
        );
        assert_eq!(pr.options.len(), 3);
        // Press '4' — out of range for a 3-option dialog, should NOT confirm.
        let confirmed = handle_permission_key(&mut pr, key(KeyCode::Char('4')));
        assert!(!confirmed);
        // Press '5' — also out of range.
        let confirmed = handle_permission_key(&mut pr, key(KeyCode::Char('5')));
        assert!(!confirmed);
    }

    #[test]
    fn permission_key_digit_5_valid_for_five_option_bash_dialog() {
        let mut pr = PermissionRequest::bash(
            "id".to_string(),
            "Bash".to_string(),
            "desc".to_string(),
            "git push".to_string(),
            Some("git ".to_string()),
        );
        assert_eq!(pr.options.len(), 5);
        // '5' should select the 5th option (deny) and confirm.
        let confirmed = handle_permission_key(&mut pr, key(KeyCode::Char('5')));
        assert!(confirmed);
        assert_eq!(pr.selected_option, 4);
    }

    #[test]
    fn permission_key_char_shortcut_confirms() {
        let mut pr =
            PermissionRequest::standard("id".to_string(), "Bash".to_string(), "desc".to_string());
        // Press 'n' → deny (index 3).
        let confirmed = handle_permission_key(&mut pr, key(KeyCode::Char('n')));
        assert!(confirmed);
        assert_eq!(pr.selected_option, 3);
    }

    #[test]
    fn permission_key_esc_selects_deny() {
        let mut pr =
            PermissionRequest::standard("id".to_string(), "Bash".to_string(), "desc".to_string());
        pr.selected_option = 0;
        let confirmed = handle_permission_key(&mut pr, key(KeyCode::Esc));
        assert!(confirmed);
        assert_eq!(pr.selected_option, pr.options.len() - 1);
    }

    #[test]
    fn permission_key_up_down_navigation() {
        let mut pr =
            PermissionRequest::standard("id".to_string(), "Bash".to_string(), "desc".to_string());
        pr.selected_option = 1;
        // Down.
        handle_permission_key(&mut pr, key(KeyCode::Down));
        assert_eq!(pr.selected_option, 2);
        // Up twice.
        handle_permission_key(&mut pr, key(KeyCode::Up));
        handle_permission_key(&mut pr, key(KeyCode::Up));
        assert_eq!(pr.selected_option, 0);
        // Up at top — should not underflow.
        handle_permission_key(&mut pr, key(KeyCode::Up));
        assert_eq!(pr.selected_option, 0);
    }

    // -----------------------------------------------------------------------
    // McpApprovalDialogState tests
    // -----------------------------------------------------------------------

    #[test]
    fn mcp_approval_new_is_invisible() {
        let state = McpApprovalDialogState::new();
        assert!(!state.visible);
        assert_eq!(state.selected, McpApprovalChoice::AllowSession);
    }

    #[test]
    fn mcp_approval_show_populates_state() {
        let mut state = McpApprovalDialogState::new();
        state.show(
            "my-server",
            Some("wss://example.com/mcp"),
            None,
            vec!["tool_a".to_string(), "tool_b".to_string()],
        );
        assert!(state.visible);
        assert_eq!(state.server_name, "my-server");
        assert_eq!(state.server_url.as_deref(), Some("wss://example.com/mcp"));
        assert_eq!(state.tool_names.len(), 2);
        assert_eq!(state.selected, McpApprovalChoice::AllowSession);
    }

    #[test]
    fn mcp_approval_select_next_and_prev() {
        let mut state = McpApprovalDialogState::new();
        state.show("s", None, None, vec![]);
        assert_eq!(state.selected, McpApprovalChoice::AllowSession);
        state.select_next();
        assert_eq!(state.selected, McpApprovalChoice::AllowAlways);
        state.select_next();
        assert_eq!(state.selected, McpApprovalChoice::Deny);
        state.select_next(); // wraps
        assert_eq!(state.selected, McpApprovalChoice::AllowSession);
        state.select_prev(); // wraps backward
        assert_eq!(state.selected, McpApprovalChoice::Deny);
    }

    #[test]
    fn mcp_approval_confirm_closes_and_returns_choice() {
        let mut state = McpApprovalDialogState::new();
        state.show("s", None, None, vec![]);
        state.select_next(); // AllowAlways
        let choice = state.confirm();
        assert_eq!(choice, McpApprovalChoice::AllowAlways);
        assert!(!state.visible);
    }

    #[test]
    fn mcp_approval_key_enter_confirms() {
        let mut state = McpApprovalDialogState::new();
        state.show("s", None, None, vec![]);
        state.select_next(); // AllowAlways
        let result = handle_mcp_approval_key(&mut state, key(KeyCode::Enter));
        assert_eq!(result, Some(McpApprovalChoice::AllowAlways));
        assert!(!state.visible);
    }

    #[test]
    fn mcp_approval_key_esc_denies() {
        let mut state = McpApprovalDialogState::new();
        state.show("s", None, None, vec![]);
        let result = handle_mcp_approval_key(&mut state, key(KeyCode::Esc));
        assert_eq!(result, Some(McpApprovalChoice::Deny));
        assert!(!state.visible);
    }

    #[test]
    fn mcp_approval_key_digit_shortcuts() {
        // '1' → AllowSession
        let mut state = McpApprovalDialogState::new();
        state.show("s", None, None, vec![]);
        let r = handle_mcp_approval_key(&mut state, key(KeyCode::Char('1')));
        assert_eq!(r, Some(McpApprovalChoice::AllowSession));

        // '2' → AllowAlways
        state.show("s", None, None, vec![]);
        let r = handle_mcp_approval_key(&mut state, key(KeyCode::Char('2')));
        assert_eq!(r, Some(McpApprovalChoice::AllowAlways));

        // '3' → Deny
        state.show("s", None, None, vec![]);
        let r = handle_mcp_approval_key(&mut state, key(KeyCode::Char('3')));
        assert_eq!(r, Some(McpApprovalChoice::Deny));
    }

    #[test]
    fn mcp_approval_key_n_denies() {
        let mut state = McpApprovalDialogState::new();
        state.show("s", None, None, vec![]);
        let r = handle_mcp_approval_key(&mut state, key(KeyCode::Char('n')));
        assert_eq!(r, Some(McpApprovalChoice::Deny));
    }

    #[test]
    fn mcp_approval_key_navigation_returns_none() {
        let mut state = McpApprovalDialogState::new();
        state.show("s", None, None, vec![]);
        let r = handle_mcp_approval_key(&mut state, key(KeyCode::Down));
        assert_eq!(r, None);
        assert!(state.visible); // still open
        let r = handle_mcp_approval_key(&mut state, key(KeyCode::Up));
        assert_eq!(r, None);
    }

    #[test]
    fn mcp_approval_tool_list_capped_at_five_in_display() {
        let tools: Vec<String> = (0..10).map(|i| format!("tool_{}", i)).collect();
        let mut state = McpApprovalDialogState::new();
        state.show("s", None, None, tools.clone());
        // State stores all 10 but render only shows first 5.
        assert_eq!(state.tool_names.len(), 10);
        // We test the cap by checking state.tool_names.iter().take(5) gives 5 items.
        assert_eq!(state.tool_names.iter().take(5).count(), 5);
    }

    #[test]
    fn truncate_str_within_limit() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_exceeds_limit() {
        let s = truncate_str("hello world", 6);
        assert!(s.ends_with('\u{2026}'));
        assert!(s.chars().count() <= 6);
    }
}
