// overage_upsell.rs — OverageCreditUpsell surface.
//
// Shown when the user has exceeded their free-tier token allowance and needs
// to add credits to continue. Rendered as a dismissable banner at the top of
// the message area.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Overage credit upsell banner state.
#[derive(Debug, Clone, Default)]
pub struct OverageCreditUpsellState {
    /// Whether the banner is visible.
    pub visible: bool,
    /// Overage amount in USD cents (e.g. 5 = $0.05).
    pub overage_cents: u32,
    /// The credit pack URL shown in the banner.
    pub credits_url: String,
    /// Whether the user has dismissed this banner.
    dismissed: bool,
}

impl OverageCreditUpsellState {
    pub fn new() -> Self {
        Self {
            credits_url: "https://claude.ai/settings/billing".to_string(),
            ..Default::default()
        }
    }

    /// Show the banner with the given overage amount.
    pub fn show(&mut self, overage_cents: u32) {
        if self.dismissed {
            return;
        }
        self.overage_cents = overage_cents;
        self.visible = true;
    }

    /// Dismiss the banner for this session.
    pub fn dismiss(&mut self) {
        self.visible = false;
        self.dismissed = true;
    }

    /// Height the banner occupies (0 if not visible).
    pub fn height(&self) -> u16 {
        if self.visible {
            4
        } else {
            0
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the overage credit upsell banner into `area`.
pub fn render_overage_upsell(state: &OverageCreditUpsellState, area: Rect, buf: &mut Buffer) {
    if !state.visible || area.height < 3 {
        return;
    }

    let banner_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: state.height().min(area.height),
    };

    Clear.render(banner_area, buf);

    let overage_dollars = state.overage_cents as f64 / 100.0;
    let title = format!(" Overage Alert — ${:.2} over limit ", overage_dollars);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(banner_area);
    block.render(banner_area, buf);

    let lines = vec![
        Line::from(vec![
            Span::styled(
                "  You've exceeded your usage limit. Add credits to continue: ",
                Style::default().fg(Color::White),
            ),
            Span::styled(
                state.credits_url.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::UNDERLINED),
            ),
        ]),
        Line::from(vec![Span::styled(
            "  Press Esc to dismiss this notice.",
            Style::default().fg(Color::DarkGray),
        )]),
    ];

    let para = Paragraph::new(lines);
    para.render(inner, buf);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn overage_upsell_show_hide() {
        let mut state = OverageCreditUpsellState::new();
        assert!(!state.visible);
        state.show(500); // $5.00 overage
        assert!(state.visible);
        assert_eq!(state.overage_cents, 500);
        state.dismiss();
        assert!(!state.visible);
        // Once dismissed, show should not re-enable it
        state.show(100);
        assert!(!state.visible);
    }

    #[test]
    fn overage_upsell_height() {
        let mut state = OverageCreditUpsellState::new();
        assert_eq!(state.height(), 0);
        state.show(50);
        assert_eq!(state.height(), 4);
    }

    #[test]
    fn overage_upsell_render_smoke() {
        let mut state = OverageCreditUpsellState::new();
        state.show(250);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 6,
        };
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_overage_upsell(&state, area, &mut buf);
        let rendered = buf
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(rendered.contains("Overage Alert"));
        assert!(rendered.contains("credits"));
    }

    #[test]
    fn overage_upsell_not_rendered_when_invisible() {
        let state = OverageCreditUpsellState::new();
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 6,
        };
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_overage_upsell(&state, area, &mut buf);
        let rendered = buf
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(!rendered.contains("Overage"));
    }
}
