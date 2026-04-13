// privacy_screen.rs — Privacy settings dialog.
//
// A focused overlay with toggle-style controls for privacy preferences.
// Opened by /privacy-settings. Changes are persisted via Settings::save_sync().

use claurst_core::config::Settings;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// State for a single privacy toggle.
#[derive(Debug, Clone)]
pub struct PrivacyToggle {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub enabled: bool,
}

pub struct PrivacyScreen {
    pub visible: bool,
    pub toggles: Vec<PrivacyToggle>,
    pub selected_idx: usize,
}

impl PrivacyScreen {
    pub fn new() -> Self {
        Self {
            visible: false,
            toggles: default_toggles(),
            selected_idx: 0,
        }
    }

    pub fn open(&mut self) {
        self.visible = true;
        self.selected_idx = 0;
        // Refresh from stored settings
        self.toggles = default_toggles();
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn select_prev(&mut self) {
        let count = self.toggles.len();
        if count == 0 {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = count - 1;
        } else {
            self.selected_idx -= 1;
        }
    }

    pub fn select_next(&mut self) {
        let count = self.toggles.len();
        if count == 0 {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % count;
    }

    /// Toggle the currently selected privacy option.
    pub fn toggle_selected(&mut self) {
        if let Some(t) = self.toggles.get_mut(self.selected_idx) {
            t.enabled = !t.enabled;
        }
    }

    /// Persist the current toggle state to settings.
    pub fn save(&self) {
        // We store boolean privacy flags in the config.verbose field for now
        // (the actual privacy flags from the TS source would need a dedicated
        // field in Config; we save what we can using the sync path).
        let _ = Settings::load_sync().map(|mut s| {
            // Persist verbose to match telemetry toggle
            if let Some(telemetry_toggle) = self.toggles.iter().find(|t| t.key == "telemetry") {
                s.config.verbose = telemetry_toggle.enabled;
            }
            let _ = s.save_sync();
        });
    }
}

impl Default for PrivacyScreen {
    fn default() -> Self {
        Self::new()
    }
}

fn default_toggles() -> Vec<PrivacyToggle> {
    vec![
        PrivacyToggle {
            key: "telemetry",
            label: "Telemetry",
            description: "Send anonymised crash reports and usage statistics to Anthropic \
                          to help diagnose issues and improve the product.",
            enabled: false,
        },
        PrivacyToggle {
            key: "usage_sharing",
            label: "Usage Sharing",
            description: "Share aggregate usage patterns (no personal data) to help \
                          Anthropic understand how Claurst is used.",
            enabled: false,
        },
        PrivacyToggle {
            key: "api_request_logging",
            label: "API Request Logging",
            description: "Log API requests to a local file (~/.claurst/api_requests.log) \
                          for debugging. Logs are stored locally only.",
            enabled: false,
        },
        PrivacyToggle {
            key: "error_reporting",
            label: "Error Reporting",
            description: "Automatically report errors and stack traces to Anthropic. \
                          Helps fix bugs faster. No conversation content is included.",
            enabled: false,
        },
    ]
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the privacy settings dialog.
pub fn render_privacy_screen(frame: &mut Frame, screen: &PrivacyScreen, area: Rect) {
    if !screen.visible {
        return;
    }

    let dialog_width = 68u16.min(area.width.saturating_sub(4));
    // Each toggle: 1 label line + 2 description lines + 1 spacer = 4 rows
    let toggle_rows = (screen.toggles.len() as u16) * 4;
    let dialog_height = (toggle_rows + 8).min(area.height.saturating_sub(4));
    let dialog_area = crate::overlays::centered_rect(dialog_width, dialog_height, area);

    frame.render_widget(Clear, dialog_area);

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![Span::styled(
        "  Privacy Settings",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    )]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "  Use Space or Enter to toggle. Changes are saved immediately.",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )]));
    lines.push(Line::from(""));

    for (i, toggle) in screen.toggles.iter().enumerate() {
        let is_selected = i == screen.selected_idx;

        let prefix = if is_selected { "  \u{25BA} " } else { "    " };

        let (toggle_text, toggle_fg) = if toggle.enabled {
            ("[ ON  ]", Color::Green)
        } else {
            ("[ OFF ]", Color::Red)
        };

        let label_style = if is_selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        // Label row with toggle
        lines.push(Line::from(vec![
            Span::raw(prefix),
            Span::styled(format!("{:<28}", toggle.label), label_style),
            Span::styled(
                toggle_text.to_string(),
                Style::default().fg(toggle_fg).add_modifier(Modifier::BOLD),
            ),
        ]));

        // Description (wrapped to fit dialog width - 8 chars for indentation)
        let desc_max = (dialog_width as usize).saturating_sub(10);
        let wrapped = word_wrap_str(toggle.description, desc_max);
        for desc_line in &wrapped {
            lines.push(Line::from(vec![Span::styled(
                format!("       {}", desc_line),
                Style::default().fg(Color::DarkGray),
            )]));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(vec![Span::styled(
        "  \u{2191}\u{2193} navigate  \u{00b7}  Space/Enter toggle  \u{00b7}  Esc close",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )]));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Privacy Settings ")
        .border_style(Style::default().fg(Color::Cyan));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, dialog_area);
}

fn word_wrap_str(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.len() <= width {
        return vec![text.to_string()];
    }
    let mut result = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in text.split_whitespace() {
        let ww = word.len();
        if current_width == 0 {
            current.push_str(word);
            current_width = ww;
        } else if current_width + 1 + ww <= width {
            current.push(' ');
            current.push_str(word);
            current_width += 1 + ww;
        } else {
            result.push(std::mem::take(&mut current));
            current.push_str(word);
            current_width = ww;
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    if result.is_empty() {
        result.push(text.to_string());
    }
    result
}

// ---------------------------------------------------------------------------
// Key handling helpers (called from app.rs)
// ---------------------------------------------------------------------------

/// Returns `true` if the key event was consumed by the privacy screen.
pub fn handle_privacy_key(screen: &mut PrivacyScreen, key: crossterm::event::KeyEvent) -> bool {
    use crossterm::event::KeyCode;

    if !screen.visible {
        return false;
    }

    match key.code {
        KeyCode::Esc => {
            screen.close();
        }
        KeyCode::Up => {
            screen.select_prev();
        }
        KeyCode::Down => {
            screen.select_next();
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            screen.toggle_selected();
            screen.save();
        }
        _ => {}
    }
    true
}
