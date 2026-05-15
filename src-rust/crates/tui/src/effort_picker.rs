// effort_picker.rs — Small modal picker for /effort command.
//
// Replaces the prior text-only `/effort` status message with an interactive
// 4-row select dialog (issue #149 follow-up).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::model_picker::EffortLevel;
use crate::overlays::centered_rect;

#[derive(Debug, Default, Clone)]
pub struct EffortPickerState {
    pub visible: bool,
    pub selected: usize, // 0..=3
}

impl EffortPickerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, current: EffortLevel) {
        self.visible = true;
        self.selected = match current {
            EffortLevel::Low => 0,
            EffortLevel::Normal => 1,
            EffortLevel::High => 2,
            EffortLevel::Max => 3,
        };
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn select_prev(&mut self) {
        self.selected = if self.selected == 0 { 3 } else { self.selected - 1 };
    }

    pub fn select_next(&mut self) {
        self.selected = (self.selected + 1) % 4;
    }

    pub fn current(&self) -> EffortLevel {
        match self.selected {
            0 => EffortLevel::Low,
            1 => EffortLevel::Normal,
            2 => EffortLevel::High,
            _ => EffortLevel::Max,
        }
    }
}

pub fn render_effort_picker(frame: &mut Frame, state: &EffortPickerState, area: Rect) {
    if !state.visible {
        return;
    }

    let w = 44u16.min(area.width.saturating_sub(4));
    let h = 11u16.min(area.height.saturating_sub(4));
    let dlg = centered_rect(w, h, area);

    frame.render_widget(Clear, dlg);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta))
        .title(Span::styled(
            " Effort level ",
            Style::default().add_modifier(Modifier::BOLD),
        ));

    let mut lines: Vec<Line> = Vec::new();
    let options: [(EffortLevel, &str); 4] = [
        (EffortLevel::Low,    "low"),
        (EffortLevel::Normal, "normal"),
        (EffortLevel::High,   "high"),
        (EffortLevel::Max,    "max"),
    ];
    for (i, (lvl, label)) in options.iter().enumerate() {
        let selected = i == state.selected;
        let prefix = if selected { "›" } else { " " };
        let style = if selected {
            Style::default().fg(Color::Black).bg(Color::Magenta).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", prefix), style),
            Span::styled(format!("{}  {}", lvl.symbol(), label), style),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  ↑/↓ to choose · Enter to apply · Esc to cancel",
        Style::default().fg(Color::DarkGray),
    )));

    frame.render_widget(Paragraph::new(lines).block(block), dlg);
}
