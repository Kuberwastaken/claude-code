// feedback_survey.rs — Session quality survey overlay matching TS FeedbackSurvey.tsx

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::overlays::centered_rect;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackResponse {
    Bad,
    Fine,
    Good,
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackSurveyStage {
    /// Ask 1=Bad 2=Fine 3=Good 0=Dismiss
    Rating,
    /// Ask to share transcript: 1=Yes 2=No 3=DontAskAgain
    SharePrompt,
    /// Show thank-you message
    Thanks,
    /// Survey is closed / not active
    Closed,
}

pub struct FeedbackSurveyState {
    pub visible: bool,
    pub stage: FeedbackSurveyStage,
    pub response: Option<FeedbackResponse>,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl FeedbackSurveyState {
    pub fn new() -> Self {
        Self {
            visible: false,
            stage: FeedbackSurveyStage::Closed,
            response: None,
        }
    }

    pub fn open(&mut self) {
        self.visible = true;
        self.stage = FeedbackSurveyStage::Rating;
        self.response = None;
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.stage = FeedbackSurveyStage::Closed;
    }

    /// Handle a digit key press. Returns `true` if the survey consumed the key.
    ///
    /// Stage=Rating:      1→Bad, 2→Fine, 3→Good, 0→Dismissed
    ///   Good → transition to SharePrompt, otherwise → Thanks
    /// Stage=SharePrompt: 1/2/3 → show Thanks and close
    /// Stage=Thanks:      any key → close
    pub fn handle_digit(&mut self, digit: u8) -> bool {
        if !self.visible {
            return false;
        }
        match &self.stage {
            FeedbackSurveyStage::Rating => {
                match digit {
                    0 => {
                        self.response = Some(FeedbackResponse::Dismissed);
                        self.stage = FeedbackSurveyStage::Thanks;
                    }
                    1 => {
                        self.response = Some(FeedbackResponse::Bad);
                        self.stage = FeedbackSurveyStage::Thanks;
                    }
                    2 => {
                        self.response = Some(FeedbackResponse::Fine);
                        self.stage = FeedbackSurveyStage::Thanks;
                    }
                    3 => {
                        self.response = Some(FeedbackResponse::Good);
                        self.stage = FeedbackSurveyStage::SharePrompt;
                    }
                    _ => {}
                }
                true
            }
            FeedbackSurveyStage::SharePrompt => {
                if matches!(digit, 1 | 2 | 3) {
                    self.stage = FeedbackSurveyStage::Thanks;
                }
                true
            }
            FeedbackSurveyStage::Thanks => {
                self.close();
                true
            }
            FeedbackSurveyStage::Closed => false,
        }
    }
}

impl Default for FeedbackSurveyState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the feedback survey as a centered floating dialog.
pub fn render_feedback_survey(state: &FeedbackSurveyState, area: Rect, buf: &mut Buffer) {
    if !state.visible {
        return;
    }

    let dialog_area = centered_rect(50, 8, area);

    // Clear the region first
    for y in dialog_area.y..dialog_area.y + dialog_area.height {
        for x in dialog_area.x..dialog_area.x + dialog_area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
            }
        }
    }

    let (title, body_lines): (&str, Vec<Line>) = match &state.stage {
        FeedbackSurveyStage::Rating => (
            " Session Feedback ",
            vec![
                Line::from(""),
                Line::from(vec![Span::styled(
                    "  How is Claurst doing this session? (optional)",
                    Style::default().fg(Color::White),
                )]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "1",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" Bad   "),
                    Span::styled(
                        "2",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" Fine   "),
                    Span::styled(
                        "3",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" Good   "),
                    Span::styled(
                        "0",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" Dismiss"),
                ]),
            ],
        ),
        FeedbackSurveyStage::SharePrompt => (
            " Share Transcript? ",
            vec![
                Line::from(""),
                Line::from(vec![Span::styled(
                    "  Can Anthropic look at your session transcript?",
                    Style::default().fg(Color::White),
                )]),
                Line::from(""),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "1",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" Yes   "),
                    Span::styled(
                        "2",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" No   "),
                    Span::styled(
                        "3",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" Don't ask again"),
                ]),
            ],
        ),
        FeedbackSurveyStage::Thanks | FeedbackSurveyStage::Closed => (
            " Thank You ",
            vec![
                Line::from(""),
                Line::from(vec![Span::styled(
                    "  Thank you for your feedback!",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )]),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "  Press any key to close",
                    Style::default().fg(Color::DarkGray),
                )]),
            ],
        ),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Rgb(233, 30, 99)));

    let para = Paragraph::new(body_lines)
        .block(block)
        .alignment(Alignment::Left);

    use ratatui::widgets::Widget;
    para.render(dialog_area, buf);
}
