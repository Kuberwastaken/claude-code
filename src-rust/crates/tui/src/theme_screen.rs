// theme_screen.rs — Theme picker overlay opened by /theme.
//
// Shows a list of available themes with colour swatches. Arrow keys navigate,
// Enter selects, Esc cancels.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::overlays::{
    begin_modal_frame, modal_header_line_area, render_modal_title_frame, CLAURST_ACCENT,
    CLAURST_MUTED, CLAURST_PANEL_BG, CLAURST_TEXT,
};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single theme option shown in the picker.
#[derive(Debug, Clone)]
pub struct ThemeOption {
    pub name: String,
    pub label: String,
    pub description: String,
    /// A few representative colours used for the swatch preview.
    pub swatch: [Color; 4],
}

pub struct ThemeScreen {
    pub visible: bool,
    pub themes: Vec<ThemeOption>,
    pub selected_idx: usize,
}

impl ThemeScreen {
    pub fn new() -> Self {
        Self {
            visible: false,
            themes: builtin_themes(),
            selected_idx: 0,
        }
    }

    pub fn open(&mut self, current_theme: &str) {
        self.visible = true;
        // Select the current theme, if found
        if let Some(idx) = self.themes.iter().position(|t| t.name == current_theme) {
            self.selected_idx = idx;
        } else {
            self.selected_idx = 0;
        }
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn select_prev(&mut self) {
        let count = self.themes.len();
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
        let count = self.themes.len();
        if count == 0 {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % count;
    }

    /// Return the name of the currently selected theme.
    pub fn selected_name(&self) -> Option<&str> {
        self.themes.get(self.selected_idx).map(|t| t.name.as_str())
    }
}

impl Default for ThemeScreen {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Built-in themes
// ---------------------------------------------------------------------------

fn builtin_themes() -> Vec<ThemeOption> {
    vec![
        ThemeOption {
            name: "default".to_string(),
            label: "Default".to_string(),
            description: "Claurst default — dark background, cyan accents".to_string(),
            swatch: [Color::Black, Color::Cyan, Color::Green, Color::White],
        },
        ThemeOption {
            name: "dark".to_string(),
            label: "Dark".to_string(),
            description: "High-contrast dark theme".to_string(),
            swatch: [
                Color::Rgb(18, 18, 18),
                Color::Rgb(97, 175, 239),
                Color::Rgb(152, 195, 121),
                Color::Rgb(229, 229, 229),
            ],
        },
        ThemeOption {
            name: "light".to_string(),
            label: "Light".to_string(),
            description: "Light background with dark text".to_string(),
            swatch: [Color::White, Color::Blue, Color::DarkGray, Color::Black],
        },
        ThemeOption {
            name: "solarized".to_string(),
            label: "Solarized".to_string(),
            description: "Solarized Dark — warm tones with blue accents".to_string(),
            swatch: [
                Color::Rgb(0, 43, 54),
                Color::Rgb(38, 139, 210),
                Color::Rgb(133, 153, 0),
                Color::Rgb(131, 148, 150),
            ],
        },
        ThemeOption {
            name: "nord".to_string(),
            label: "Nord".to_string(),
            description: "Nord — cool blue-grey palette".to_string(),
            swatch: [
                Color::Rgb(46, 52, 64),
                Color::Rgb(136, 192, 208),
                Color::Rgb(163, 190, 140),
                Color::Rgb(216, 222, 233),
            ],
        },
        ThemeOption {
            name: "dracula".to_string(),
            label: "Dracula".to_string(),
            description: "Dracula — purple/pink dark theme".to_string(),
            swatch: [
                Color::Rgb(40, 42, 54),
                Color::Rgb(139, 233, 253),
                Color::Rgb(80, 250, 123),
                Color::Rgb(248, 248, 242),
            ],
        },
        ThemeOption {
            name: "monokai".to_string(),
            label: "Monokai".to_string(),
            description: "Monokai — vibrant colours on dark background".to_string(),
            swatch: [
                Color::Rgb(39, 40, 34),
                Color::Rgb(102, 217, 239),
                Color::Rgb(166, 226, 46),
                Color::Rgb(248, 248, 242),
            ],
        },
        ThemeOption {
            name: "deuteranopia".to_string(),
            label: "Deuteranopia".to_string(),
            description: "Red-green color blind friendly — blue/yellow/gray palette".to_string(),
            swatch: [
                Color::Rgb(18, 18, 18),
                Color::Rgb(0, 122, 204),   // Blue
                Color::Rgb(255, 180, 0),   // Gold/Yellow
                Color::Rgb(200, 200, 200), // Light gray
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the theme picker overlay into `frame`.
pub fn render_theme_screen(frame: &mut Frame, screen: &ThemeScreen, area: Rect) {
    if !screen.visible {
        return;
    }

    let rows = (screen.themes.len() as u16 + 2).min(area.height.saturating_sub(6));
    let layout = begin_modal_frame(frame, area, 70, rows + 6, 2, 1);
    render_modal_title_frame(frame, layout.header_area, "Choose a theme", "esc");
    if let Some(subtitle_area) = modal_header_line_area(layout.header_area, 1) {
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                " Preview palettes before wiring up richer theme behavior.",
                Style::default().fg(CLAURST_MUTED),
            )])),
            subtitle_area,
        );
    }

    let mut lines: Vec<Line> = Vec::new();

    for (i, theme) in screen.themes.iter().enumerate() {
        let is_selected = i == screen.selected_idx;
        let bg = if is_selected {
            CLAURST_ACCENT
        } else {
            CLAURST_PANEL_BG
        };
        let fg = if is_selected {
            Color::White
        } else {
            CLAURST_TEXT
        };
        let desc_fg = if is_selected {
            Color::Rgb(248, 220, 236)
        } else {
            CLAURST_MUTED
        };

        // Build the swatch using block characters with background colour
        let swatch_spans: Vec<Span> = theme
            .swatch
            .iter()
            .map(|&c| Span::styled("  ", Style::default().bg(c)))
            .collect();

        let mut row_spans: Vec<Span> = Vec::new();
        row_spans.push(Span::styled(" ", Style::default().bg(bg)));
        row_spans.extend(swatch_spans);
        row_spans.push(Span::styled("  ", Style::default().bg(bg)));
        row_spans.push(Span::styled(
            format!("{:<12}", theme.label),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ));
        row_spans.push(Span::styled(
            theme.description.clone(),
            Style::default().fg(desc_fg).bg(bg),
        ));
        let used: usize = row_spans.iter().map(|span| span.content.len()).sum();
        let pad = layout.body_area.width.saturating_sub(used as u16) as usize;
        if pad > 0 {
            row_spans.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
        }

        lines.push(Line::from(row_spans));
        lines.push(Line::from(""));
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(CLAURST_PANEL_BG)),
        layout.body_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            " ↑↓ navigate  ·  enter apply  ·  esc cancel",
            Style::default()
                .fg(CLAURST_MUTED)
                .add_modifier(Modifier::ITALIC),
        )])),
        layout.footer_area,
    );
}

// ---------------------------------------------------------------------------
// Key handling helpers (called from app.rs)
// ---------------------------------------------------------------------------

/// Returns the selected theme name when the user confirms, `None` otherwise.
/// Call this from the app's key handler when `theme_screen.visible`.
pub fn handle_theme_key(
    screen: &mut ThemeScreen,
    key: crossterm::event::KeyEvent,
) -> Option<String> {
    use crossterm::event::KeyCode;

    if !screen.visible {
        return None;
    }

    match key.code {
        KeyCode::Esc => {
            screen.close();
            None
        }
        KeyCode::Enter => {
            let name = screen.selected_name().map(String::from);
            screen.close();
            name
        }
        KeyCode::Up => {
            screen.select_prev();
            None
        }
        KeyCode::Down => {
            screen.select_next();
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn theme_screen_renders_current_theme() {
        let mut screen = ThemeScreen::new();
        screen.open("dark");

        let backend = TestBackend::new(90, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_theme_screen(frame, &screen, frame.area()))
            .unwrap();

        let rendered = terminal.backend().buffer();
        let content = rendered
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(content.contains("Choose a theme"));
        assert!(content.contains("Dark"));
    }

    #[test]
    fn theme_navigation_wraps() {
        let mut screen = ThemeScreen::new();
        screen.open("default");

        screen.select_prev();
        assert_eq!(screen.selected_name(), Some("deuteranopia"));

        screen.select_next();
        assert_eq!(screen.selected_name(), Some("default"));
    }
}
