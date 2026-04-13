// context_viz.rs — Context window and rate-limit visualization overlay.
// Triggered by the /context command. Shows horizontal progress bars.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::Frame;

use crate::overlays::{
    begin_modal_frame, modal_header_line_area, render_modal_title_frame, CLAURST_ACCENT,
    CLAURST_MUTED, CLAURST_PANEL_BG,
};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct ContextVizState {
    pub visible: bool,
}

impl ContextVizState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self) {
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub fn render_context_viz(
    frame: &mut Frame,
    state: &ContextVizState,
    area: Rect,
    context_used: u64,
    context_total: u64,
    rate_5h: Option<f32>,
    rate_7d: Option<f32>,
    cost_usd: f64,
) {
    if !state.visible {
        return;
    }

    let layout = begin_modal_frame(frame, area, 72, 20, 2, 1);
    render_modal_title_frame(frame, layout.header_area, "Context & usage", "esc");
    if let Some(subtitle_area) = modal_header_line_area(layout.header_area, 1) {
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                " Token window, rate limits, and session cost.",
                Style::default().fg(CLAURST_MUTED),
            )])),
            subtitle_area,
        );
    }
    let inner = layout.body_area;

    // bar_width: leave room for "  label  [" prefix (14 chars) and "] 100%" suffix (6 chars)
    let bar_width = (inner.width as usize).saturating_sub(22).max(4);

    let ctx_pct = if context_total > 0 {
        (context_used as f32 / context_total as f32).min(1.0)
    } else {
        0.0
    };
    let ctx_color = if ctx_pct > 0.95 {
        Color::Red
    } else if ctx_pct > 0.80 {
        Color::Yellow
    } else {
        Color::Green
    };

    let mut lines: Vec<Line<'static>> = Vec::new();

    // -- Context window ----------------------------------------------------------
    lines.push(Line::from(vec![Span::styled(
        " Context window",
        Style::default()
            .fg(CLAURST_ACCENT)
            .add_modifier(Modifier::BOLD),
    )]));

    let filled = ((ctx_pct * bar_width as f32) as usize).min(bar_width);
    let empty = bar_width - filled;
    lines.push(Line::from(vec![
        Span::styled(" [", Style::default().fg(CLAURST_MUTED)),
        Span::styled("\u{2588}".repeat(filled), Style::default().fg(ctx_color)),
        Span::styled("\u{2591}".repeat(empty), Style::default().fg(CLAURST_MUTED)),
        Span::styled(
            format!(
                "]  {:.0}%  ({} / {})",
                ctx_pct * 100.0,
                format_tokens(context_used),
                format_tokens(context_total),
            ),
            Style::default().fg(ctx_color),
        ),
    ]));

    lines.push(Line::from(""));

    // -- Rate limits -------------------------------------------------------------
    lines.push(Line::from(vec![Span::styled(
        " Rate limits",
        Style::default()
            .fg(CLAURST_ACCENT)
            .add_modifier(Modifier::BOLD),
    )]));

    for (label, pct_opt) in &[(" 5-hour ", rate_5h), (" 7-day  ", rate_7d)] {
        match pct_opt {
            Some(pct) => {
                let p = pct.clamp(0.0, 1.0);
                let color = if p > 0.90 {
                    Color::Red
                } else if p > 0.70 {
                    Color::Yellow
                } else {
                    Color::Green
                };
                let f = ((p * bar_width as f32) as usize).min(bar_width);
                let e = bar_width - f;
                lines.push(Line::from(vec![
                    Span::styled(label.to_string(), Style::default().fg(Color::White)),
                    Span::styled("  [", Style::default().fg(CLAURST_MUTED)),
                    Span::styled("\u{2588}".repeat(f), Style::default().fg(color)),
                    Span::styled("\u{2591}".repeat(e), Style::default().fg(CLAURST_MUTED)),
                    Span::styled(format!("]  {:.0}%", p * 100.0), Style::default().fg(color)),
                ]));
            }
            None => {
                lines.push(Line::from(vec![
                    Span::styled(label.to_string(), Style::default().fg(Color::White)),
                    Span::styled("  no data", Style::default().fg(CLAURST_MUTED)),
                ]));
            }
        }
    }

    lines.push(Line::from(""));

    // -- Cost --------------------------------------------------------------------
    lines.push(Line::from(vec![
        Span::styled(" Session cost:  ", Style::default().fg(Color::White)),
        Span::styled(
            format!("${:.4}", cost_usd),
            Style::default()
                .fg(CLAURST_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(CLAURST_PANEL_BG))
        .render(inner, frame.buffer_mut());
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            " enter/esc close",
            Style::default()
                .fg(CLAURST_MUTED)
                .add_modifier(Modifier::ITALIC),
        )])),
        layout.footer_area,
    );
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn context_viz_defaults_hidden() {
        let state = ContextVizState::new();
        assert!(!state.visible);
    }

    #[test]
    fn context_viz_toggle() {
        let mut state = ContextVizState::new();
        state.toggle();
        assert!(state.visible);
        state.toggle();
        assert!(!state.visible);
    }

    #[test]
    fn context_viz_renders_without_panic() {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut state = ContextVizState::new();
        state.open();
        terminal
            .draw(|frame| {
                render_context_viz(
                    frame,
                    &state,
                    frame.area(),
                    50_000,
                    200_000,
                    Some(0.3),
                    Some(0.1),
                    0.42,
                );
            })
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("Context") || content.contains("Rate"));
    }

    #[test]
    fn context_viz_hidden_renders_nothing() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let state = ContextVizState::new();
        let before = terminal.backend().buffer().clone();
        terminal
            .draw(|frame| {
                render_context_viz(frame, &state, frame.area(), 0, 0, None, None, 0.0);
            })
            .unwrap();
        assert_eq!(terminal.backend().buffer().content(), before.content());
    }
}
