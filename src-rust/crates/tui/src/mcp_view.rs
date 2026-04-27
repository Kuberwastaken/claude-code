//! MCP server management UI.
//! Mirrors src/components/mcp/ (12 files).

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::overlays::{
    centered_rect, render_dark_overlay_buf, render_dialog_bg_buf, CLAURST_ACCENT, CLAURST_MUTED,
    CLAURST_PANEL_BG, CLAURST_PANEL_BORDER, CLAURST_TEXT,
};

// ---------------------------------------------------------------------------
// Data types (view-level; mirrors cc_mcp types)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpViewStatus {
    Connected,
    Connecting,
    Disconnected,
    Error,
}

impl McpViewStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Connecting => "connecting",
            Self::Disconnected => "disconnected",
            Self::Error => "error",
        }
    }
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Connected => "●",
            Self::Connecting => "◌",
            Self::Disconnected => "○",
            Self::Error => "⚠",
        }
    }
    pub fn color(&self) -> Color {
        match self {
            Self::Connected => Color::Green,
            Self::Connecting => Color::Yellow,
            Self::Disconnected => Color::DarkGray,
            Self::Error => Color::Red,
        }
    }
}

#[derive(Debug, Clone)]
pub struct McpServerView {
    pub name: String,
    pub transport: String, // "stdio" | "sse" | "http"
    pub status: McpViewStatus,
    pub tool_count: usize,
    pub resource_count: usize,
    pub prompt_count: usize,
    pub resources: Vec<String>,
    pub prompts: Vec<String>,
    pub error_message: Option<String>,
    /// All tools provided by this server.
    pub tools: Vec<McpToolView>,
}

#[derive(Debug, Clone)]
pub struct McpToolView {
    pub name: String,
    pub server: String,
    pub description: String,
    pub input_schema: Option<String>,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpViewPane {
    ServerList,
    ToolList,
    ToolDetail,
}

#[derive(Debug, Clone)]
pub struct McpViewState {
    pub open: bool,
    pub servers: Vec<McpServerView>,
    pub active_pane: McpViewPane,
    pub selected_server: usize,
    pub selected_tool: usize,
    pub tool_search: String,
    pub server_scroll: usize,
    pub tool_scroll: usize,
    /// Whether the full error detail for the selected server is expanded.
    pub error_expanded: bool,
}

impl McpViewState {
    pub fn new() -> Self {
        Self {
            open: false,
            servers: Vec::new(),
            active_pane: McpViewPane::ServerList,
            selected_server: 0,
            selected_tool: 0,
            tool_search: String::new(),
            server_scroll: 0,
            tool_scroll: 0,
            error_expanded: false,
        }
    }

    pub fn open(&mut self, servers: Vec<McpServerView>) {
        self.servers = servers;
        self.selected_server = 0;
        self.selected_tool = 0;
        self.tool_search.clear();
        self.active_pane = McpViewPane::ServerList;
        self.error_expanded = false;
        self.open = true;
    }

    /// Toggle full error detail for the currently selected server.
    pub fn toggle_error_detail(&mut self) {
        self.error_expanded = !self.error_expanded;
    }

    pub fn close(&mut self) { self.open = false; }

    pub fn switch_pane(&mut self) {
        self.active_pane = match self.active_pane {
            McpViewPane::ServerList => McpViewPane::ToolList,
            McpViewPane::ToolList => McpViewPane::ToolDetail,
            McpViewPane::ToolDetail => McpViewPane::ServerList,
        };
    }

    pub fn select_prev(&mut self) {
        match self.active_pane {
            McpViewPane::ServerList => {
                let count = self.servers.len();
                if count == 0 {
                    return;
                }
                if self.selected_server == 0 {
                    self.selected_server = count - 1;
                } else {
                    self.selected_server -= 1;
                }
            }
            McpViewPane::ToolList | McpViewPane::ToolDetail => {
                let count = self.filtered_tools().len();
                if count == 0 {
                    return;
                }
                if self.selected_tool == 0 {
                    self.selected_tool = count - 1;
                } else {
                    self.selected_tool -= 1;
                }
            }
        }
    }

    pub fn select_next(&mut self) {
        match self.active_pane {
            McpViewPane::ServerList => {
                let count = self.servers.len();
                if count > 0 {
                    self.selected_server = (self.selected_server + 1) % count;
                }
            }
            McpViewPane::ToolList | McpViewPane::ToolDetail => {
                let count = self.filtered_tools().len();
                if count > 0 {
                    self.selected_tool = (self.selected_tool + 1) % count;
                }
            }
        }
    }

    pub fn push_search_char(&mut self, c: char) {
        self.tool_search.push(c);
        self.selected_tool = 0;
    }

    pub fn pop_search_char(&mut self) {
        self.tool_search.pop();
        self.selected_tool = 0;
    }

    /// All tools across all servers, filtered by search query.
    pub fn filtered_tools(&self) -> Vec<&McpToolView> {
        let q = self.tool_search.to_lowercase();
        self.servers
            .iter()
            .flat_map(|s| s.tools.iter())
            .filter(|t| {
                q.is_empty()
                    || t.name.to_lowercase().contains(&q)
                    || t.description.to_lowercase().contains(&q)
                    || t.server.to_lowercase().contains(&q)
            })
            .collect()
    }
}

impl Default for McpViewState {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub fn render_mcp_view(state: &McpViewState, area: Rect, buf: &mut Buffer) {
    if !state.open { return; }

    let w = (area.width * 9 / 10).max(50).min(area.width);
    let h = (area.height * 4 / 5).max(15).min(area.height);
    let dialog = centered_rect(w, h, area);
    render_dark_overlay_buf(buf, area);
    render_dialog_bg_buf(buf, dialog);

    let inner = Rect {
        x: dialog.x + 2,
        y: dialog.y + 1,
        width: dialog.width.saturating_sub(4),
        height: dialog.height.saturating_sub(2),
    };

    if inner.height < 5 {
        return;
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let title = Line::from(vec![
        Span::styled(" MCP", Style::default().fg(CLAURST_ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!(" — {} servers · {} tools", state.servers.len(), state.filtered_tools().len()),
            Style::default().fg(CLAURST_MUTED),
        ),
        Span::styled(
            format!("{:>width$}", "Esc close", width = inner.width.saturating_sub(26) as usize),
            Style::default().fg(CLAURST_MUTED),
        ),
    ]);
    Paragraph::new(title)
        .style(Style::default().bg(CLAURST_PANEL_BG).fg(CLAURST_TEXT))
        .render(layout[0], buf);

    // Split: servers (left 35%) | tools (right 65%)
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(layout[1]);

    render_server_list(state, panes[0], buf);

    // Right pane: tool list + tool detail
    let right_panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(panes[1]);

    render_tool_list(state, right_panes[0], buf);
    render_tool_detail(state, right_panes[1], buf);

    let footer = Line::from(vec![
        Span::styled(" Tab ", Style::default().fg(CLAURST_ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("switch pane  "),
        Span::styled(" / ", Style::default().fg(CLAURST_ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("filter tools  "),
        Span::styled(" e ", Style::default().fg(CLAURST_ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("error detail  "),
        Span::styled(" a ", Style::default().fg(CLAURST_ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("auth  "),
        Span::styled(" r ", Style::default().fg(CLAURST_ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("reconnect"),
    ]);
    Paragraph::new(footer)
        .style(Style::default().bg(CLAURST_PANEL_BG).fg(CLAURST_MUTED))
        .render(layout[2], buf);
}

fn render_server_list(state: &McpViewState, area: Rect, buf: &mut Buffer) {
    let focused = state.active_pane == McpViewPane::ServerList;
    let border_style = if focused {
        Style::default().fg(CLAURST_ACCENT)
    } else {
        Style::default().fg(CLAURST_PANEL_BORDER)
    };
    Block::default()
        .title(Span::styled(
            " Servers ",
            Style::default()
                .fg(if focused { CLAURST_ACCENT } else { CLAURST_MUTED })
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(CLAURST_PANEL_BG).fg(CLAURST_TEXT))
        .render(area, buf);

    let inner = Rect { x: area.x + 1, y: area.y + 1, width: area.width.saturating_sub(2), height: area.height.saturating_sub(2) };

    // Group by transport
    let stdio: Vec<_> = state.servers.iter().enumerate().filter(|(_, s)| s.transport == "stdio").collect();
    let sse: Vec<_> = state.servers.iter().enumerate().filter(|(_, s)| s.transport == "sse").collect();
    let http: Vec<_> = state.servers.iter().enumerate().filter(|(_, s)| s.transport == "http").collect();

    let mut row = 0u16;

    let render_group = |group: &Vec<(usize, &McpServerView)>, label: &str, row: &mut u16, area: Rect, buf: &mut Buffer, selected: usize, focused: bool| {
        if group.is_empty() { return; }
        if *row >= area.height { return; }
        Paragraph::new(Line::from(vec![Span::styled(label.to_string(), Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))]))
            .render(Rect { x: area.x, y: area.y + *row, width: area.width, height: 1 }, buf);
        *row += 1;
        for (idx, server) in group {
            if *row >= area.height { break; }
            let sel = *idx == selected && focused;
            let prefix = if sel { "› " } else { "  " };
            let row_text = pad_line(
                &format!(
                    "{}{} {}  {} tools  {} res  {} prompts",
                    prefix,
                    server.status.badge(),
                    server.name,
                    server.tool_count,
                    server.resource_count,
                    server.prompt_count
                ),
                area.width,
            );
            let line = Line::from(vec![Span::styled(
                row_text,
                if sel {
                    Style::default().fg(Color::Black).bg(CLAURST_ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(CLAURST_TEXT)
                },
            )]);
            Paragraph::new(line).render(Rect { x: area.x, y: area.y + *row, width: area.width, height: 1 }, buf);
            if let Some(err) = &server.error_message {
                *row += 1;
                if *row < area.height {
                    let short: String = err.chars().take(area.width as usize - 4).collect();
                    Paragraph::new(Line::from(vec![Span::styled(format!("    {}", short), Style::default().fg(Color::Red))]))
                        .render(Rect { x: area.x, y: area.y + *row, width: area.width, height: 1 }, buf);
                }
            }
            *row += 1;
        }
    };

    render_group(&stdio, "stdio", &mut row, inner, buf, state.selected_server, focused);
    render_group(&sse, "SSE", &mut row, inner, buf, state.selected_server, focused);
    render_group(&http, "HTTP", &mut row, inner, buf, state.selected_server, focused);
}

fn render_tool_list(state: &McpViewState, area: Rect, buf: &mut Buffer) {
    let focused = state.active_pane == McpViewPane::ToolList || state.active_pane == McpViewPane::ToolDetail;
    let border_style = if focused {
        Style::default().fg(CLAURST_ACCENT)
    } else {
        Style::default().fg(CLAURST_PANEL_BORDER)
    };
    Block::default()
        .title(Span::styled(
            " Tools ",
            Style::default()
                .fg(if focused { CLAURST_ACCENT } else { CLAURST_MUTED })
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(CLAURST_PANEL_BG).fg(CLAURST_TEXT))
        .render(area, buf);

    let inner = Rect { x: area.x + 1, y: area.y + 1, width: area.width.saturating_sub(2), height: area.height.saturating_sub(2) };

    // Search bar
    let search_line = Line::from(vec![
        Span::styled("/ ", Style::default().fg(CLAURST_ACCENT)),
        Span::styled(
            if state.tool_search.is_empty() { "filter tools".to_string() } else { state.tool_search.clone() },
            Style::default().fg(if state.tool_search.is_empty() { CLAURST_MUTED } else { CLAURST_TEXT }),
        ),
    ]);
    Paragraph::new(search_line).render(Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 }, buf);

    let list_area = Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: inner.height.saturating_sub(1) };
    let tools = state.filtered_tools();
    let max_visible = list_area.height as usize;
    let start = state.selected_tool.saturating_sub(max_visible / 2);

    for (i, tool) in tools[start..].iter().enumerate() {
        if i >= max_visible { break; }
        let sel = start + i == state.selected_tool;
        let avail = list_area.width.saturating_sub(20) as usize;
        let name = format!("{}:{}", tool.server, tool.name);
        let name_short: String = name.chars().take(avail).collect();
        let desc: String = tool.description.chars().take(30).collect();
        let row = pad_line(&format!("{}  {}", name_short, desc), list_area.width);
        let line = Line::from(vec![Span::styled(
            row,
            if sel {
                Style::default().fg(Color::Black).bg(CLAURST_ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(CLAURST_TEXT)
            },
        )]);
        Paragraph::new(line).render(
            Rect { x: list_area.x, y: list_area.y + i as u16, width: list_area.width, height: 1 },
            buf,
        );
    }
}

fn render_tool_detail(state: &McpViewState, area: Rect, buf: &mut Buffer) {
    let focused = state.active_pane == McpViewPane::ToolDetail;
    let border_style = if focused {
        Style::default().fg(CLAURST_ACCENT)
    } else {
        Style::default().fg(CLAURST_PANEL_BORDER)
    };

    // If error is expanded, show full error text in this pane
    if state.error_expanded {
        if let Some(server) = state.servers.get(state.selected_server) {
            if let Some(ref err_msg) = server.error_message {
                Block::default()
                    .title(" Error Detail [e: close] ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red))
                    .style(Style::default().bg(CLAURST_PANEL_BG).fg(CLAURST_TEXT))
                    .render(area, buf);
                let inner = Rect {
                    x: area.x + 1,
                    y: area.y + 1,
                    width: area.width.saturating_sub(2),
                    height: area.height.saturating_sub(2),
                };
                let lines: Vec<Line> = err_msg
                    .lines()
                    .map(|l| Line::from(vec![Span::styled(l.to_string(), Style::default().fg(Color::White))]))
                    .collect();
                Paragraph::new(lines)
                    .wrap(ratatui::widgets::Wrap { trim: false })
                    .render(inner, buf);
                return;
            }
        }
    }

    Block::default()
        .title(Span::styled(
            " Tool Detail ",
            Style::default()
                .fg(if focused { CLAURST_ACCENT } else { CLAURST_MUTED })
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(CLAURST_PANEL_BG).fg(CLAURST_TEXT))
        .render(area, buf);

    let inner = Rect { x: area.x + 1, y: area.y + 1, width: area.width.saturating_sub(2), height: area.height.saturating_sub(2) };

    let tools = state.filtered_tools();
    let Some(tool) = tools.get(state.selected_tool) else {
        Paragraph::new("Select a tool to view details.")
            .style(Style::default().fg(Color::DarkGray))
            .render(inner, buf);
        return;
    };

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}:{}", tool.server, tool.name),
            Style::default().fg(CLAURST_ACCENT).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::default());
    for line in tool.description.lines() {
        lines.push(Line::from(vec![Span::raw(line.to_string())]));
    }
    if let Some(schema) = &tool.input_schema {
        lines.push(Line::default());
        lines.push(Line::from(vec![Span::styled("Input:", Style::default().fg(Color::DarkGray))]));
        for line in schema.lines().take(10) {
            lines.push(Line::from(vec![Span::styled(format!("  {}", line), Style::default().fg(Color::DarkGray))]));
        }
    }

    if let Some(server) = state.servers.iter().find(|server| server.name == tool.server) {
        lines.push(Line::default());
        lines.push(Line::from(vec![Span::styled(
            format!(
                "Server: {}  [{}]  {} resources  {} prompts",
                server.name,
                server.status.label(),
                server.resource_count,
                server.prompt_count
            ),
            Style::default().fg(server.status.color()),
        )]));

        if !server.resources.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "Resources:",
                Style::default().fg(CLAURST_MUTED),
            )]));
            for resource in server.resources.iter().take(3) {
                lines.push(Line::from(vec![Span::styled(
                    format!("  - {}", resource),
                    Style::default().fg(Color::White),
                )]));
            }
        }

        if !server.prompts.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "Prompts:",
                Style::default().fg(CLAURST_MUTED),
            )]));
            for prompt in server.prompts.iter().take(3) {
                lines.push(Line::from(vec![Span::styled(
                    format!("  - {}", prompt),
                    Style::default().fg(Color::White),
                )]));
            }
        }
    }

    Paragraph::new(lines)
        .wrap(ratatui::widgets::Wrap { trim: false })
        .render(inner, buf);
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn make_server(name: &str, status: McpViewStatus, error: Option<&str>) -> McpServerView {
        McpServerView {
            name: name.to_string(),
            transport: "stdio".to_string(),
            status,
            tool_count: 2,
            resource_count: 0,
            prompt_count: 0,
            resources: Vec::new(),
            prompts: Vec::new(),
            error_message: error.map(|e| e.to_string()),
            tools: vec![
                McpToolView {
                    name: "tool_a".to_string(),
                    server: name.to_string(),
                    description: "Does A".to_string(),
                    input_schema: None,
                },
            ],
        }
    }

    #[test]
    fn mcp_view_state_defaults() {
        let state = McpViewState::new();
        assert!(!state.open);
        assert!(!state.error_expanded);
        assert_eq!(state.selected_server, 0);
    }

    #[test]
    fn mcp_view_open_resets_error_expanded() {
        let mut state = McpViewState::new();
        state.error_expanded = true;
        state.open(vec![make_server("test", McpViewStatus::Connected, None)]);
        assert!(!state.error_expanded, "open() should reset error_expanded");
        assert!(state.open);
    }

    #[test]
    fn mcp_view_toggle_error_detail() {
        let mut state = McpViewState::new();
        assert!(!state.error_expanded);
        state.toggle_error_detail();
        assert!(state.error_expanded);
        state.toggle_error_detail();
        assert!(!state.error_expanded);
    }

    #[test]
    fn mcp_view_renders_without_panic() {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = McpViewState::new();
        state.open(vec![
            make_server("fs-server", McpViewStatus::Connected, None),
            make_server("err-server", McpViewStatus::Error, Some("connection refused")),
        ]);
        terminal.draw(|frame| {
            render_mcp_view(&state, frame.area(), frame.buffer_mut());
        }).unwrap();
        let buf = terminal.backend().buffer().clone();
        let content: String = buf.content().iter().map(|c| c.symbol().chars().next().unwrap_or(' ')).collect();
        assert!(content.contains("MCP") || content.contains("Servers"));
    }

    #[test]
    fn mcp_view_error_expanded_renders_error_detail() {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = McpViewState::new();
        state.open(vec![make_server("broken", McpViewStatus::Error, Some("timeout: no response"))]);
        state.error_expanded = true;
        terminal.draw(|frame| {
            render_mcp_view(&state, frame.area(), frame.buffer_mut());
        }).unwrap();
        let buf = terminal.backend().buffer().clone();
        let content: String = buf.content().iter().map(|c| c.symbol().chars().next().unwrap_or(' ')).collect();
        assert!(content.contains("timeout") || content.contains("Error Detail"));
    }

    #[test]
    fn mcp_view_footer_renders_auth_hotkey() {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut state = McpViewState::new();
        state.open(vec![make_server("fs-server", McpViewStatus::Connected, None)]);
        terminal.draw(|frame| {
            render_mcp_view(&state, frame.area(), frame.buffer_mut());
        }).unwrap();
        let buf = terminal.backend().buffer().clone();
        let content: String = buf.content().iter().map(|c| c.symbol().chars().next().unwrap_or(' ')).collect();
        assert!(content.contains("auth") || content.contains(" a "));
    }

    #[test]
    fn mcp_view_closed_renders_nothing() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let state = McpViewState::new(); // open = false
        let before = terminal.backend().buffer().clone();
        terminal.draw(|frame| {
            render_mcp_view(&state, frame.area(), frame.buffer_mut());
        }).unwrap();
        assert_eq!(terminal.backend().buffer().content(), before.content());
    }
}

