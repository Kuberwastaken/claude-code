// plugin_views.rs — Plugin hint/recommendation UI elements and plugin list widget.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

/// A dismissible banner shown at the top of the message area when a plugin
/// wants to surface a hint or recommendation to the user.
#[derive(Debug, Clone)]
pub struct PluginHintBanner {
    /// The plugin's display name.
    pub plugin_name: String,
    /// The hint / recommendation message.
    pub message: String,
    /// Whether the user has dismissed this banner (it will not be rendered).
    pub dismissed: bool,
}

impl PluginHintBanner {
    pub fn new(plugin_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            plugin_name: plugin_name.into(),
            message: message.into(),
            dismissed: false,
        }
    }

    /// Mark the banner as dismissed.
    pub fn dismiss(&mut self) {
        self.dismissed = true;
    }

    /// Return `true` if this banner should be shown.
    pub fn is_visible(&self) -> bool {
        !self.dismissed
    }
}

/// Render the first undismissed plugin hint banner into `area`.
/// Returns the height consumed (0 if nothing rendered).
pub fn render_plugin_hints(frame: &mut Frame, hints: &[PluginHintBanner], area: Rect) -> u16 {
    let hint = match hints.iter().find(|h| h.is_visible()) {
        Some(h) => h,
        None => return 0,
    };

    // 3-row banner (border + 1 content line + border)
    let banner_height = 3u16;
    if area.height < banner_height {
        return 0;
    }

    let banner_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: banner_height,
    };

    let inner_width = area.width.saturating_sub(4) as usize;
    let content = format!(" [{}] {} [Esc to dismiss]", hint.plugin_name, hint.message);
    let display = if content.len() > inner_width {
        format!("{}…", &content[..inner_width.saturating_sub(1)])
    } else {
        content
    };

    let lines = vec![Line::from(vec![Span::styled(
        display,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::ITALIC),
    )])];

    frame.render_widget(Clear, banner_area);
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Plugin: {} ", hint.plugin_name))
            .border_style(Style::default().fg(Color::Magenta)),
    );
    frame.render_widget(para, banner_area);

    banner_height
}

// ---------------------------------------------------------------------------
// Plugin list item — a lightweight display record for the TUI
// ---------------------------------------------------------------------------

/// A concise, displayable summary of one loaded plugin.
/// Constructed from `claurst_plugins::LoadedPlugin` when the caller does not want
/// to take a direct dependency on the plugins crate inside TUI rendering code.
#[derive(Debug, Clone)]
pub struct PluginListItem {
    pub name: String,
    pub version: String,
    pub description: String,
    pub enabled: bool,
    pub source: String,
    pub command_count: usize,
    pub hook_count: usize,
}

impl PluginListItem {
    /// Render one line suitable for a list widget.
    fn to_line(&self) -> Line<'static> {
        let status_style = if self.enabled {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let status = if self.enabled { "on " } else { "off" };

        let mut parts: Vec<Span<'static>> = vec![
            Span::styled(format!(" {} ", status), status_style),
            Span::styled(
                format!("{} ", self.name.clone()),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("v{} ", self.version.clone()),
                Style::default().fg(Color::DarkGray),
            ),
        ];

        if !self.description.is_empty() {
            parts.push(Span::styled(
                format!("— {} ", self.description.clone()),
                Style::default().fg(Color::White),
            ));
        }

        let mut meta: Vec<String> = Vec::new();
        if self.command_count > 0 {
            meta.push(format!(
                "{} cmd{}",
                self.command_count,
                if self.command_count == 1 { "" } else { "s" }
            ));
        }
        if self.hook_count > 0 {
            meta.push(format!(
                "{} hook{}",
                self.hook_count,
                if self.hook_count == 1 { "" } else { "s" }
            ));
        }
        if !meta.is_empty() {
            parts.push(Span::styled(
                format!("({})", meta.join(", ")),
                Style::default().fg(Color::Cyan),
            ));
        }

        Line::from(parts)
    }
}

// ---------------------------------------------------------------------------
// Interactive plugin list state
// ---------------------------------------------------------------------------

/// Interactive state for a navigable plugin list widget.
#[derive(Debug, Default)]
pub struct PluginListState {
    pub items: Vec<PluginListItem>,
    /// Currently highlighted plugin index.
    pub selected: usize,
    /// Scroll offset for the list.
    pub scroll: usize,
    /// Whether the detail panel is shown for the selected plugin.
    pub show_detail: bool,
}

impl PluginListState {
    pub fn new(items: Vec<PluginListItem>) -> Self {
        Self {
            items,
            ..Default::default()
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll {
                self.scroll = self.selected;
            }
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.items.len() {
            self.selected += 1;
        }
    }

    pub fn toggle_detail(&mut self) {
        self.show_detail = !self.show_detail;
    }

    pub fn selected_item(&self) -> Option<&PluginListItem> {
        self.items.get(self.selected)
    }
}

/// Render a list of plugin summary items into `area`.
///
/// Shows a bordered box titled "Plugins" with one line per plugin.
/// When `state.show_detail` is true and a plugin is selected, a detail
/// panel is rendered below the list.
/// Returns the height consumed.
pub fn render_plugin_list(
    frame: &mut Frame,
    state: &mut PluginListState,
    area: Rect,
    title: Option<&str>,
) -> u16 {
    if area.height < 3 {
        return 0;
    }

    let items = &state.items;
    let list_items: Vec<ListItem> = items.iter().map(|p| ListItem::new(p.to_line())).collect();

    let block_title = title.unwrap_or("Plugins");
    let total = items.len();
    let enabled = items.iter().filter(|p| p.enabled).count();

    // If detail panel is shown we split area vertically.
    let (list_area, detail_area_opt) = if state.show_detail && state.selected_item().is_some() {
        let detail_height = 9u16; // border + 6 content lines + border
        if area.height > detail_height + 3 {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(detail_height)])
                .split(area);
            (chunks[0], Some(chunks[1]))
        } else {
            (area, None)
        }
    } else {
        (area, None)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ({}/{} enabled) ", block_title, enabled, total))
        .border_style(Style::default().fg(Color::Magenta));

    let highlight_style = Style::default()
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);

    let list = List::new(list_items)
        .block(block)
        .highlight_style(highlight_style)
        .highlight_symbol("> ");

    let mut list_state = ListState::default();
    if !state.items.is_empty() {
        list_state.select(Some(state.selected));
    }

    frame.render_widget(Clear, list_area);
    frame.render_stateful_widget(list, list_area, &mut list_state);

    if let (Some(detail_area), Some(item)) = (detail_area_opt, state.selected_item()) {
        render_plugin_detail(frame, item, detail_area);
    }

    area.height
}

/// Render a detail panel for a single plugin into `area`.
pub fn render_plugin_detail(frame: &mut Frame, item: &PluginListItem, area: Rect) {
    if area.height < 3 {
        return;
    }

    let status = if item.enabled { "enabled" } else { "disabled" };
    let status_style = if item.enabled {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let cmd_label = if item.command_count == 1 {
        "command"
    } else {
        "commands"
    };
    let hook_label = if item.hook_count == 1 {
        "hook"
    } else {
        "hooks"
    };

    let lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("  Name:    ", Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("{} v{}", item.name, item.version),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(format!("[{}]", status), status_style),
        ]),
        Line::from(vec![
            Span::styled("  Desc:    ", Style::default().fg(Color::Cyan)),
            Span::raw(item.description.clone()),
        ]),
        Line::from(vec![
            Span::styled("  Source:  ", Style::default().fg(Color::Cyan)),
            Span::styled(item.source.clone(), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("  Counts:  ", Style::default().fg(Color::Cyan)),
            Span::styled(
                format!(
                    "{} {}  •  {} {}",
                    item.command_count, cmd_label, item.hook_count, hook_label
                ),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![]),
        Line::from(vec![Span::styled(
            "  [Enter] toggle detail   [j/k] navigate",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Plugin Detail ")
        .border_style(Style::default().fg(Color::Blue));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(Clear, area);
    frame.render_widget(para, area);
}

// ---------------------------------------------------------------------------
// Plugin banner
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_visibility() {
        let mut b = PluginHintBanner::new("my-plugin", "Use /foo for more info");
        assert!(b.is_visible());
        b.dismiss();
        assert!(!b.is_visible());
    }

    #[test]
    fn find_first_undismissed() {
        let hints = vec![
            {
                let mut b = PluginHintBanner::new("a", "msg a");
                b.dismiss();
                b
            },
            PluginHintBanner::new("b", "msg b"),
        ];
        let visible = hints.iter().find(|h| h.is_visible()).unwrap();
        assert_eq!(visible.plugin_name, "b");
    }

    fn make_items(n: usize) -> Vec<PluginListItem> {
        (0..n)
            .map(|i| PluginListItem {
                name: format!("plugin-{i}"),
                version: "1.0.0".to_string(),
                description: String::new(),
                enabled: true,
                source: String::new(),
                command_count: 0,
                hook_count: 0,
            })
            .collect()
    }

    #[test]
    fn plugin_list_state_move_down() {
        let mut s = PluginListState::new(make_items(3));
        assert_eq!(s.selected, 0);
        s.move_down();
        assert_eq!(s.selected, 1);
        s.move_down();
        assert_eq!(s.selected, 2);
        // At end, stays put
        s.move_down();
        assert_eq!(s.selected, 2);
    }

    #[test]
    fn plugin_list_state_move_up() {
        let mut s = PluginListState::new(make_items(3));
        s.selected = 2;
        s.move_up();
        assert_eq!(s.selected, 1);
        s.move_up();
        assert_eq!(s.selected, 0);
        // At top, stays put
        s.move_up();
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn plugin_list_state_move_up_adjusts_scroll() {
        let mut s = PluginListState::new(make_items(5));
        s.selected = 3;
        s.scroll = 3;
        s.move_up();
        assert_eq!(s.selected, 2);
        // scroll should be pulled back to match selected
        assert_eq!(s.scroll, 2);
    }

    #[test]
    fn plugin_list_state_toggle_detail() {
        let mut s = PluginListState::new(make_items(2));
        assert!(!s.show_detail);
        s.toggle_detail();
        assert!(s.show_detail);
        s.toggle_detail();
        assert!(!s.show_detail);
    }

    #[test]
    fn plugin_list_state_selected_item() {
        let mut s = PluginListState::new(make_items(3));
        assert_eq!(s.selected_item().map(|i| i.name.as_str()), Some("plugin-0"));
        s.move_down();
        assert_eq!(s.selected_item().map(|i| i.name.as_str()), Some("plugin-1"));
    }

    #[test]
    fn plugin_list_state_empty() {
        let mut s = PluginListState::new(vec![]);
        assert!(s.selected_item().is_none());
        // These should not panic on empty
        s.move_up();
        s.move_down();
        assert_eq!(s.selected, 0);
    }
}
