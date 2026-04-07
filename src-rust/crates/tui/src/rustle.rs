//! Rustle mascot rendering for ratatui.
//!
//! A 5-row Unicode block-art crab-like creature. Call `rustle_lines()` to get
//! 5 `Line` values (4 body rows + 1 blank spacing row) ready for embedding in
//! a Paragraph.
//!
//! Structure (top to bottom):
//!   Row 1 — head: narrow top (5-wide) widening downward (7-wide)
//!   Row 2 — claws + eyes: widest row, pincers extend from sides
//!   Row 3 — body
//!   Row 4 — legs: body tapers into two pairs of legs via ▀ gap
//!   Row 5 — blank spacing

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// The pose / expression of the Rustle mascot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RustlePose {
    Default,
    ArmsUp,
    LookLeft,
    LookRight,
    LookDown,
    /// Loading / error spinner — `frame` drives the animation.
    Loading {
        frame: u64,
    },
}

/// Body-part style: bold pink foreground (#e91e63).
fn body_style() -> Style {
    Style::default()
        .fg(Color::Rgb(233, 30, 99))
        .add_modifier(Modifier::BOLD)
}

/// Eye-row style: pink text on black background.
fn eye_bg_style() -> Style {
    Style::default()
        .fg(Color::Rgb(233, 30, 99))
        .bg(Color::Black)
        .add_modifier(Modifier::BOLD)
}

/// Eyeball highlight style: white on black.
fn eyeball_style() -> Style {
    Style::default()
        .fg(Color::White)
        .bg(Color::Black)
        .add_modifier(Modifier::BOLD)
}

/// Build spans for the eye section, giving ▘/▝ eyeball characters white
/// foreground and everything else pink-on-black.
fn eye_spans(s: &'static str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut buf_is_eyeball = false;

    for ch in s.chars() {
        let is_eyeball = ch == '▘' || ch == '▝' || ch == '▀' || ch == '▄' || ch == '▖';
        if is_eyeball != buf_is_eyeball && !buf.is_empty() {
            let style = if buf_is_eyeball {
                eyeball_style()
            } else {
                eye_bg_style()
            };
            spans.push(Span::styled(buf.clone(), style));
            buf.clear();
        }
        buf_is_eyeball = is_eyeball;
        buf.push(ch);
    }
    if !buf.is_empty() {
        let style = if buf_is_eyeball {
            eyeball_style()
        } else {
            eye_bg_style()
        };
        spans.push(Span::styled(buf, style));
    }
    spans
}

/// Build the spinner eye spans for the Loading pose.
///
/// Each eye socket is a single character cell with a 2×2 sub-pixel grid.
/// The spinner rotates which quarter-block is lit:
///   Left eye (clockwise):        ▘ → ▝ → ▗ → ▖
///   Right eye (anti-clockwise):  ▝ → ▘ → ▖ → ▗
///
/// The current position is white; trailing positions use progressively
/// darker grays so the animation looks like a sweeping gradient.
fn loading_eye_spans(frame: u64) -> Vec<Span<'static>> {
    // Quarter-block characters for each 2×2 position:
    //   0=TL(▘)  1=TR(▝)  2=BR(▗)  3=BL(▖)
    const QUARTERS: [char; 4] = ['▘', '▝', '▗', '▖'];
    // Clockwise order for left eye
    const CW: [usize; 4] = [0, 1, 2, 3];
    // Anti-clockwise order for right eye (mirrored)
    const CCW: [usize; 4] = [1, 0, 3, 2];

    // Brightness gradient: current → trailing positions
    const COLORS: [Color; 4] = [
        Color::White,
        Color::Rgb(170, 170, 175),
        Color::Rgb(110, 110, 115),
        Color::Rgb(55, 55, 60),
    ];

    // One step every 5 frames (~250ms at 50ms/frame = smooth spin)
    let step = (frame / 5) as usize % 4;

    // Build left eye: show all 4 quarter-blocks overlaid via half-blocks.
    // Since one character can only show one quarter, we show the BRIGHTEST
    // position as the visible quarter-block character and cycle it.
    let left_ch = QUARTERS[CW[step]];
    let left_color = COLORS[0]; // current position is always white

    let right_ch = QUARTERS[CCW[step]];
    let right_color = COLORS[0];

    // For a trail effect, also render the PREVIOUS position in a dimmer shade
    // using a second character. The eye section format is:
    //   [left_prev][left_curr] █ [right_curr][right_prev]
    let left_prev_step = (step + 3) % 4; // one step back
    let left_prev_ch = QUARTERS[CW[left_prev_step]];
    let right_prev_step = (step + 3) % 4;
    let right_prev_ch = QUARTERS[CCW[right_prev_step]];

    vec![
        // Left eye: previous (dim) then current (bright)
        Span::styled(
            left_prev_ch.to_string(),
            Style::default()
                .fg(COLORS[2])
                .bg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            left_ch.to_string(),
            Style::default()
                .fg(left_color)
                .bg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        // Nose
        Span::styled("█".to_string(), eye_bg_style()),
        // Right eye: current (bright) then previous (dim)
        Span::styled(
            right_ch.to_string(),
            Style::default()
                .fg(right_color)
                .bg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            right_prev_ch.to_string(),
            Style::default()
                .fg(COLORS[2])
                .bg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
    ]
}

/// Returns 5 Lines representing the Rustle mascot:
///   [0] — head row (5-wide top, 7-wide bottom)
///   [1] — claws + eyes row (widest — pincers extend from sides)
///   [2] — body row
///   [3] — legs row (body tapers into two pairs of legs)
///   [4] — blank spacing line
pub fn rustle_lines(pose: &RustlePose) -> [Line<'static>; 5] {
    // Pose varies the claw row (Row 2):
    //   r2l — left claw + head edge (body_style)
    //   r2e — eye section with ▘/▝ eyeball highlights
    //   r2r — head edge + right claw (body_style)

    let (r2l, r2e, r2r) = match pose {
        RustlePose::Default => (
            "█▄█",   // left claw tip, ▄ gap-to-connect, head edge
            "▀ █▀ ", // keep the left eye as-is; match the right eye size and move it up-left
            "█▄█",   // head edge, ▄ connect-to-gap, right claw tip
        ),
        RustlePose::ArmsUp => (
            "█▀█",   // ▀ = claw raised (upper half = arm up)
            "▀ █▀ ", // keep the left eye as-is; match the right eye size and move it up-left
            "█▀█",   // raised right claw
        ),
        RustlePose::LookLeft => (
            "█▄█",
            "▘ █ ▘", // single-pixel upper-left quarter blocks = eyes shifted left
            "█▄█",
        ),
        RustlePose::LookRight => (
            "█▄█",
            "▝ █ ▝", // single-pixel upper-right quarter blocks = eyes shifted right
            "█▄█",
        ),
        RustlePose::LookDown => (
            "█▄█",
            "▄ █▄ ", // lower-half blocks = eyes shifted down
            "█▄█",
        ),
        RustlePose::Loading { .. } => (
            "█▄█",
            "",
            "█▄█", // eyes built separately via loading_eye_spans
        ),
    };

    // Row 1: head — narrow top (5-wide), wider bottom (7-wide)
    let row1 = Line::from(vec![Span::styled("  ▄█████▄  ".to_string(), body_style())]);

    // Row 2: claws extending from sides + face with eyeball highlights (widest row)
    let mut row2_spans = vec![Span::styled(r2l.to_string(), body_style())];
    if let RustlePose::Loading { frame } = pose {
        row2_spans.extend(loading_eye_spans(*frame));
    } else {
        row2_spans.extend(eye_spans(r2e));
    }
    row2_spans.push(Span::styled(r2r.to_string(), body_style()));
    let row2 = Line::from(row2_spans);

    // Row 3: body
    let row3 = Line::from(vec![Span::styled(" ████████  ".to_string(), body_style())]);

    // Row 4: legs — upper half body (6-wide), lower half two leg pairs (2+gap+2)
    let row4 = Line::from(vec![Span::styled("  ██▀▀██   ".to_string(), body_style())]);

    // Row 5: blank spacing
    let row5 = Line::from("");

    [row1, row2, row3, row4, row5]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn default_pose_right_eye_matches_left_eye_size() {
        let lines = rustle_lines(&RustlePose::Default);
        assert_eq!(line_text(&lines[1]), "█▄█▀ █▀ █▄█");
    }

    #[test]
    fn arms_up_pose_right_eye_matches_left_eye_size() {
        let lines = rustle_lines(&RustlePose::ArmsUp);
        assert_eq!(line_text(&lines[1]), "█▀█▀ █▀ █▀█");
    }
}
