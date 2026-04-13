//! Configurable keyboard shortcuts system

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// All keybinding contexts
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum KeyContext {
    Global,
    Chat,
    Autocomplete,
    Confirmation,
    Help,
    Transcript,
    HistorySearch,
    Task,
    ThemePicker,
    Settings,
    Tabs,
    Attachments,
    Footer,
    MessageSelector,
    DiffDialog,
    ModelPicker,
    Select,
    Plugin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedKeystroke {
    pub key: String, // normalized key name
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

pub type Chord = Vec<ParsedKeystroke>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedBinding {
    pub chord: Chord,
    pub action: Option<String>, // None = unbound
    pub context: KeyContext,
}

/// Parse a keystroke string like "ctrl+shift+enter" into ParsedKeystroke
pub fn parse_keystroke(s: &str) -> Option<ParsedKeystroke> {
    let s = s.trim().to_lowercase();
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut meta = false;
    let mut key_parts: Vec<&str> = Vec::new();

    for part in s.split('+') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part {
            "ctrl" | "control" => ctrl = true,
            "alt" | "opt" | "option" => alt = true,
            "shift" => shift = true,
            "meta" | "cmd" | "command" | "super" | "win" => meta = true,
            _ => key_parts.push(part),
        }
    }

    if key_parts.is_empty() {
        return None;
    }

    let key = normalize_key(key_parts.join("+").as_str());
    Some(ParsedKeystroke {
        key,
        ctrl,
        alt,
        shift,
        meta,
    })
}

fn normalize_key(k: &str) -> String {
    match k {
        "esc" | "escape" => "escape".to_string(),
        "return" | "enter" => "enter".to_string(),
        "del" | "delete" => "delete".to_string(),
        "backspace" | "bs" => "backspace".to_string(),
        "space" | " " => "space".to_string(),
        "up" => "up".to_string(),
        "down" => "down".to_string(),
        "left" => "left".to_string(),
        "right" => "right".to_string(),
        "pageup" | "pgup" => "pageup".to_string(),
        "pagedown" | "pgdn" | "pgdown" => "pagedown".to_string(),
        "home" => "home".to_string(),
        "end" => "end".to_string(),
        "tab" => "tab".to_string(),
        k => k.to_string(),
    }
}

/// Parse a chord (space-separated keystrokes like "ctrl+k ctrl+d")
pub fn parse_chord(s: &str) -> Option<Chord> {
    let keystrokes: Vec<ParsedKeystroke> =
        s.split_whitespace().filter_map(parse_keystroke).collect();
    if keystrokes.is_empty() {
        None
    } else {
        Some(keystrokes)
    }
}

/// Keys that cannot be rebound
pub const NON_REBINDABLE: &[&str] = &["ctrl+c", "ctrl+d", "ctrl+m"];

/// Default keybindings with comprehensive coverage of text editing, navigation, vim, and TUI actions
///
/// # Standard Keybindings (Phase 1 Implementation)
/// - **Ctrl+L**: Clear current input line (like bash) [Chat context only due to conflict]
/// - **Ctrl+A**: Open the model picker
/// - **Ctrl+K**: Open the command palette
/// - **Ctrl+U**: Kill input from cursor to start of line (Emacs-style)
/// - **Alt+←/Alt+→**: Navigate to previous/next message in transcript
/// - **Ctrl+. (Ctrl+>)**: Jump to next error/issue in messages
/// - **Ctrl+Shift+.**: Jump to previous error/issue
/// - **Ctrl+M**: Send message (alternative to Enter)
/// - **Shift+Tab**: Reverse indent/unindent in input (cycle permission mode)
/// - **Ctrl+H**: Delete character before cursor (Chat context, Emacs-style)
/// - **Alt+H**: Open help (alternative to F1)
/// - **Ctrl+O**: Jump back in history (command history)
/// - **Ctrl+I**: Jump forward in history
/// - **Alt+D**: Delete word forward (already implemented)
/// - **Ctrl+V**: Paste from clipboard (already implemented)
pub fn default_bindings() -> Vec<ParsedBinding> {
    let defaults: &[(&str, &str, KeyContext)] = &[
        // ========== GLOBAL CONTROL ==========
        ("ctrl+c", "interrupt", KeyContext::Global),
        ("ctrl+d", "exit", KeyContext::Global),
        ("ctrl+l", "redraw", KeyContext::Global),
        ("ctrl+r", "historySearch", KeyContext::Global),
        ("ctrl+b", "createBranch", KeyContext::Global),
        ("alt+h", "openHelp", KeyContext::Global),
        // ========== CHAT / INPUT CONTEXT ==========
        ("enter", "submit", KeyContext::Chat),
        ("up", "historyPrev", KeyContext::Chat),
        ("down", "historyNext", KeyContext::Chat),
        ("shift+tab", "reverseIndent", KeyContext::Chat),
        ("pageup", "scrollUp", KeyContext::Chat),
        ("pagedown", "scrollDown", KeyContext::Chat),
        ("tab", "indent", KeyContext::Chat),
        ("shift+enter", "newline", KeyContext::Chat),
        ("home", "goLineStart", KeyContext::Chat),
        ("end", "goLineEnd", KeyContext::Chat),
        // Text Editing (Emacs-style) + app shortcuts
        ("ctrl+a", "openModelPicker", KeyContext::Chat),
        ("ctrl+e", "goLineEnd", KeyContext::Chat),
        ("ctrl+h", "deleteCharBefore", KeyContext::Chat),
        ("ctrl+k", "openCommandPalette", KeyContext::Chat),
        ("ctrl+u", "killToStart", KeyContext::Chat),
        ("ctrl+w", "killWord", KeyContext::Chat),
        ("alt+d", "deleteWord", KeyContext::Chat),
        ("alt+backspace", "killWord", KeyContext::Chat),
        // New Text Editing & Navigation
        ("ctrl+m", "sendMessage", KeyContext::Chat),
        ("ctrl+l", "clearLine", KeyContext::Chat),
        ("ctrl+.", "jumpToNextError", KeyContext::Chat),
        ("ctrl+shift+.", "jumpToPreviousError", KeyContext::Chat),
        ("alt+left", "previousMessage", KeyContext::Chat),
        ("alt+right", "nextMessage", KeyContext::Chat),
        ("ctrl+o", "historyPrev", KeyContext::Chat),
        ("ctrl+i", "historyNext", KeyContext::Chat),
        // Searching
        ("ctrl+f", "findInMessage", KeyContext::Chat),
        ("ctrl+shift+f", "globalSearch", KeyContext::Chat),
        ("ctrl+g", "goToLine", KeyContext::Chat),
        ("f3", "findNext", KeyContext::Chat),
        ("ctrl+]", "findNext", KeyContext::Chat),
        ("shift+f3", "findPrev", KeyContext::Chat),
        ("ctrl+[", "findPrev", KeyContext::Chat),
        // ========== CONFIRMATION DIALOGS ==========
        ("y", "yes", KeyContext::Confirmation),
        ("enter", "yes", KeyContext::Confirmation),
        ("n", "no", KeyContext::Confirmation),
        ("escape", "no", KeyContext::Confirmation),
        ("up", "prevOption", KeyContext::Confirmation),
        ("down", "nextOption", KeyContext::Confirmation),
        // ========== HELP OVERLAY ==========
        ("escape", "close", KeyContext::Help),
        ("q", "close", KeyContext::Help),
        ("up", "scrollUp", KeyContext::Help),
        ("down", "scrollDown", KeyContext::Help),
        ("pageup", "pageUp", KeyContext::Help),
        ("pagedown", "pageDown", KeyContext::Help),
        // ========== HISTORY SEARCH ==========
        ("enter", "select", KeyContext::HistorySearch),
        ("escape", "cancel", KeyContext::HistorySearch),
        ("up", "prevResult", KeyContext::HistorySearch),
        ("down", "nextResult", KeyContext::HistorySearch),
        ("tab", "togglePreview", KeyContext::HistorySearch),
        // ========== TRANSCRIPT / MESSAGE SELECTION ==========
        ("up", "prevMessage", KeyContext::Transcript),
        ("down", "nextMessage", KeyContext::Transcript),
        ("pageup", "pageUp", KeyContext::Transcript),
        ("pagedown", "pageDown", KeyContext::Transcript),
        ("home", "goStart", KeyContext::Transcript),
        ("end", "goEnd", KeyContext::Transcript),
        ("enter", "selectMessage", KeyContext::Transcript),
        ("escape", "cancel", KeyContext::Transcript),
        // ========== MESSAGE SELECTOR OVERLAY ==========
        ("up", "prevMessage", KeyContext::MessageSelector),
        ("down", "nextMessage", KeyContext::MessageSelector),
        ("enter", "select", KeyContext::MessageSelector),
        ("escape", "cancel", KeyContext::MessageSelector),
        ("j", "nextMessage", KeyContext::MessageSelector),
        ("k", "prevMessage", KeyContext::MessageSelector),
        // ========== THEME & MODEL PICKERS ==========
        ("up", "prev", KeyContext::ThemePicker),
        ("down", "next", KeyContext::ThemePicker),
        ("pageup", "pageUp", KeyContext::ThemePicker),
        ("pagedown", "pageDown", KeyContext::ThemePicker),
        ("enter", "select", KeyContext::ThemePicker),
        ("escape", "cancel", KeyContext::ThemePicker),
        ("j", "next", KeyContext::ThemePicker),
        ("k", "prev", KeyContext::ThemePicker),
        // ========== TASK LIST ==========
        ("up", "prevTask", KeyContext::Task),
        ("down", "nextTask", KeyContext::Task),
        ("enter", "selectTask", KeyContext::Task),
        ("escape", "closeTask", KeyContext::Task),
        ("x", "toggleDone", KeyContext::Task),
        // ========== DIFF DIALOG ==========
        ("up", "prevDiff", KeyContext::DiffDialog),
        ("down", "nextDiff", KeyContext::DiffDialog),
        ("pageup", "pageUp", KeyContext::DiffDialog),
        ("pagedown", "pageDown", KeyContext::DiffDialog),
        ("enter", "acceptDiff", KeyContext::DiffDialog),
        ("escape", "rejectDiff", KeyContext::DiffDialog),
        ("r", "rejectDiff", KeyContext::DiffDialog),
        ("a", "acceptDiff", KeyContext::DiffDialog),
        // ========== MODAL SELECT (Generic) ==========
        ("up", "prev", KeyContext::Select),
        ("down", "next", KeyContext::Select),
        ("pageup", "pageUp", KeyContext::Select),
        ("pagedown", "pageDown", KeyContext::Select),
        ("enter", "select", KeyContext::Select),
        ("escape", "cancel", KeyContext::Select),
        ("j", "next", KeyContext::Select),
        ("k", "prev", KeyContext::Select),
        ("/", "search", KeyContext::Select),
        // ========== PLUGIN & ATTACHMENTS ==========
        ("up", "prev", KeyContext::Plugin),
        ("down", "next", KeyContext::Plugin),
        ("enter", "select", KeyContext::Plugin),
        ("escape", "cancel", KeyContext::Plugin),
        ("space", "toggle", KeyContext::Attachments),
        ("a", "addAttachment", KeyContext::Attachments),
        ("r", "removeAttachment", KeyContext::Attachments),
    ];

    defaults
        .iter()
        .filter_map(|(chord_str, action, context)| {
            parse_chord(chord_str).map(|chord| ParsedBinding {
                chord,
                action: Some(action.to_string()),
                context: context.clone(),
            })
        })
        .collect()
}

/// User keybindings loaded from ~/.claurst/keybindings.json
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserKeybindings {
    pub bindings: Vec<UserBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonKeybindingConfig {
    #[serde(default)]
    bindings: Vec<JsonKeybindingBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonKeybindingBlock {
    context: String,
    bindings: IndexMap<String, Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserBinding {
    pub chord: String,          // e.g. "ctrl+k ctrl+d"
    pub action: Option<String>, // None = unbound
    pub context: Option<String>,
}

impl UserKeybindings {
    pub fn from_json_str(content: &str) -> Self {
        serde_json::from_str(content)
            .or_else(|_| Self::from_block_config(content))
            .unwrap_or_default()
    }

    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join("keybindings.json");
        if let Ok(content) = std::fs::read_to_string(&path) {
            Self::from_json_str(&content)
        } else {
            Self::default()
        }
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        let path = config_dir.join("keybindings.json");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    fn from_block_config(content: &str) -> Result<Self, serde_json::Error> {
        let config: JsonKeybindingConfig = serde_json::from_str(content)?;
        let bindings = config
            .bindings
            .into_iter()
            .flat_map(|block| {
                let context = block.context;
                block
                    .bindings
                    .into_iter()
                    .map(move |(chord, action)| UserBinding {
                        chord,
                        action,
                        context: Some(context.clone()),
                    })
            })
            .collect();
        Ok(Self { bindings })
    }
}

/// Resolved keybindings (defaults merged with user overrides)
pub struct KeybindingResolver {
    bindings: Vec<ParsedBinding>,
    pending_chord: Vec<ParsedKeystroke>,
}

impl KeybindingResolver {
    pub fn new(user: &UserKeybindings) -> Self {
        let mut bindings = default_bindings();

        // Apply user overrides (user bindings win, last match wins)
        for user_binding in &user.bindings {
            if let Some(chord) = parse_chord(&user_binding.chord) {
                let context = user_binding
                    .context
                    .as_deref()
                    .and_then(|c| serde_json::from_str(&format!("\"{}\"", c)).ok())
                    .unwrap_or(KeyContext::Global);

                bindings.push(ParsedBinding {
                    chord,
                    action: user_binding.action.clone(),
                    context,
                });
            }
        }

        Self {
            bindings,
            pending_chord: Vec::new(),
        }
    }

    /// Process a keystroke, returns action if binding matches
    pub fn process(
        &mut self,
        keystroke: ParsedKeystroke,
        context: &KeyContext,
    ) -> KeybindingResult {
        self.pending_chord.push(keystroke);

        // Find matching bindings in current context + Global
        let matches: Vec<&ParsedBinding> = self
            .bindings
            .iter()
            .filter(|b| &b.context == context || b.context == KeyContext::Global)
            .filter(|b| b.chord.starts_with(self.pending_chord.as_slice()))
            .collect();

        if matches.is_empty() {
            self.pending_chord.clear();
            return KeybindingResult::NoMatch;
        }

        let exact: Vec<&ParsedBinding> = matches
            .iter()
            .copied()
            .filter(|b| b.chord.len() == self.pending_chord.len())
            .collect();

        if !exact.is_empty() {
            // Last match wins (user overrides)
            let binding = exact.last().unwrap();
            self.pending_chord.clear();
            return match &binding.action {
                Some(action) => KeybindingResult::Action(action.clone()),
                None => KeybindingResult::Unbound,
            };
        }

        // Chord in progress
        KeybindingResult::Pending
    }

    pub fn cancel_chord(&mut self) {
        self.pending_chord.clear();
    }

    pub fn has_pending_chord(&self) -> bool {
        !self.pending_chord.is_empty()
    }
}

impl PartialEq for ParsedKeystroke {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.ctrl == other.ctrl
            && self.alt == other.alt
            && self.shift == other.shift
            && self.meta == other.meta
    }
}

#[derive(Debug, Clone)]
pub enum KeybindingResult {
    Action(String),
    Unbound,
    Pending,
    NoMatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_keystroke_simple() {
        let ks = parse_keystroke("enter").unwrap();
        assert_eq!(ks.key, "enter");
        assert!(!ks.ctrl);
        assert!(!ks.alt);
        assert!(!ks.shift);
        assert!(!ks.meta);
    }

    #[test]
    fn test_parse_keystroke_ctrl_c() {
        let ks = parse_keystroke("ctrl+c").unwrap();
        assert_eq!(ks.key, "c");
        assert!(ks.ctrl);
        assert!(!ks.alt);
    }

    #[test]
    fn test_parse_keystroke_ctrl_shift_enter() {
        let ks = parse_keystroke("ctrl+shift+enter").unwrap();
        assert_eq!(ks.key, "enter");
        assert!(ks.ctrl);
        assert!(ks.shift);
        assert!(!ks.alt);
    }

    #[test]
    fn test_parse_keystroke_normalizes_esc() {
        let ks = parse_keystroke("esc").unwrap();
        assert_eq!(ks.key, "escape");
    }

    #[test]
    fn test_parse_keystroke_normalizes_return() {
        let ks = parse_keystroke("return").unwrap();
        assert_eq!(ks.key, "enter");
    }

    #[test]
    fn test_parse_keystroke_empty_returns_none() {
        assert!(parse_keystroke("ctrl+").is_none());
        assert!(parse_keystroke("").is_none());
    }

    #[test]
    fn test_parse_chord_single() {
        let chord = parse_chord("ctrl+c").unwrap();
        assert_eq!(chord.len(), 1);
        assert_eq!(chord[0].key, "c");
        assert!(chord[0].ctrl);
    }

    #[test]
    fn test_parse_chord_multi() {
        let chord = parse_chord("ctrl+k ctrl+d").unwrap();
        assert_eq!(chord.len(), 2);
        assert_eq!(chord[0].key, "k");
        assert_eq!(chord[1].key, "d");
        assert!(chord[0].ctrl);
        assert!(chord[1].ctrl);
    }

    #[test]
    fn test_parse_chord_empty_returns_none() {
        assert!(parse_chord("").is_none());
    }

    #[test]
    fn test_default_bindings_not_empty() {
        let bindings = default_bindings();
        assert!(!bindings.is_empty());
    }

    #[test]
    fn test_default_bindings_contains_ctrl_c() {
        let bindings = default_bindings();
        let ctrl_c = bindings.iter().find(|b| {
            b.chord.len() == 1
                && b.chord[0].ctrl
                && b.chord[0].key == "c"
                && b.context == KeyContext::Global
        });
        assert!(ctrl_c.is_some());
        assert_eq!(ctrl_c.unwrap().action.as_deref(), Some("interrupt"));
    }

    #[test]
    fn test_default_bindings_map_ctrl_a_and_ctrl_k_to_app_shortcuts() {
        let bindings = default_bindings();

        let ctrl_a = bindings.iter().find(|b| {
            b.chord.len() == 1
                && b.chord[0].ctrl
                && b.chord[0].key == "a"
                && b.context == KeyContext::Chat
        });
        let ctrl_k = bindings.iter().find(|b| {
            b.chord.len() == 1
                && b.chord[0].ctrl
                && b.chord[0].key == "k"
                && b.context == KeyContext::Chat
        });

        assert_eq!(
            ctrl_a.and_then(|b| b.action.as_deref()),
            Some("openModelPicker")
        );
        assert_eq!(
            ctrl_k.and_then(|b| b.action.as_deref()),
            Some("openCommandPalette")
        );
    }

    #[test]
    fn test_resolver_simple_action() {
        let user = UserKeybindings::default();
        let mut resolver = KeybindingResolver::new(&user);
        let ks = parse_keystroke("ctrl+c").unwrap();
        let result = resolver.process(ks, &KeyContext::Global);
        assert!(matches!(result, KeybindingResult::Action(ref a) if a == "interrupt"));
    }

    #[test]
    fn test_resolver_no_match() {
        let user = UserKeybindings::default();
        let mut resolver = KeybindingResolver::new(&user);
        // ctrl+z has no default binding
        let ks = parse_keystroke("ctrl+z").unwrap();
        let result = resolver.process(ks, &KeyContext::Chat);
        assert!(matches!(result, KeybindingResult::NoMatch));
    }

    #[test]
    fn test_resolver_context_match_global_from_chat() {
        let user = UserKeybindings::default();
        let mut resolver = KeybindingResolver::new(&user);
        // ctrl+l in Chat context maps to "clearLine" (newly added Phase 1 keybinding)
        // Global context is checked after context-specific bindings
        let ks = parse_keystroke("ctrl+l").unwrap();
        let result = resolver.process(ks, &KeyContext::Chat);
        assert!(matches!(result, KeybindingResult::Action(ref a) if a == "clearLine"));
    }

    #[test]
    fn test_keystroke_equality() {
        let ks1 = parse_keystroke("ctrl+enter").unwrap();
        let ks2 = parse_keystroke("ctrl+enter").unwrap();
        let ks3 = parse_keystroke("shift+enter").unwrap();
        assert_eq!(ks1, ks2);
        assert_ne!(ks1, ks3);
    }

    #[test]
    fn test_user_keybindings_default_empty() {
        let user = UserKeybindings::default();
        assert!(user.bindings.is_empty());
    }

    #[test]
    fn test_user_keybindings_supports_ts_block_format() {
        let user = UserKeybindings::from_json_str(
            r#"{
  "bindings": [
    {
      "context": "Chat",
      "bindings": {
        "ctrl+g": "chat:externalEditor",
        "space": null
      }
    }
  ]
}"#,
        );

        assert_eq!(user.bindings.len(), 2);
        assert_eq!(user.bindings[0].context.as_deref(), Some("Chat"));
        assert_eq!(user.bindings[0].chord, "ctrl+g");
        assert_eq!(
            user.bindings[0].action.as_deref(),
            Some("chat:externalEditor")
        );
        assert_eq!(user.bindings[1].chord, "space");
        assert_eq!(user.bindings[1].action, None);
    }

    #[test]
    fn test_new_phase1_keybindings_registered() {
        // Verify that all Phase 1 keybindings are registered
        let bindings = default_bindings();

        // Build list of keybinding actions
        let actions: Vec<String> = bindings.iter().filter_map(|b| b.action.clone()).collect();

        // Check Phase 1 keybinding actions exist
        assert!(
            actions.contains(&"clearLine".to_string()),
            "clearLine action not found"
        );
        assert!(
            actions.contains(&"sendMessage".to_string()),
            "sendMessage action not found"
        );
        assert!(
            actions.contains(&"jumpToNextError".to_string()),
            "jumpToNextError action not found"
        );
        assert!(
            actions.contains(&"jumpToPreviousError".to_string()),
            "jumpToPreviousError action not found"
        );
        assert!(
            actions.contains(&"previousMessage".to_string()),
            "previousMessage action not found"
        );
        assert!(
            actions.contains(&"nextMessage".to_string()),
            "nextMessage action not found"
        );
        assert!(
            actions.contains(&"openHelp".to_string()),
            "openHelp action not found"
        );
        assert!(
            actions.contains(&"deleteCharBefore".to_string()),
            "deleteCharBefore action not found"
        );
        assert!(
            actions.contains(&"reverseIndent".to_string()),
            "reverseIndent action not found"
        );

        // Verify we have at least 10 new keybindings (Phase 1 requirement)
        assert!(
            actions.len() >= 40,
            "Expected at least 40 keybindings, found {}",
            actions.len()
        );
    }
}
