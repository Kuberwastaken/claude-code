// device_auth_dialog.rs — Device code / browser-based auth overlay.
//
// Provides a modal dialog that shows the device code flow status for GitHub
// Copilot (RFC 8628) and browser-based OAuth for Anthropic.  The actual
// network requests run in a background tokio task; this module only owns the
// display state.

use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::overlays::{centered_rect, render_dark_overlay, render_dialog_bg, CLAURST_PANEL_BG};

// ---------------------------------------------------------------------------
// Status enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum DeviceAuthStatus {
    /// Dialog is idle / not started.
    Idle,
    /// Requesting the device code from the authorization server.
    WaitingForCode,
    /// User code is displayed; waiting for the user to authorize in-browser.
    ShowingCode,
    /// Actively polling the token endpoint.
    Polling,
    /// Browser-based auth (Anthropic OAuth) — browser was opened.
    BrowserAuth,
    /// Successfully obtained a token.
    Success(String),
    /// An error occurred.
    Error(String),
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct DeviceAuthDialogState {
    pub visible: bool,
    pub provider_id: String,
    pub provider_name: String,
    pub status: DeviceAuthStatus,
    pub user_code: String,
    pub verification_uri: String,
    pub device_code: String,
    pub interval: u64,
}

impl DeviceAuthDialogState {
    pub fn new() -> Self {
        Self {
            visible: false,
            provider_id: String::new(),
            provider_name: String::new(),
            status: DeviceAuthStatus::Idle,
            user_code: String::new(),
            verification_uri: String::new(),
            device_code: String::new(),
            interval: 5,
        }
    }

    /// Open the dialog for a specific provider and begin the auth flow.
    pub fn open(&mut self, provider_id: String, provider_name: String) {
        self.visible = true;
        self.provider_id = provider_id;
        self.provider_name = provider_name;
        self.status = DeviceAuthStatus::WaitingForCode;
        self.user_code.clear();
        self.verification_uri.clear();
        self.device_code.clear();
    }

    /// Close and reset the dialog.
    pub fn close(&mut self) {
        self.visible = false;
        self.status = DeviceAuthStatus::Idle;
    }

    /// Set the device code information received from the authorization server.
    pub fn set_code(
        &mut self,
        user_code: String,
        verification_uri: String,
        device_code: String,
        interval: u64,
    ) {
        self.user_code = user_code;
        self.verification_uri = verification_uri;
        self.device_code = device_code;
        self.interval = interval;
        self.status = DeviceAuthStatus::ShowingCode;
    }

    /// Transition to the polling state (code has been shown, now waiting for
    /// the user to complete authorization).
    pub fn set_polling(&mut self) {
        self.status = DeviceAuthStatus::Polling;
    }

    /// Mark the flow as successful with the obtained token.
    pub fn set_success(&mut self, token: String) {
        self.status = DeviceAuthStatus::Success(token);
    }

    /// Mark the flow as failed.
    pub fn set_error(&mut self, msg: String) {
        self.status = DeviceAuthStatus::Error(msg);
    }
}

// ---------------------------------------------------------------------------
// Events sent from the background task to the main loop
// ---------------------------------------------------------------------------

/// Messages sent from the background device-code / OAuth task back to the
/// main event loop so it can update the dialog state.
pub enum DeviceAuthEvent {
    /// Device code received — show the user code and verification URI.
    GotCode {
        user_code: String,
        verification_uri: String,
        device_code: String,
        interval: u64,
    },
    /// Access token obtained — auth succeeded.
    TokenReceived(String),
    /// Something went wrong.
    Error(String),
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the device auth dialog overlay — OpenCode-style: dark overlay, no
/// border, minimal and polished.
pub fn render_device_auth_dialog(frame: &mut Frame, state: &DeviceAuthDialogState, area: Rect) {
    if !state.visible {
        return;
    }

    let pink = Color::Rgb(233, 30, 99);
    let dim = Color::Rgb(90, 90, 90);
    let dialog_bg = CLAURST_PANEL_BG;
    let green = Color::Rgb(80, 200, 120);

    // ── Darken the entire background ──
    render_dark_overlay(frame, area);

    // ── Dialog size ──
    let width = 56u16.min(area.width.saturating_sub(4));
    let height = 14u16;
    let dialog_area = centered_rect(width, height, area);

    // ── Fill dialog background (no border) ──
    render_dialog_bg(frame, dialog_area);

    let inner = Rect {
        x: dialog_area.x + 1,
        y: dialog_area.y + 1,
        width: dialog_area.width.saturating_sub(2),
        height: dialog_area.height.saturating_sub(2),
    };

    // ── Build lines ──
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Title row: "Connect {provider}" on left, "esc" on right
    let title_text = format!("Connect {}", state.provider_name);
    let title_pad = inner.width.saturating_sub(title_text.len() as u16 + 5) as usize;
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {}", title_text),
            Style::default().fg(pink).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>width$}", "esc ", width = title_pad),
            Style::default().fg(dim),
        ),
    ]));

    // Status-dependent content
    match &state.status {
        DeviceAuthStatus::Idle | DeviceAuthStatus::WaitingForCode => {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " Requesting device code...",
                Style::default().fg(Color::Yellow),
            )));
        }
        DeviceAuthStatus::ShowingCode | DeviceAuthStatus::Polling => {
            let status_text = if state.status == DeviceAuthStatus::Polling {
                " Checking for authorization..."
            } else {
                " Waiting for authorization..."
            };
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " Enter this code:",
                Style::default().fg(Color::Rgb(180, 180, 180)),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("    {}", state.user_code),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(" at ", Style::default().fg(dim)),
                Span::styled(
                    state.verification_uri.clone(),
                    Style::default().fg(pink).add_modifier(Modifier::UNDERLINED),
                ),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                status_text,
                Style::default().fg(Color::Yellow),
            )));
        }
        DeviceAuthStatus::BrowserAuth => {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " Opening browser for authentication...",
                Style::default().fg(Color::Yellow),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " Complete the login in your browser.",
                Style::default().fg(dim),
            )));
            lines.push(Line::from(Span::styled(
                " This dialog will update when done.",
                Style::default().fg(dim),
            )));
        }
        DeviceAuthStatus::Success(_) => {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " \u{2714} Connected successfully!",
                Style::default().fg(green).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " Press any key to continue.",
                Style::default().fg(dim),
            )));
        }
        DeviceAuthStatus::Error(msg) => {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!(" Error: {}", msg),
                Style::default().fg(Color::Red),
            )));
        }
    };

    let para = Paragraph::new(lines).bg(dialog_bg);
    frame.render_widget(para, inner);
}
