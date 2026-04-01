// app.rs — App state struct and main event loop.

use crate::bridge_state::BridgeConnectionState;
use crate::dialogs::PermissionRequest;
use crate::notifications::{NotificationKind, NotificationQueue};
use crate::overlays::{
    HelpOverlay, HistorySearchOverlay, MessageSelectorOverlay, RewindFlowOverlay, SelectorMessage,
};
use crate::plugin_views::PluginHintBanner;
use crate::privacy_screen::PrivacyScreen;
use crate::render;
use crate::settings_screen::SettingsScreen;
use crate::theme_screen::ThemeScreen;
use cc_core::config::{Config, Settings, Theme};
use cc_core::cost::CostTracker;
use cc_core::keybindings::{
    KeyContext, KeybindingResolver, KeybindingResult, ParsedKeystroke, UserKeybindings,
};
use cc_core::types::{Message, Role};
use cc_query::QueryEvent;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::sync::Arc;
use tracing::debug;

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Status of an active or completed tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Done,
    Error,
}

/// Represents an active or completed tool invocation visible in the UI.
#[derive(Debug, Clone)]
pub struct ToolUseBlock {
    pub id: String,
    pub name: String,
    pub status: ToolStatus,
    pub output_preview: Option<String>,
}

/// State for Ctrl+R history search mode (legacy inline struct, kept for test
/// compatibility — the overlay version lives in `overlays::HistorySearchOverlay`).
#[derive(Debug, Clone)]
pub struct HistorySearch {
    pub query: String,
    /// Indices into `input_history` that match the current query.
    pub matches: Vec<usize>,
    /// Which match is currently highlighted.
    pub selected: usize,
}

impl HistorySearch {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
        }
    }

    /// Re-compute matches against the given history slice.
    pub fn update_matches(&mut self, history: &[String]) {
        let q = self.query.to_lowercase();
        self.matches = history
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                if s.to_lowercase().contains(&q) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        // Clamp selected to valid range
        if !self.matches.is_empty() && self.selected >= self.matches.len() {
            self.selected = self.matches.len() - 1;
        }
    }

    /// Return the currently selected history entry, if any.
    pub fn current_entry<'a>(&self, history: &'a [String]) -> Option<&'a str> {
        self.matches
            .get(self.selected)
            .and_then(|&i| history.get(i))
            .map(String::as_str)
    }
}

fn key_event_to_keystroke(key: &KeyEvent) -> Option<ParsedKeystroke> {
    let normalized_key = match key.code {
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Esc => "escape".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::BackTab => "tab".to_string(),
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_lowercase().to_string(),
        _ => return None,
    };

    Some(ParsedKeystroke {
        key: normalized_key,
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
        meta: key.modifiers.contains(KeyModifiers::SUPER),
    })
}

// ---------------------------------------------------------------------------
// App struct
// ---------------------------------------------------------------------------

/// The top-level TUI application.
pub struct App {
    // Core state
    pub config: Config,
    pub cost_tracker: Arc<CostTracker>,
    pub messages: Vec<Message>,
    pub input: String,
    pub input_history: Vec<String>,
    pub history_index: Option<usize>,
    pub scroll_offset: usize,
    pub is_streaming: bool,
    pub streaming_text: String,
    pub status_message: Option<String>,
    pub should_quit: bool,
    pub show_help: bool,

    // Extended state
    pub tool_use_blocks: Vec<ToolUseBlock>,
    pub permission_request: Option<PermissionRequest>,
    pub frame_count: u64,
    pub token_count: u32,
    pub cost_usd: f64,
    pub model_name: String,
    pub agent_status: Vec<(String, String)>,
    pub history_search: Option<HistorySearch>,
    pub keybindings: KeybindingResolver,

    // Cursor position within input (byte offset)
    pub cursor_pos: usize,

    // ---- New overlay / notification fields --------------------------------

    /// Full-screen help overlay (? / F1).
    pub help_overlay: HelpOverlay,
    /// Ctrl+R history search overlay.
    pub history_search_overlay: HistorySearchOverlay,
    /// Message selector used by /rewind.
    pub message_selector: MessageSelectorOverlay,
    /// Multi-step rewind flow overlay.
    pub rewind_flow: RewindFlowOverlay,
    /// Bridge connection state.
    pub bridge_state: BridgeConnectionState,
    /// Active notification queue.
    pub notifications: NotificationQueue,
    /// Plugin hint banners.
    pub plugin_hints: Vec<PluginHintBanner>,
    /// Optional session title shown in the status bar.
    pub session_title: Option<String>,
    /// Remote session URL (set when bridge connects; readable by commands).
    pub remote_session_url: Option<String>,

    // ---- Settings / theme / privacy screens --------------------------------

    /// Full-screen tabbed settings screen (/config, /settings).
    pub settings_screen: SettingsScreen,
    /// Theme picker overlay (/theme).
    pub theme_screen: ThemeScreen,
    /// Privacy settings dialog (/privacy-settings).
    pub privacy_screen: PrivacyScreen,
}

impl App {
    pub fn new(config: Config, cost_tracker: Arc<CostTracker>) -> Self {
        let model_name = config.effective_model().to_string();
        let user_keybindings = UserKeybindings::load(&Settings::config_dir());
        Self {
            config,
            cost_tracker,
            messages: Vec::new(),
            input: String::new(),
            input_history: Vec::new(),
            history_index: None,
            scroll_offset: 0,
            is_streaming: false,
            streaming_text: String::new(),
            status_message: None,
            should_quit: false,
            show_help: false,
            tool_use_blocks: Vec::new(),
            permission_request: None,
            frame_count: 0,
            token_count: 0,
            cost_usd: 0.0,
            model_name,
            agent_status: Vec::new(),
            history_search: None,
            keybindings: KeybindingResolver::new(&user_keybindings),
            cursor_pos: 0,
            help_overlay: HelpOverlay::new(),
            history_search_overlay: HistorySearchOverlay::new(),
            message_selector: MessageSelectorOverlay::new(),
            rewind_flow: RewindFlowOverlay::new(),
            bridge_state: BridgeConnectionState::Disconnected,
            notifications: NotificationQueue::new(),
            plugin_hints: Vec::new(),
            session_title: None,
            remote_session_url: None,
            settings_screen: SettingsScreen::new(),
            theme_screen: ThemeScreen::new(),
            privacy_screen: PrivacyScreen::new(),
        }
    }

    /// Update the active model name (also updates cost tracker).
    pub fn set_model(&mut self, model: String) {
        self.cost_tracker.set_model(&model);
        self.model_name = model;
    }

    /// Apply a theme by name, persisting it to config.
    pub fn apply_theme(&mut self, theme_name: &str) {
        let theme = match theme_name {
            "dark" => Theme::Dark,
            "light" => Theme::Light,
            "default" => Theme::Default,
            other => Theme::Custom(other.to_string()),
        };
        self.config.theme = theme;
        // Persist to settings file
        let mut settings = Settings::load_sync().unwrap_or_default();
        settings.config.theme = self.config.theme.clone();
        let _ = settings.save_sync();
        self.status_message = Some(format!("Theme set to: {}", theme_name));
    }

    /// Handle slash commands that should open UI screens rather than execute
    /// as normal commands. Returns `true` if the command was intercepted.
    pub fn intercept_slash_command(&mut self, cmd: &str) -> bool {
        match cmd {
            "config" | "settings" => {
                self.settings_screen.open();
                true
            }
            "theme" => {
                let current = match &self.config.theme {
                    Theme::Dark => "dark",
                    Theme::Light => "light",
                    Theme::Default => "default",
                    Theme::Custom(s) => s.as_str(),
                };
                self.theme_screen.open(current);
                true
            }
            "privacy-settings" | "privacy" => {
                self.privacy_screen.open();
                true
            }
            _ => false,
        }
    }

    /// Add a message directly (e.g. from a non-streaming source).
    pub fn add_message(&mut self, role: Role, text: String) {
        self.messages.push(match role {
            Role::User => Message::user(text),
            Role::Assistant => Message::assistant(text),
        });
    }

    /// Take the current input buffer, push it to history, and return it.
    pub fn take_input(&mut self) -> String {
        let input = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        if !input.is_empty() {
            self.input_history.push(input.clone());
            self.history_index = None;
        }
        input
    }

    /// Open the rewind flow with the current message list converted to
    /// `SelectorMessage` entries.
    pub fn open_rewind_flow(&mut self) {
        let selector_msgs: Vec<SelectorMessage> = self
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let text = m.get_all_text();
                let preview: String = text.chars().take(80).collect();
                let has_tool_use = !m.get_tool_use_blocks().is_empty();
                SelectorMessage {
                    idx: i,
                    role: format!("{:?}", m.role).to_lowercase(),
                    preview,
                    has_tool_use,
                }
            })
            .collect();
        self.rewind_flow.open(selector_msgs);
    }

    // -------------------------------------------------------------------
    // Event handling
    // -------------------------------------------------------------------

    /// Process a keyboard event. Returns `true` when the input should be
    /// submitted (Enter pressed with no blocking dialog).
    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        let key_context = self.current_key_context();
        if let Some(keystroke) = key_event_to_keystroke(&key) {
            let had_pending_chord = self.keybindings.has_pending_chord();
            match self.keybindings.process(keystroke, &key_context) {
                KeybindingResult::Action(action) => {
                    return self.handle_keybinding_action(&action);
                }
                KeybindingResult::Unbound | KeybindingResult::Pending => return false,
                KeybindingResult::NoMatch if had_pending_chord => return false,
                KeybindingResult::NoMatch => {}
            }
        } else {
            self.keybindings.cancel_chord();
        }

        // Settings screen intercepts keys
        if self.settings_screen.visible {
            crate::settings_screen::handle_settings_key(
                &mut self.settings_screen,
                &mut self.config,
                key,
            );
            return false;
        }

        // Theme picker intercepts keys
        if self.theme_screen.visible {
            if let Some(theme_name) =
                crate::theme_screen::handle_theme_key(&mut self.theme_screen, key)
            {
                self.apply_theme(&theme_name);
            }
            return false;
        }

        // Privacy screen intercepts keys
        if self.privacy_screen.visible {
            crate::privacy_screen::handle_privacy_key(&mut self.privacy_screen, key);
            return false;
        }

        // Rewind flow overlay intercepts keys first
        if self.rewind_flow.visible {
            return self.handle_rewind_flow_key(key);
        }

        // Help overlay intercepts keys next
        if self.help_overlay.visible {
            return self.handle_help_overlay_key(key);
        }

        // New history-search overlay
        if self.history_search_overlay.visible {
            return self.handle_history_search_overlay_key(key);
        }

        // Legacy history-search mode intercepts most keys
        if self.history_search.is_some() {
            return self.handle_history_search_key(key);
        }

        // Permission dialog mode intercepts most keys
        if self.permission_request.is_some() {
            self.handle_permission_key(key);
            return false;
        }

        // Notification dismiss
        if key.code == KeyCode::Esc && !self.notifications.is_empty() {
            self.notifications.dismiss_current();
            return false;
        }

        // Plugin hint dismiss
        if key.code == KeyCode::Esc {
            if let Some(hint) = self.plugin_hints.iter_mut().find(|h| h.is_visible()) {
                hint.dismiss();
                return false;
            }
        }

        match key.code {
            // ---- Quit / cancel ----------------------------------------
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.is_streaming {
                    self.is_streaming = false;
                    self.streaming_text.clear();
                    self.tool_use_blocks.clear();
                    self.status_message = Some("Cancelled.".to_string());
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.input.is_empty() {
                    self.should_quit = true;
                }
            }

            // ---- History search ----------------------------------------
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Open the new overlay-based history search
                let overlay = HistorySearchOverlay::open(&self.input_history);
                self.history_search_overlay = overlay;
                // Also open legacy for backwards compat
                let mut hs = HistorySearch::new();
                hs.update_matches(&self.input_history);
                self.history_search = Some(hs);
            }

            // ---- Help overlay ------------------------------------------
            KeyCode::F(1) => {
                self.show_help = !self.show_help;
                self.help_overlay.toggle();
            }
            KeyCode::Char('?') if key.modifiers.is_empty() && !self.is_streaming && self.input.is_empty() => {
                self.show_help = !self.show_help;
                self.help_overlay.toggle();
            }

            // ---- Text entry (blocked while streaming) ------------------
            KeyCode::Char(c) if !self.is_streaming => {
                let byte_pos = self.char_boundary_at(self.cursor_pos);
                self.input.insert(byte_pos, c);
                self.cursor_pos += c.len_utf8();
            }
            KeyCode::Backspace if !self.is_streaming => {
                if self.cursor_pos > 0 {
                    let end = self.char_boundary_at(self.cursor_pos);
                    let start = self.prev_char_boundary(end);
                    self.input.drain(start..end);
                    self.cursor_pos = start;
                }
            }
            KeyCode::Delete if !self.is_streaming => {
                let start = self.char_boundary_at(self.cursor_pos);
                if start < self.input.len() {
                    let end = self.next_char_boundary(start);
                    self.input.drain(start..end);
                }
            }
            KeyCode::Left if !self.is_streaming => {
                if self.cursor_pos > 0 {
                    let end = self.char_boundary_at(self.cursor_pos);
                    self.cursor_pos = self.prev_char_boundary(end);
                }
            }
            KeyCode::Right if !self.is_streaming => {
                let start = self.char_boundary_at(self.cursor_pos);
                if start < self.input.len() {
                    self.cursor_pos = self.next_char_boundary(start);
                }
            }
            KeyCode::Home if !self.is_streaming => {
                self.cursor_pos = 0;
            }
            KeyCode::End if !self.is_streaming => {
                self.cursor_pos = self.input.len();
            }

            // ---- Submit ------------------------------------------------
            KeyCode::Enter if !self.is_streaming => {
                return true;
            }

            // ---- Input history navigation ------------------------------
            KeyCode::Up => {
                if !self.input_history.is_empty() {
                    let idx = match self.history_index {
                        Some(i) => i.saturating_sub(1),
                        None => self.input_history.len().saturating_sub(1),
                    };
                    self.history_index = Some(idx);
                    self.input = self.input_history[idx].clone();
                    self.cursor_pos = self.input.len();
                }
            }
            KeyCode::Down => {
                if let Some(idx) = self.history_index {
                    if idx + 1 < self.input_history.len() {
                        let new_idx = idx + 1;
                        self.history_index = Some(new_idx);
                        self.input = self.input_history[new_idx].clone();
                        self.cursor_pos = self.input.len();
                    } else {
                        self.history_index = None;
                        self.input.clear();
                        self.cursor_pos = 0;
                    }
                }
            }

            // ---- Scroll ------------------------------------------------
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
            }

            _ => {}
        }
        false
    }

    fn current_key_context(&self) -> KeyContext {
        if self.settings_screen.visible {
            KeyContext::Settings
        } else if self.theme_screen.visible {
            KeyContext::ThemePicker
        } else if self.rewind_flow.visible {
            KeyContext::Confirmation
        } else if self.help_overlay.visible {
            KeyContext::Help
        } else if self.history_search_overlay.visible || self.history_search.is_some() {
            KeyContext::HistorySearch
        } else if self.permission_request.is_some() {
            KeyContext::Confirmation
        } else if self.show_help {
            KeyContext::Help
        } else {
            KeyContext::Chat
        }
    }

    // -------------------------------------------------------------------
    // New overlay key handlers
    // -------------------------------------------------------------------

    fn handle_help_overlay_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::F(1) => {
                self.help_overlay.close();
                self.show_help = false;
            }
            KeyCode::Char('?') if key.modifiers.is_empty() => {
                self.help_overlay.close();
                self.show_help = false;
            }
            KeyCode::Up => {
                self.help_overlay.scroll_up();
            }
            KeyCode::Down => {
                let max = 50u16; // generous upper bound; renderer will clamp
                self.help_overlay.scroll_down(max);
            }
            KeyCode::Backspace => {
                self.help_overlay.pop_filter_char();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.help_overlay.push_filter_char(c);
            }
            _ => {}
        }
        false
    }

    fn handle_history_search_overlay_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.history_search_overlay.close();
                self.history_search = None;
            }
            KeyCode::Enter => {
                if let Some(entry) = self
                    .history_search_overlay
                    .current_entry(&self.input_history)
                {
                    self.input = entry.to_string();
                    self.cursor_pos = self.input.len();
                }
                self.history_search_overlay.close();
                self.history_search = None;
            }
            KeyCode::Up => {
                self.history_search_overlay.select_prev();
                if let Some(hs) = self.history_search.as_mut() {
                    if hs.selected > 0 {
                        hs.selected -= 1;
                    }
                }
            }
            KeyCode::Down => {
                self.history_search_overlay.select_next();
                if let Some(hs) = self.history_search.as_mut() {
                    let max = hs.matches.len().saturating_sub(1);
                    if hs.selected < max {
                        hs.selected += 1;
                    }
                }
            }
            KeyCode::Backspace => {
                let history = self.input_history.clone();
                self.history_search_overlay.pop_char(&history);
                if let Some(hs) = self.history_search.as_mut() {
                    hs.query.pop();
                    hs.update_matches(&history);
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let history = self.input_history.clone();
                self.history_search_overlay.push_char(c, &history);
                if let Some(hs) = self.history_search.as_mut() {
                    hs.query.push(c);
                    hs.update_matches(&history);
                }
            }
            _ => {}
        }
        false
    }

    fn handle_rewind_flow_key(&mut self, key: KeyEvent) -> bool {
        use crate::overlays::RewindStep;
        match &self.rewind_flow.step {
            RewindStep::Selecting => match key.code {
                KeyCode::Esc => {
                    self.rewind_flow.close();
                }
                KeyCode::Enter => {
                    self.rewind_flow.confirm_selection();
                }
                KeyCode::Up => {
                    self.rewind_flow.selector.select_prev();
                }
                KeyCode::Down => {
                    self.rewind_flow.selector.select_next();
                }
                _ => {}
            },
            RewindStep::Confirming { .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(idx) = self.rewind_flow.accept_confirm() {
                        // Truncate conversation to the selected message index
                        self.messages.truncate(idx);
                        self.notifications.push(
                            NotificationKind::Success,
                            format!("Rewound to message #{}", idx),
                            Some(4),
                        );
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.rewind_flow.reject_confirm();
                }
                _ => {}
            },
        }
        false
    }

    fn handle_keybinding_action(&mut self, action: &str) -> bool {
        match action {
            "interrupt" => {
                if self.is_streaming {
                    self.is_streaming = false;
                    self.streaming_text.clear();
                    self.tool_use_blocks.clear();
                    self.status_message = Some("Cancelled.".to_string());
                } else {
                    self.should_quit = true;
                }
                false
            }
            "exit" => {
                if self.input.is_empty() {
                    self.should_quit = true;
                }
                false
            }
            "redraw" => false,
            "historySearch" => {
                let overlay = HistorySearchOverlay::open(&self.input_history);
                self.history_search_overlay = overlay;
                let mut hs = HistorySearch::new();
                hs.update_matches(&self.input_history);
                self.history_search = Some(hs);
                false
            }
            "submit" => !self.is_streaming,
            "historyPrev" => {
                if !self.input_history.is_empty() {
                    let idx = match self.history_index {
                        Some(i) => i.saturating_sub(1),
                        None => self.input_history.len().saturating_sub(1),
                    };
                    self.history_index = Some(idx);
                    self.input = self.input_history[idx].clone();
                    self.cursor_pos = self.input.len();
                }
                false
            }
            "historyNext" => {
                if let Some(idx) = self.history_index {
                    if idx + 1 < self.input_history.len() {
                        let new_idx = idx + 1;
                        self.history_index = Some(new_idx);
                        self.input = self.input_history[new_idx].clone();
                        self.cursor_pos = self.input.len();
                    } else {
                        self.history_index = None;
                        self.input.clear();
                        self.cursor_pos = 0;
                    }
                }
                false
            }
            "scrollUp" => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
                false
            }
            "scrollDown" => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
                false
            }
            "yes" => {
                self.permission_request = None;
                false
            }
            "no" => {
                self.permission_request = None;
                false
            }
            "prevOption" => {
                if let Some(pr) = self.permission_request.as_mut() {
                    if pr.selected_option > 0 {
                        pr.selected_option -= 1;
                    }
                }
                false
            }
            "nextOption" => {
                if let Some(pr) = self.permission_request.as_mut() {
                    if pr.selected_option + 1 < pr.options.len() {
                        pr.selected_option += 1;
                    }
                }
                false
            }
            "close" => {
                self.show_help = false;
                self.help_overlay.close();
                false
            }
            "select" => {
                // Legacy history search select
                if let Some(hs) = self.history_search.as_ref() {
                    if let Some(entry) = hs.current_entry(&self.input_history) {
                        self.input = entry.to_string();
                        self.cursor_pos = self.input.len();
                    }
                }
                self.history_search = None;
                self.history_search_overlay.close();
                false
            }
            "cancel" => {
                self.history_search = None;
                self.history_search_overlay.close();
                false
            }
            "prevResult" => {
                if let Some(hs) = self.history_search.as_mut() {
                    if hs.selected > 0 {
                        hs.selected -= 1;
                    }
                }
                self.history_search_overlay.select_prev();
                false
            }
            "nextResult" => {
                if let Some(hs) = self.history_search.as_mut() {
                    let max = hs.matches.len().saturating_sub(1);
                    if hs.selected < max {
                        hs.selected += 1;
                    }
                }
                self.history_search_overlay.select_next();
                false
            }
            _ => false,
        }
    }

    /// Handle a key event while in legacy history-search mode.
    fn handle_history_search_key(&mut self, key: KeyEvent) -> bool {
        let hs = match self.history_search.as_mut() {
            Some(h) => h,
            None => return false,
        };
        match key.code {
            KeyCode::Esc => {
                self.history_search = None;
                self.history_search_overlay.close();
            }
            KeyCode::Enter => {
                if let Some(entry) = hs.current_entry(&self.input_history) {
                    self.input = entry.to_string();
                    self.cursor_pos = self.input.len();
                }
                self.history_search = None;
                self.history_search_overlay.close();
            }
            KeyCode::Up => {
                if hs.selected > 0 {
                    hs.selected -= 1;
                }
            }
            KeyCode::Down => {
                let max = hs.matches.len().saturating_sub(1);
                if hs.selected < max {
                    hs.selected += 1;
                }
            }
            KeyCode::Backspace => {
                hs.query.pop();
                let history = self.input_history.clone();
                if let Some(hs) = self.history_search.as_mut() {
                    hs.update_matches(&history);
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                hs.query.push(c);
                let history = self.input_history.clone();
                if let Some(hs) = self.history_search.as_mut() {
                    hs.update_matches(&history);
                }
            }
            _ => {}
        }
        false
    }

    /// Handle a key event while a permission dialog is active.
    fn handle_permission_key(&mut self, key: KeyEvent) {
        let pr = match self.permission_request.as_mut() {
            Some(p) => p,
            None => return,
        };

        match key.code {
            KeyCode::Char(c) => {
                if let Some(digit) = c.to_digit(10) {
                    let idx = (digit as usize).saturating_sub(1);
                    if idx < pr.options.len() {
                        pr.selected_option = idx;
                    }
                } else {
                    for (i, opt) in pr.options.iter().enumerate() {
                        if opt.key == c {
                            pr.selected_option = i;
                            self.permission_request = None;
                            return;
                        }
                    }
                }
            }
            KeyCode::Enter => {
                self.permission_request = None;
            }
            KeyCode::Up => {
                let pr = self.permission_request.as_mut().unwrap();
                if pr.selected_option > 0 {
                    pr.selected_option -= 1;
                }
            }
            KeyCode::Down => {
                let pr = self.permission_request.as_mut().unwrap();
                if pr.selected_option + 1 < pr.options.len() {
                    pr.selected_option += 1;
                }
            }
            KeyCode::Esc => {
                self.permission_request = None;
            }
            _ => {}
        }
    }

    // -------------------------------------------------------------------
    // Query event handling
    // -------------------------------------------------------------------

    /// Process a query event from the agentic loop.
    pub fn handle_query_event(&mut self, event: QueryEvent) {
        match event {
            QueryEvent::Stream(stream_evt) => {
                self.is_streaming = true;
                match stream_evt {
                    cc_api::StreamEvent::ContentBlockDelta { delta, .. } => match delta {
                        cc_api::streaming::ContentDelta::TextDelta { text } => {
                            self.streaming_text.push_str(&text);
                        }
                        cc_api::streaming::ContentDelta::ThinkingDelta { thinking } => {
                            debug!(len = thinking.len(), "Thinking delta received");
                        }
                        _ => {}
                    },
                    cc_api::StreamEvent::MessageStop => {
                        self.is_streaming = false;
                        if !self.streaming_text.is_empty() {
                            let text = std::mem::take(&mut self.streaming_text);
                            self.messages.push(Message::assistant(text));
                        }
                    }
                    _ => {}
                }
            }

            QueryEvent::ToolStart { tool_name, tool_id } => {
                self.is_streaming = true;
                self.status_message = Some(format!("Running {}…", tool_name));
                if let Some(existing) =
                    self.tool_use_blocks.iter_mut().find(|b| b.id == tool_id)
                {
                    existing.status = ToolStatus::Running;
                    existing.output_preview = None;
                } else {
                    self.tool_use_blocks.push(ToolUseBlock {
                        id: tool_id,
                        name: tool_name,
                        status: ToolStatus::Running,
                        output_preview: None,
                    });
                }
            }

            QueryEvent::ToolEnd {
                tool_name: _,
                tool_id,
                result,
                is_error,
            } => {
                let preview = if result.len() > 120 {
                    format!("{}…", &result[..120])
                } else {
                    result.clone()
                };
                if let Some(block) =
                    self.tool_use_blocks.iter_mut().find(|b| b.id == tool_id)
                {
                    block.status = if is_error {
                        ToolStatus::Error
                    } else {
                        ToolStatus::Done
                    };
                    block.output_preview = Some(preview);
                }
                if is_error {
                    self.status_message = Some(format!("Tool error: {}", result));
                } else {
                    self.status_message = None;
                }
            }

            QueryEvent::TurnComplete { turn, stop_reason } => {
                debug!(turn, stop_reason, "Turn complete");
                self.is_streaming = false;
                if !self.streaming_text.is_empty() {
                    let text = std::mem::take(&mut self.streaming_text);
                    self.messages.push(Message::assistant(text));
                }
                self.tool_use_blocks.retain(|b| b.status != ToolStatus::Running);
            }

            QueryEvent::Status(msg) => {
                self.status_message = Some(msg);
            }

            QueryEvent::Error(msg) => {
                self.is_streaming = false;
                self.streaming_text.clear();
                self.messages
                    .push(Message::assistant(format!("Error: {}", msg)));
                self.status_message = Some(format!("Error: {}", msg));
            }
        }
    }

    // -------------------------------------------------------------------
    // Main run loop
    // -------------------------------------------------------------------

    /// Run the TUI event loop. Returns `Some(input)` when the user submits
    /// a message, or `None` when the user quits.
    pub fn run(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> anyhow::Result<Option<String>> {
        loop {
            self.frame_count = self.frame_count.wrapping_add(1);

            // Sync cost/token counters from the shared tracker
            self.cost_usd = self.cost_tracker.total_cost_usd();
            self.token_count = self.cost_tracker.total_tokens() as u32;

            // Expire old notifications
            self.notifications.tick();

            // Draw the frame
            terminal.draw(|f| render::render_app(f, self))?;

            // Poll for events with a short timeout so we can redraw for animation
            if event::poll(std::time::Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    let should_submit = self.handle_key_event(key);
                    if self.should_quit {
                        return Ok(None);
                    }
                    if should_submit {
                        // Check if this is a slash command that should open a UI screen
                        if crate::input::is_slash_command(&self.input) {
                            let cmd = {
                                let (c, _) = crate::input::parse_slash_command(&self.input);
                                c.to_string()
                            };
                            if self.intercept_slash_command(&cmd) {
                                self.input.clear();
                                self.cursor_pos = 0;
                                continue;
                            }
                        }
                        let input = self.take_input();
                        if !input.is_empty() {
                            return Ok(Some(input));
                        }
                    }
                }
            }
        }
    }

    // -------------------------------------------------------------------
    // UTF-8 cursor helpers
    // -------------------------------------------------------------------

    fn char_boundary_at(&self, pos: usize) -> usize {
        let len = self.input.len();
        let pos = pos.min(len);
        let mut p = pos;
        while p < len && !self.input.is_char_boundary(p) {
            p += 1;
        }
        p
    }

    fn prev_char_boundary(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        let mut p = pos - 1;
        while p > 0 && !self.input.is_char_boundary(p) {
            p -= 1;
        }
        p
    }

    fn next_char_boundary(&self, pos: usize) -> usize {
        let len = self.input.len();
        if pos >= len {
            return len;
        }
        let mut p = pos + 1;
        while p < len && !self.input.is_char_boundary(p) {
            p += 1;
        }
        p
    }
}
