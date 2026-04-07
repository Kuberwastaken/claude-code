// settings_screen.rs — Full-screen tabbed settings interface.
//
// Opened by /config or /settings commands. Provides a tabbed UI for
// viewing and editing General, Display, Privacy, Advanced, and KeyBindings
// settings. Changes are persisted via Settings::save_sync().

use crate::overlays::{
    centered_rect, render_dark_overlay, render_dialog_bg, CLAURST_ACCENT, CLAURST_MUTED,
    CLAURST_PANEL_BG, CLAURST_TEXT,
};
use claurst_core::config::{Config, Settings};
use claurst_core::keybindings::default_bindings;
use claurst_core::output_styles::builtin_styles;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Tabs, Wrap};
use ratatui::Frame;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Display,
    Privacy,
    Advanced,
    KeyBindings,
}

impl SettingsTab {
    pub fn all() -> &'static [SettingsTab] {
        &[
            SettingsTab::General,
            SettingsTab::Display,
            SettingsTab::Privacy,
            SettingsTab::Advanced,
            SettingsTab::KeyBindings,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Display => "Display",
            SettingsTab::Privacy => "Privacy",
            SettingsTab::Advanced => "Advanced",
            SettingsTab::KeyBindings => "KeyBindings",
        }
    }

    pub fn index(&self) -> usize {
        Self::all().iter().position(|t| t == self).unwrap_or(0)
    }
}

pub struct SettingsScreen {
    pub visible: bool,
    pub active_tab: SettingsTab,
    pub scroll_offset: u16,
    /// Which field is being edited (field name as key).
    pub edit_field: Option<String>,
    /// Current buffer content while editing a field.
    pub edit_value: String,
    /// Snapshot of settings at open time for display.
    pub settings_snapshot: Settings,
    /// Pending changes (field_name → new_value string).
    pub pending_changes: std::collections::HashMap<String, String>,

    // ---- Real settings fields ----
    /// Whether auto-compact is enabled.
    pub auto_compact_enabled: bool,
    /// Auto-compact threshold (0-100%).
    pub auto_compact_threshold: u8,
    /// Whether desktop notifications are enabled.
    pub notifications_enabled: bool,
    /// Whether to reduce UI motion (animations).
    pub reduce_motion: bool,
    /// Whether to show turn duration in status bar.
    pub show_turn_duration: bool,
    /// Whether the terminal progress bar is enabled.
    pub terminal_progress_bar: bool,

    /// Index of the currently-selected interactive field within the active tab.
    pub selected_field: usize,
}

impl SettingsScreen {
    pub fn new() -> Self {
        let settings_snapshot = Settings::load_sync().unwrap_or_default();
        let auto_compact_enabled = settings_snapshot.config.auto_compact;
        let auto_compact_threshold = {
            let t = settings_snapshot.config.compact_threshold;
            if t > 0.0 {
                (t * 100.0).round() as u8
            } else {
                95
            }
        };
        Self {
            visible: false,
            active_tab: SettingsTab::General,
            scroll_offset: 0,
            edit_field: None,
            edit_value: String::new(),
            settings_snapshot,
            pending_changes: std::collections::HashMap::new(),
            auto_compact_enabled,
            auto_compact_threshold,
            notifications_enabled: true,
            reduce_motion: false,
            show_turn_duration: false,
            terminal_progress_bar: true,
            selected_field: 0,
        }
    }

    pub fn open(&mut self) {
        self.settings_snapshot = Settings::load_sync().unwrap_or_default();
        self.pending_changes.clear();
        self.edit_field = None;
        self.edit_value.clear();
        self.scroll_offset = 0;
        self.active_tab = SettingsTab::General;
        self.selected_field = 0;
        self.visible = true;

        // Wire real boolean settings from the settings snapshot.
        self.auto_compact_enabled = read_setting_bool(
            &self.settings_snapshot,
            "autoCompact",
            self.settings_snapshot.config.auto_compact,
        );
        self.auto_compact_threshold =
            read_setting_u8(&self.settings_snapshot, "autoCompactThreshold", {
                let t = self.settings_snapshot.config.compact_threshold;
                if t > 0.0 {
                    (t * 100.0).round() as u8
                } else {
                    95
                }
            });
        self.notifications_enabled =
            read_setting_bool(&self.settings_snapshot, "notifications", true);
        self.reduce_motion = read_setting_bool(&self.settings_snapshot, "reduceMotion", false);
        self.show_turn_duration =
            read_setting_bool(&self.settings_snapshot, "showTurnDuration", false);
        self.terminal_progress_bar =
            read_setting_bool(&self.settings_snapshot, "terminalProgressBar", true);
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.edit_field = None;
        self.edit_value.clear();
    }

    pub fn next_tab(&mut self) {
        let idx = self.active_tab.index();
        let next = (idx + 1) % SettingsTab::all().len();
        self.active_tab = SettingsTab::all()[next].clone();
        self.scroll_offset = 0;
        self.selected_field = 0;
        self.edit_field = None;
        self.edit_value.clear();
    }

    pub fn prev_tab(&mut self) {
        let idx = self.active_tab.index();
        let prev = if idx == 0 {
            SettingsTab::all().len() - 1
        } else {
            idx - 1
        };
        self.active_tab = SettingsTab::all()[prev].clone();
        self.scroll_offset = 0;
        self.selected_field = 0;
        self.edit_field = None;
        self.edit_value.clear();
    }

    pub fn scroll_up(&mut self) {
        if self.selected_field > 0 {
            self.selected_field -= 1;
        }
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        let max_fields = self.max_fields_for_tab();
        if self.selected_field + 1 < max_fields {
            self.selected_field += 1;
        }
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    /// Returns the number of interactive (togglable) fields in the current tab.
    fn max_fields_for_tab(&self) -> usize {
        match &self.active_tab {
            SettingsTab::General => 3, // auto-compact, notifications, show-turn-duration
            SettingsTab::Display => 2, // reduce-motion, terminal-progress-bar
            _ => 0,
        }
    }

    /// Start editing a field by name, seeding the buffer with current value.
    pub fn start_edit(&mut self, field: &str, current_value: &str) {
        self.edit_field = Some(field.to_string());
        self.edit_value = current_value.to_string();
    }

    /// Commit the current edit to pending_changes.
    pub fn commit_edit(&mut self) {
        if let Some(field) = self.edit_field.take() {
            let value = std::mem::take(&mut self.edit_value);
            self.pending_changes.insert(field, value);
        }
    }

    /// Discard the current edit.
    pub fn cancel_edit(&mut self) {
        self.edit_field = None;
        self.edit_value.clear();
    }

    /// Apply all pending changes to settings and persist them.
    pub fn apply_and_save(&mut self, config: &mut Config) {
        for (field, value) in &self.pending_changes {
            match field.as_str() {
                "model" => {
                    config.model = if value.is_empty() {
                        None
                    } else {
                        Some(value.clone())
                    };
                }
                "max_tokens" => {
                    if let Ok(n) = value.parse::<u32>() {
                        config.max_tokens = Some(n);
                    }
                }
                "output_style" => {
                    config.output_style = if value.is_empty() {
                        None
                    } else {
                        Some(value.clone())
                    };
                }
                _ => {}
            }
        }
        // Update snapshot and persist
        self.settings_snapshot.config = config.clone();
        let _ = self.settings_snapshot.save_sync();
        self.pending_changes.clear();
    }
}

impl Default for SettingsScreen {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Settings I/O helpers
// ---------------------------------------------------------------------------

/// Read a boolean value from `settings.json` by camelCase key, falling back to
/// `default` when the file is absent or the key is missing.
fn read_setting_bool(_settings: &Settings, key: &str, default: bool) -> bool {
    let path = claurst_core::config::Settings::config_dir().join("settings.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(b) = val.get(key).and_then(|v| v.as_bool()) {
                return b;
            }
        }
    }
    default
}

/// Read a u8 value from `settings.json` by camelCase key, falling back to
/// `default` when the file is absent or the key is missing.
fn read_setting_u8(_settings: &Settings, key: &str, default: u8) -> u8 {
    let path = claurst_core::config::Settings::config_dir().join("settings.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(n) = val.get(key).and_then(|v| v.as_u64()) {
                return n as u8;
            }
        }
    }
    default
}

/// Write a single boolean key-value pair to `settings.json`, preserving other
/// fields already present in the file.
fn save_setting_bool(key: &str, value: bool) {
    let path = claurst_core::config::Settings::config_dir().join("settings.json");
    let mut val: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
    if let Some(obj) = val.as_object_mut() {
        obj.insert(key.to_string(), serde_json::Value::Bool(value));
    }
    if let Ok(json) = serde_json::to_string_pretty(&val) {
        let _ = std::fs::write(&path, json);
    }
}

/// Toggle the boolean field currently selected in `screen` and persist the
/// change. The mapping is: General tab fields 0,1,2 → auto_compact_enabled,
/// notifications_enabled, show_turn_duration; Display tab fields 0,1 →
/// reduce_motion, terminal_progress_bar.
pub fn toggle_current_field(screen: &mut SettingsScreen, _config: &mut Config) {
    match &screen.active_tab {
        SettingsTab::General => match screen.selected_field {
            0 => {
                screen.auto_compact_enabled = !screen.auto_compact_enabled;
                save_setting_bool("autoCompact", screen.auto_compact_enabled);
            }
            1 => {
                screen.notifications_enabled = !screen.notifications_enabled;
                save_setting_bool("notifications", screen.notifications_enabled);
            }
            2 => {
                screen.show_turn_duration = !screen.show_turn_duration;
                save_setting_bool("showTurnDuration", screen.show_turn_duration);
            }
            _ => {}
        },
        SettingsTab::Display => match screen.selected_field {
            0 => {
                screen.reduce_motion = !screen.reduce_motion;
                save_setting_bool("reduceMotion", screen.reduce_motion);
            }
            1 => {
                screen.terminal_progress_bar = !screen.terminal_progress_bar;
                save_setting_bool("terminalProgressBar", screen.terminal_progress_bar);
            }
            _ => {}
        },
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the settings screen (full-screen popup) into `frame`.
pub fn render_settings_screen(frame: &mut Frame, screen: &SettingsScreen, area: Rect) {
    if !screen.visible {
        return;
    }

    render_dark_overlay(frame, area);

    // 80% width, 90% height, centred
    let w = (area.width * 4 / 5)
        .max(60)
        .min(area.width.saturating_sub(2));
    let h = (area.height * 9 / 10)
        .max(20)
        .min(area.height.saturating_sub(2));
    let popup = centered_rect(w, h, area);
    render_dialog_bg(frame, popup);

    // Inset inner area
    let inner = Rect {
        x: popup.x + 2,
        y: popup.y + 1,
        width: popup.width.saturating_sub(4),
        height: popup.height.saturating_sub(2),
    };

    if inner.height < 6 {
        return;
    }

    // Split into header + tabs + content + footer
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let header_area = layout[0];
    let tabs_area = layout[1];
    let content_area = layout[2];
    let footer_area = layout[3];

    let title = Line::from(vec![
        Span::styled(
            " Settings",
            Style::default()
                .fg(CLAURST_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" — Claurst", Style::default().fg(CLAURST_MUTED)),
        Span::styled(
            format!(
                "{:>width$}",
                "Esc close",
                width = inner.width.saturating_sub(19) as usize
            ),
            Style::default().fg(CLAURST_MUTED),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(title).style(Style::default().bg(CLAURST_PANEL_BG)),
        header_area,
    );

    // Tabs bar
    let tab_labels: Vec<Line> = SettingsTab::all()
        .iter()
        .map(|t| {
            if *t == screen.active_tab {
                Line::from(vec![Span::styled(
                    format!(" {} ", t.label()),
                    Style::default()
                        .fg(Color::Black)
                        .bg(CLAURST_ACCENT)
                        .add_modifier(Modifier::BOLD),
                )])
            } else {
                Line::from(vec![Span::styled(
                    format!(" {} ", t.label()),
                    Style::default().fg(CLAURST_MUTED),
                )])
            }
        })
        .collect();

    let tabs = Tabs::new(tab_labels)
        .divider(Span::styled("  ", Style::default().fg(CLAURST_MUTED)))
        .style(Style::default().fg(CLAURST_MUTED).bg(CLAURST_PANEL_BG));
    frame.render_widget(tabs, tabs_area);

    // Tab content
    render_tab_content(frame, screen, content_area);

    // Footer
    let footer = if screen.edit_field.is_some() {
        Line::from(vec![
            Span::styled(
                " Enter ",
                Style::default()
                    .fg(CLAURST_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("save  "),
            Span::styled(
                " Esc ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("cancel"),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " Tab ",
                Style::default()
                    .fg(CLAURST_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("next tab  "),
            Span::styled(
                " ↑↓ ",
                Style::default()
                    .fg(CLAURST_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("select  "),
            Span::styled(
                " Space/Enter ",
                Style::default()
                    .fg(CLAURST_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("toggle  "),
            Span::styled(
                " Esc ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("close"),
        ])
    };
    let footer_para = Paragraph::new(vec![footer])
        .style(Style::default().fg(CLAURST_MUTED).bg(CLAURST_PANEL_BG))
        .alignment(Alignment::Center);
    frame.render_widget(footer_para, footer_area);
}

fn render_tab_content(frame: &mut Frame, screen: &SettingsScreen, area: Rect) {
    let lines = match &screen.active_tab {
        SettingsTab::General => build_general_lines(screen),
        SettingsTab::Display => build_display_lines(screen),
        SettingsTab::Privacy => build_privacy_lines(screen),
        SettingsTab::Advanced => build_advanced_lines(screen),
        SettingsTab::KeyBindings => build_keybindings_lines(screen),
    };

    let total = lines.len() as u16;
    let visible = area.height;
    let max_scroll = total.saturating_sub(visible);
    let scroll = screen.scroll_offset.min(max_scroll);

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(para, area);
}

// ---------------------------------------------------------------------------
// General tab
// ---------------------------------------------------------------------------

fn build_general_lines(screen: &SettingsScreen) -> Vec<Line<'static>> {
    let cfg = &screen.settings_snapshot.config;
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(section_header("General Settings"));
    lines.push(Line::from(""));

    // Model
    let model_val = cfg
        .model
        .clone()
        .unwrap_or_else(|| claurst_core::constants::DEFAULT_MODEL.to_string());
    lines.extend(field_lines(
        "model",
        "Model",
        &model_val,
        "AI model used for responses.",
        screen,
    ));
    // Show available models hint
    lines.push(indent_line(
        "  Available: claude-opus-4-6, claude-sonnet-4-6, claude-haiku-4-5-20251001",
        Color::DarkGray,
    ));
    lines.push(Line::from(""));

    // Max tokens
    let max_tokens_val = cfg
        .max_tokens
        .map(|n| n.to_string())
        .unwrap_or_else(|| claurst_core::constants::DEFAULT_MAX_TOKENS.to_string());
    lines.extend(field_lines(
        "max_tokens",
        "Max Tokens",
        &max_tokens_val,
        "Maximum tokens per response.",
        screen,
    ));
    lines.push(Line::from(""));

    // Output style
    let style_names: Vec<String> = builtin_styles().into_iter().map(|s| s.name).collect();
    let output_style_val = cfg
        .output_style
        .clone()
        .unwrap_or_else(|| "default".to_string());
    lines.extend(field_lines(
        "output_style",
        "Output Style",
        &output_style_val,
        "Controls the verbosity and format of responses.",
        screen,
    ));
    lines.push(indent_line(
        &format!("  Available: {}", style_names.join(", ")),
        Color::DarkGray,
    ));
    lines.push(Line::from(""));

    // Working directory
    let wd = cfg
        .project_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "(unknown)".to_string())
        });
    lines.push(label_value_line("Working Directory", &wd));
    lines.push(indent_line(
        "  (Set via --project-dir flag)",
        Color::DarkGray,
    ));
    lines.push(Line::from(""));

    // --- Toggleable fields ---

    lines.push(section_header("Behaviour"));
    lines.push(Line::from(""));

    // Field 0: Auto-compact
    lines.extend(toggle_field_lines(
        screen.auto_compact_enabled,
        "Auto-compact",
        &format!(
            "Automatically compact at {}%",
            screen.auto_compact_threshold
        ),
        screen.selected_field == 0,
    ));

    // Field 1: Notifications
    lines.extend(toggle_field_lines(
        screen.notifications_enabled,
        "Desktop notifications",
        "Notify when turn completes",
        screen.selected_field == 1,
    ));

    // Field 2: Show turn duration
    lines.extend(toggle_field_lines(
        screen.show_turn_duration,
        "Show turn duration",
        "Display elapsed time per turn",
        screen.selected_field == 2,
    ));

    lines
}

// ---------------------------------------------------------------------------
// Display tab
// ---------------------------------------------------------------------------

fn build_display_lines(screen: &SettingsScreen) -> Vec<Line<'static>> {
    let cfg = &screen.settings_snapshot.config;
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(section_header("Display Settings"));
    lines.push(Line::from(""));

    // Theme
    let theme_name = match &cfg.theme {
        claurst_core::config::Theme::Default => "default",
        claurst_core::config::Theme::Dark => "dark",
        claurst_core::config::Theme::Light => "light",
        claurst_core::config::Theme::Deuteranopia => "deuteranopia",
        claurst_core::config::Theme::Custom(s) => s.as_str(),
    };
    lines.push(label_value_line("Theme", theme_name));
    lines.push(indent_line(
        "  Options: default, dark, light, deuteranopia  (use /theme to change)",
        Color::DarkGray,
    ));
    lines.push(Line::from(""));

    // Output format
    let fmt = match &cfg.output_format {
        claurst_core::config::OutputFormat::Text => "text",
        claurst_core::config::OutputFormat::Json => "json",
        claurst_core::config::OutputFormat::StreamJson => "stream-json",
    };
    lines.push(label_value_line("Output Format", fmt));
    lines.push(indent_line(
        "  Options: text, json, stream-json",
        Color::DarkGray,
    ));
    lines.push(Line::from(""));

    // Verbose
    let verbose = if cfg.verbose { "yes" } else { "no" };
    lines.push(label_value_line("Verbose Mode", verbose));
    lines.push(indent_line(
        "  Shows additional debug information during queries.",
        Color::DarkGray,
    ));
    lines.push(Line::from(""));

    // --- Toggleable fields ---

    lines.push(section_header("UI Options"));
    lines.push(Line::from(""));

    // Field 0: Reduce motion
    lines.extend(toggle_field_lines(
        screen.reduce_motion,
        "Reduce motion",
        "Disable UI animations",
        screen.selected_field == 0,
    ));

    // Field 1: Terminal progress bar
    lines.extend(toggle_field_lines(
        screen.terminal_progress_bar,
        "Terminal progress bar",
        "Show progress during tool use",
        screen.selected_field == 1,
    ));

    // Output styles section
    lines.push(section_header("Available Output Styles"));
    lines.push(Line::from(""));
    for style in builtin_styles() {
        let active = cfg.output_style.as_deref() == Some(&style.name)
            || (cfg.output_style.is_none() && style.name == "default");
        let marker = if active { " *" } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}  {:<15}", marker, style.name),
                Style::default()
                    .fg(if active { CLAURST_ACCENT } else { CLAURST_TEXT })
                    .add_modifier(if active {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(
                style.description.clone(),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    lines.push(Line::from(""));

    let _ = cfg; // suppress unused warning
    lines
}

// ---------------------------------------------------------------------------
// Privacy tab
// ---------------------------------------------------------------------------

/// Privacy-relevant fields parsed from the raw settings JSON.
#[derive(Debug, Clone, Default)]
struct PrivacySnapshot {
    /// `hasAgreedToUsagePolicy` from settings.json
    has_agreed: Option<bool>,
    /// `disableTelemetry` from settings.json
    disable_telemetry: Option<bool>,
    /// `shareUsageData` from settings.json (optional field)
    share_usage_data: Option<bool>,
}

impl PrivacySnapshot {
    /// Load privacy fields from `~/.claurst/settings.json`.
    fn load() -> Self {
        let path = claurst_core::config::Settings::config_dir().join("settings.json");
        let Ok(content) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
            return Self::default();
        };
        Self {
            has_agreed: json.get("hasAgreedToUsagePolicy").and_then(|v| v.as_bool()),
            disable_telemetry: json.get("disableTelemetry").and_then(|v| v.as_bool()),
            share_usage_data: json.get("shareUsageData").and_then(|v| v.as_bool()),
        }
    }

    fn telemetry_enabled(&self) -> bool {
        !self.disable_telemetry.unwrap_or(false)
    }

    fn usage_sharing_enabled(&self) -> bool {
        self.share_usage_data.unwrap_or(false)
    }
}

fn build_privacy_lines(screen: &SettingsScreen) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let privacy = PrivacySnapshot::load();

    lines.push(section_header("Privacy Settings"));
    lines.push(Line::from(""));

    lines.push(Line::from(vec![Span::styled(
        "  These settings control data sharing with Anthropic.",
        Style::default().fg(Color::DarkGray),
    )]));
    lines.push(Line::from(""));

    // Usage policy agreement
    let agreed_label = match privacy.has_agreed {
        Some(true) => "Agreed",
        Some(false) => "Not agreed",
        None => "Unknown (field absent)",
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:<25}", "Usage Policy"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            agreed_label.to_string(),
            Style::default().fg(if privacy.has_agreed == Some(true) {
                Color::Green
            } else {
                Color::Yellow
            }),
        ),
    ]));
    lines.push(indent_line(
        "  Whether you have agreed to Anthropic's usage policy.",
        Color::DarkGray,
    ));
    lines.push(Line::from(""));

    // Telemetry
    privacy_toggle_lines(
        &mut lines,
        "Telemetry",
        privacy.telemetry_enabled(),
        "Sends anonymised usage statistics to help improve Claurst.",
    );

    // Usage sharing
    privacy_toggle_lines(
        &mut lines,
        "Usage Sharing",
        privacy.usage_sharing_enabled(),
        "Shares aggregate usage data for product improvement.",
    );

    // Verbose (local debug logging)
    privacy_toggle_lines(
        &mut lines,
        "Verbose Logging",
        screen.settings_snapshot.config.verbose,
        "Logs additional debug information locally (--verbose flag).",
    );

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "  Note: Edit ~/.claurst/settings.json to toggle telemetry/sharing values.",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::ITALIC),
    )]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "  For full privacy policy see: https://www.anthropic.com/privacy",
        Style::default().fg(Color::DarkGray),
    )]));

    lines
}

fn privacy_toggle_lines(lines: &mut Vec<Line<'static>>, name: &str, enabled: bool, desc: &str) {
    let (toggle_text, toggle_color) = if enabled {
        (" ON  ", Color::Green)
    } else {
        (" OFF ", Color::Red)
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:<25}", name),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("[{}]", toggle_text),
            Style::default()
                .fg(toggle_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(indent_line(&format!("  {}", desc), Color::DarkGray));
    lines.push(Line::from(""));
}

// ---------------------------------------------------------------------------
// Advanced tab
// ---------------------------------------------------------------------------

fn build_advanced_lines(screen: &SettingsScreen) -> Vec<Line<'static>> {
    let cfg = &screen.settings_snapshot.config;
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(section_header("Advanced Settings"));
    lines.push(Line::from(""));

    // API key source
    let key_source = if cfg.api_key.is_some() {
        "config file (masked)"
    } else if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        "environment variable (ANTHROPIC_API_KEY)"
    } else {
        "not set"
    };
    lines.push(label_value_line("API Key Source", key_source));
    if cfg.api_key.is_some() {
        lines.push(indent_line("  sk-ant-api03-***...***", Color::DarkGray));
    }
    lines.push(Line::from(""));

    // MCP Servers
    lines.push(section_header("MCP Servers"));
    lines.push(Line::from(""));
    if cfg.mcp_servers.is_empty() {
        lines.push(indent_line("  (none configured)", Color::DarkGray));
    } else {
        for srv in &cfg.mcp_servers {
            let kind = if srv.url.is_some() {
                "http"
            } else {
                &srv.server_type
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<20}", srv.name),
                    Style::default()
                        .fg(CLAURST_ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("[{}]", kind), Style::default().fg(Color::DarkGray)),
            ]));
            if let Some(cmd) = &srv.command {
                lines.push(indent_line(&format!("  cmd: {}", cmd), Color::DarkGray));
            }
            if let Some(url) = &srv.url {
                lines.push(indent_line(&format!("  url: {}", url), Color::DarkGray));
            }
        }
    }
    lines.push(Line::from(""));

    // Hooks
    lines.push(section_header("Configured Hooks"));
    lines.push(Line::from(""));
    if cfg.hooks.is_empty() {
        lines.push(indent_line("  (none configured)", Color::DarkGray));
    } else {
        for (event, entries) in &cfg.hooks {
            for entry in entries {
                let event_name = format!("{:?}", event);
                let filter = entry
                    .tool_filter
                    .as_deref()
                    .map(|f| format!("[{}]", f))
                    .unwrap_or_default();
                let blocking = if entry.blocking { " (blocking)" } else { "" };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {:<20}", event_name),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(filter, Style::default().fg(CLAURST_ACCENT)),
                    Span::styled(blocking.to_string(), Style::default().fg(Color::Red)),
                ]));
                lines.push(indent_line(
                    &format!("    cmd: {}", entry.command),
                    Color::DarkGray,
                ));
            }
        }
    }
    lines.push(Line::from(""));

    // Environment variables
    lines.push(section_header("Environment Variables"));
    lines.push(Line::from(""));
    if cfg.env.is_empty() {
        lines.push(indent_line("  (none configured)", Color::DarkGray));
    } else {
        for (key, _val) in &cfg.env {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<25}", key),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("= ***".to_string(), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }
    lines.push(Line::from(""));

    lines
}

// ---------------------------------------------------------------------------
// KeyBindings tab
// ---------------------------------------------------------------------------

fn build_keybindings_lines(_screen: &SettingsScreen) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(section_header("Key Bindings"));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "  Edit ~/.claurst/keybindings.json to customise bindings.",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::ITALIC),
    )]));
    lines.push(Line::from(""));

    // Group bindings by context
    let mut by_context: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();

    for binding in default_bindings() {
        if let Some(action) = &binding.action {
            let ctx_name = format!("{:?}", binding.context);
            let chord_str = binding
                .chord
                .iter()
                .map(|ks| {
                    let mut parts = Vec::new();
                    if ks.ctrl {
                        parts.push("Ctrl");
                    }
                    if ks.alt {
                        parts.push("Alt");
                    }
                    if ks.shift {
                        parts.push("Shift");
                    }
                    parts.push(ks.key.as_str());
                    parts.join("+")
                })
                .collect::<Vec<_>>()
                .join(" ");
            by_context
                .entry(ctx_name)
                .or_default()
                .push((chord_str, action.clone()));
        }
    }

    // Render in sorted context order
    let mut contexts: Vec<String> = by_context.keys().cloned().collect();
    contexts.sort();

    // Ensure Global and Chat come first
    contexts.retain(|c| c != "Global" && c != "Chat");
    let mut ordered = vec!["Global".to_string(), "Chat".to_string()];
    ordered.extend(contexts);

    for ctx in &ordered {
        if let Some(entries) = by_context.get(ctx) {
            lines.push(Line::from(vec![Span::styled(
                format!("  {} Context", ctx),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )]));
            lines.push(Line::from(""));
            for (chord, action) in entries {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(
                        format!("{:<25}", chord),
                        Style::default()
                            .fg(CLAURST_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(action.clone(), Style::default().fg(Color::White)),
                ]));
            }
            lines.push(Line::from(""));
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn section_header(title: &str) -> Line<'static> {
    Line::from(vec![Span::styled(
        format!("  {}", title),
        Style::default()
            .fg(CLAURST_ACCENT)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    )])
}

fn label_value_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {:<25}", label),
            Style::default()
                .fg(CLAURST_TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_string(), Style::default().fg(CLAURST_ACCENT)),
    ])
}

fn indent_line(text: &str, color: Color) -> Line<'static> {
    Line::from(vec![Span::styled(
        text.to_string(),
        Style::default().fg(color),
    )])
}

/// Build a pair of display lines for a boolean toggle field.
///
/// Format:
///   `[✓] Label    Description`  (selected → highlighted row)
///   `[○] Label    Description`  (unchecked)
fn toggle_field_lines(
    enabled: bool,
    label: &str,
    description: &str,
    selected: bool,
) -> Vec<Line<'static>> {
    let (check_char, check_color) = if enabled {
        ("✓", Color::Green)
    } else {
        ("○", Color::DarkGray)
    };

    let row_style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(CLAURST_ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let line = Line::from(vec![
        Span::styled(
            format!("  [{}] {:<26}", check_char, label),
            if selected {
                row_style
                    .fg(Color::Black)
                    .bg(CLAURST_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(if enabled {
                        CLAURST_TEXT
                    } else {
                        Color::DarkGray
                    })
                    .add_modifier(if enabled {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    })
            },
        ),
        Span::styled(check_char.to_string(), Style::default().fg(check_color)),
        // Overwrite the duplicated check_char — we embedded it above; use the
        // description as the right-hand column instead.
        Span::styled(
            format!("  {}", description),
            if selected {
                Style::default().fg(Color::Black).bg(CLAURST_ACCENT)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
    ]);

    // Build it cleanly without the accidental duplicate span
    let clean_line = Line::from(vec![
        Span::styled(
            format!("  [{}] {:<26}", check_char, label),
            if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(CLAURST_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(if enabled {
                        CLAURST_TEXT
                    } else {
                        Color::DarkGray
                    })
                    .add_modifier(if enabled {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    })
            },
        ),
        Span::styled(
            format!("  {}", description),
            if selected {
                Style::default().fg(Color::Black).bg(CLAURST_ACCENT)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
    ]);

    let _ = line; // discard the draft with the duplicate
    vec![clean_line, Line::from("")]
}

/// Build display lines for an editable field.
fn field_lines(
    field_key: &str,
    label: &str,
    current_value: &str,
    description: &str,
    screen: &SettingsScreen,
) -> Vec<Line<'static>> {
    let is_editing = screen.edit_field.as_deref() == Some(field_key);
    let has_pending = screen.pending_changes.contains_key(field_key);

    let display_value = if is_editing {
        format!("{}_", screen.edit_value)
    } else if let Some(pending) = screen.pending_changes.get(field_key) {
        format!("{} (unsaved)", pending)
    } else {
        current_value.to_string()
    };

    let value_color = if is_editing {
        Color::Yellow
    } else if has_pending {
        Color::Magenta
    } else {
        CLAURST_ACCENT
    };

    let edit_hint = if is_editing {
        " [editing]"
    } else {
        " [Enter to edit]"
    };

    vec![
        Line::from(vec![
            Span::styled(
                format!("  {:<25}", label),
                Style::default()
                    .fg(CLAURST_TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(display_value, Style::default().fg(value_color)),
            Span::styled(
                edit_hint.to_string(),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]),
        Line::from(vec![Span::styled(
            format!("    {}", description),
            Style::default().fg(Color::DarkGray),
        )]),
    ]
}

// ---------------------------------------------------------------------------
// Key handling helpers (called from app.rs)
// ---------------------------------------------------------------------------

/// Returns `true` if the key event was consumed by the settings screen.
pub fn handle_settings_key(
    screen: &mut SettingsScreen,
    config: &mut Config,
    key: crossterm::event::KeyEvent,
) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};

    if !screen.visible {
        return false;
    }

    // Editing mode
    if screen.edit_field.is_some() {
        match key.code {
            KeyCode::Enter => {
                screen.commit_edit();
                screen.apply_and_save(config);
            }
            KeyCode::Esc => {
                screen.cancel_edit();
            }
            KeyCode::Backspace => {
                screen.edit_value.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                screen.edit_value.push(c);
            }
            _ => {}
        }
        return true;
    }

    // Navigation mode
    match key.code {
        KeyCode::Esc => {
            screen.close();
        }
        KeyCode::Tab => {
            screen.next_tab();
        }
        KeyCode::BackTab => {
            screen.prev_tab();
        }
        KeyCode::Up => {
            screen.scroll_up();
        }
        KeyCode::Down => {
            screen.scroll_down();
        }
        KeyCode::PageUp => {
            for _ in 0..10 {
                screen.scroll_up();
            }
        }
        KeyCode::PageDown => {
            for _ in 0..10 {
                screen.scroll_down();
            }
        }
        // Space toggles the currently-selected boolean field
        KeyCode::Char(' ') => {
            toggle_current_field(screen, config);
        }
        KeyCode::Enter => {
            // For tabs with interactive boolean fields, Enter toggles.
            // For General tab with no field selected (selected_field beyond
            // boolean range), fall through to text editing.
            let has_toggle = matches!(
                &screen.active_tab,
                SettingsTab::General | SettingsTab::Display
            ) && screen.selected_field < screen.max_fields_for_tab();

            if has_toggle {
                toggle_current_field(screen, config);
            } else {
                // Start editing the first editable text field
                match &screen.active_tab {
                    SettingsTab::General => {
                        let cfg = &screen.settings_snapshot.config;
                        let model_val = cfg
                            .model
                            .clone()
                            .unwrap_or_else(|| claurst_core::constants::DEFAULT_MODEL.to_string());
                        screen.start_edit("model", &model_val);
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // Helper: create a temporary settings.json with a given JSON body.
    #[allow(dead_code)]
    fn write_temp_settings(json: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("tempfile");
        f.write_all(json.as_bytes()).expect("write");
        f
    }

    // ---------------------------------------------------------------------------
    // read_setting_bool tests
    // ---------------------------------------------------------------------------

    #[test]
    fn read_setting_bool_returns_default_when_file_missing() {
        // Point config_dir at a non-existent path by using a temp dir that has
        // no settings.json. Because read_setting_bool internally calls
        // Settings::config_dir() we can't easily redirect it in a unit test
        // without modifying the helper signature; instead, we exercise the
        // fallback branch directly by using a fabricated Settings and relying on
        // the fact that a random temp dir won't have settings.json.
        //
        // This test simply asserts the default is honoured when parsing fails.
        let settings = Settings::default();
        // Call with a key that definitely doesn't exist in the (absent) file.
        let result = read_setting_bool(&settings, "thisKeyDoesNotExist_xyzzy", true);
        // We can't guarantee the file doesn't exist on the test machine, but we
        // can guarantee that a missing key returns the default.
        // If the file exists but lacks the key: returns true.
        // If the file is absent: returns true.
        assert!(result, "default should be returned for missing key");
    }

    #[test]
    fn read_setting_bool_false_default_when_key_absent() {
        let settings = Settings::default();
        let result = read_setting_bool(&settings, "anotherMissingKey_abc123", false);
        assert!(!result);
    }

    // ---------------------------------------------------------------------------
    // SettingsScreen::new() defaults
    // ---------------------------------------------------------------------------

    #[test]
    fn settings_screen_new_has_sensible_defaults() {
        let screen = SettingsScreen::new();
        assert!(!screen.visible, "should not be visible on creation");
        assert_eq!(screen.active_tab, SettingsTab::General);
        assert_eq!(screen.scroll_offset, 0);
        assert_eq!(screen.selected_field, 0);
        assert!(screen.edit_field.is_none());
        assert!(screen.edit_value.is_empty());
        assert!(screen.pending_changes.is_empty());
        // Boolean defaults
        assert!(screen.notifications_enabled, "notifications default on");
        assert!(!screen.reduce_motion, "reduce_motion default off");
        assert!(!screen.show_turn_duration, "show_turn_duration default off");
        assert!(
            screen.terminal_progress_bar,
            "terminal_progress_bar default on"
        );
    }

    #[test]
    fn settings_screen_new_auto_compact_threshold_sensible() {
        let screen = SettingsScreen::new();
        // Threshold must be in 0-100 range
        assert!(
            screen.auto_compact_threshold <= 100,
            "threshold {} out of range",
            screen.auto_compact_threshold
        );
    }

    // ---------------------------------------------------------------------------
    // toggle_current_field tests
    // ---------------------------------------------------------------------------

    #[test]
    fn toggle_current_field_general_field0_flips_auto_compact() {
        let mut screen = SettingsScreen::new();
        let mut config = Config::default();
        screen.active_tab = SettingsTab::General;
        screen.selected_field = 0;

        let before = screen.auto_compact_enabled;
        toggle_current_field(&mut screen, &mut config);
        assert_eq!(
            screen.auto_compact_enabled, !before,
            "auto_compact_enabled should have flipped"
        );
        // Toggle back
        toggle_current_field(&mut screen, &mut config);
        assert_eq!(screen.auto_compact_enabled, before);
    }

    #[test]
    fn toggle_current_field_general_field1_flips_notifications() {
        let mut screen = SettingsScreen::new();
        let mut config = Config::default();
        screen.active_tab = SettingsTab::General;
        screen.selected_field = 1;

        let before = screen.notifications_enabled;
        toggle_current_field(&mut screen, &mut config);
        assert_eq!(screen.notifications_enabled, !before);
    }

    #[test]
    fn toggle_current_field_display_field0_flips_reduce_motion() {
        let mut screen = SettingsScreen::new();
        let mut config = Config::default();
        screen.active_tab = SettingsTab::Display;
        screen.selected_field = 0;

        let before = screen.reduce_motion;
        toggle_current_field(&mut screen, &mut config);
        assert_eq!(screen.reduce_motion, !before);
    }

    #[test]
    fn toggle_current_field_display_field1_flips_terminal_progress_bar() {
        let mut screen = SettingsScreen::new();
        let mut config = Config::default();
        screen.active_tab = SettingsTab::Display;
        screen.selected_field = 1;

        let before = screen.terminal_progress_bar;
        toggle_current_field(&mut screen, &mut config);
        assert_eq!(screen.terminal_progress_bar, !before);
    }

    // ---------------------------------------------------------------------------
    // General tab render includes auto-compact row
    // ---------------------------------------------------------------------------

    #[test]
    fn general_tab_contains_auto_compact_row() {
        let mut screen = SettingsScreen::new();
        screen.auto_compact_enabled = true;
        screen.auto_compact_threshold = 95;

        let lines = build_general_lines(&screen);

        // Flatten all spans to a single string for easy assertion
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();

        assert!(
            text.contains("Auto-compact"),
            "General tab should render an Auto-compact row; got: {}",
            &text[..text.len().min(300)]
        );
        assert!(
            text.contains("95%"),
            "General tab should show threshold percentage"
        );
    }

    #[test]
    fn general_tab_contains_notifications_row() {
        let screen = SettingsScreen::new();
        let lines = build_general_lines(&screen);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            text.contains("Desktop notifications"),
            "General tab should render a Notifications row"
        );
    }

    #[test]
    fn display_tab_contains_reduce_motion_and_progress_bar_rows() {
        let screen = SettingsScreen::new();
        let lines = build_display_lines(&screen);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            text.contains("Reduce motion"),
            "Display tab should have Reduce motion row"
        );
        assert!(
            text.contains("Terminal progress bar"),
            "Display tab should have Terminal progress bar row"
        );
    }

    // ---------------------------------------------------------------------------
    // max_fields_for_tab sanity
    // ---------------------------------------------------------------------------

    #[test]
    fn max_fields_for_tab_returns_correct_counts() {
        let mut screen = SettingsScreen::new();

        screen.active_tab = SettingsTab::General;
        assert_eq!(screen.max_fields_for_tab(), 3);

        screen.active_tab = SettingsTab::Display;
        assert_eq!(screen.max_fields_for_tab(), 2);

        screen.active_tab = SettingsTab::Privacy;
        assert_eq!(screen.max_fields_for_tab(), 0);

        screen.active_tab = SettingsTab::Advanced;
        assert_eq!(screen.max_fields_for_tab(), 0);

        screen.active_tab = SettingsTab::KeyBindings;
        assert_eq!(screen.max_fields_for_tab(), 0);
    }
}
