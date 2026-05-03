// app.rs — App state struct and main event loop.

use crate::bridge_state::BridgeConnectionState;
use crate::context_viz::ContextVizState;
use crate::dialog_select::{DialogSelectState, SelectItem};
use crate::export_dialog::{ExportDialogState, ExportFormat};
use crate::import_config_dialog::ImportConfigDialogState;
use crate::dialogs::PermissionRequest;
use crate::diff_viewer::{DiffViewerState, build_turn_diff};
use crate::model_picker::{EffortLevel, ModelPickerState};
use crate::session_browser::SessionBrowserState;
use crate::tasks_overlay::TasksOverlay;
use crate::dialogs::McpApprovalDialogState;
use crate::mcp_view::{McpServerView, McpToolView, McpViewState, McpViewStatus};
use crate::notifications::{NotificationKind, NotificationQueue};
use crate::overlays::{
    GlobalSearchState, HelpEntry, HelpOverlay, HistorySearchOverlay, MessageSelectorOverlay,
    RewindFlowOverlay, SelectorMessage,
};
use crate::plugin_views::PluginHintBanner;
use crate::privacy_screen::PrivacyScreen;
use crate::prompt_input::{InputMode, PromptInputState, VimMode};
use crate::render;
use crate::settings_screen::SettingsScreen;
use crate::stats_dialog::StatsDialogState;
use crate::theme_screen::ThemeScreen;
use crate::{agents_view::{AgentInfo, AgentStatus, AgentsMenuState, AgentsRoute}, diff_viewer::DiffPane};
use claurst_core::config::{Config, Settings, Theme};
use claurst_core::cost::CostTracker;
use claurst_core::file_history::FileHistory;
use claurst_core::keybindings::{
    KeyContext, KeybindingResolver, KeybindingResult, ParsedKeystroke, UserKeybindings,
};
use claurst_core::types::{ContentBlock, Message, Role};
use claurst_query::QueryEvent;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::style::Color;
use ratatui::Terminal;
use std::cell::{Cell, RefCell};
use std::io::Stdout;
use std::sync::{Arc, Mutex};
use tracing::debug;

const PROMPT_SLASH_COMMANDS: &[(&str, &str)] = &[
    ("advisor", "Set or unset the server-side advisor model"),
    ("agent", "List available agents or show agent details"),
    ("agents", "Browse agent definitions and active agents"),
    ("changes", "Inspect changes from the current session"),
    ("clear", "Clear the conversation transcript"),
    ("compact", "Compact the conversation context"),
    ("config", "Open settings"),
    ("connect", "Connect an AI provider"),
    ("context", "Show context window and rate limit usage"),
    ("copy", "Copy the last assistant response to clipboard"),
    ("cost", "Show cost breakdown"),
    ("diff", "Inspect the current git diff"),
    ("doctor", "Run diagnostics"),
    ("effort", "Set effort level (low/medium/high/max)"),
    ("exit", "Quit Claurst"),
    ("export", "Export conversation"),
    ("fast", "Toggle fast mode"),
    ("feedback", "Open session feedback survey"),
    ("fork", "Fork session into a new branch"),
    ("heapdump", "Show process memory and diagnostic information"),
    ("help", "Show help"),
    ("hooks", "Browse configured hooks (read-only)"),
    ("import-config", "Import CLAUDE.md and settings.json from ~/.claude"),
    ("init", "Initialize AGENTS.md for this project"),
    ("insights", "Generate a session analysis report with conversation statistics"),
    ("install-slack-app", "Install the Claurst Slack integration"),
    ("keybindings", "Show keybinding configuration"),
    ("login", "Log in to Claurst"),
    ("logout", "Log out of Claurst"),
    ("mcp", "Browse configured MCP servers"),
    ("memory", "Browse and open AGENTS.md memory files"),
    ("model", "Change the AI model"),
    ("output-style", "Toggle output style (auto/stream/verbose)"),
    ("plugin", "Manage plugins (list/info/enable/disable/reload)"),
    ("privacy", "Open privacy settings"),
    ("providers", "List available AI providers and their status"),
    ("caveman", "Caveman speech mode — save big token"),
    ("rocky", "Rocky speech mode — amaze amaze amaze"),
    ("normal", "Deactivate speech mode"),
    ("quit", "Quit Claurst"),
    ("refresh", "Clear saved provider auth and model caches"),
    ("rename", "Rename this session"),
    ("resume", "Resume a previous session"),
    ("review", "Review changes (git diff)"),
    ("rewind", "Rewind to an earlier turn"),
    ("session", "Browse and manage sessions"),
    ("settings", "Open settings"),
    ("stats", "Open token and cost stats"),
    ("survey", "Open session feedback survey"),
    ("theme", "Open the theme picker"),
    ("ultrareview", "Run an exhaustive multi-dimensional code review"),
    ("vim", "Toggle vim keybindings"),
    ("voice", "Toggle voice input mode"),
];

fn help_command_category(name: &str) -> &'static str {
    match name {
        "connect" | "model" | "providers" | "refresh" | "fast" | "effort" | "voice" => "Model & Provider",
        "changes" | "diff" | "review" | "rewind" | "export" | "copy" => "Review & History",
        "stats" | "cost" | "context" | "insights" | "heapdump" | "doctor" => "Diagnostics",
        "config" | "settings" | "theme" | "privacy" | "keybindings" | "hooks" | "mcp" | "import-config" => {
            "Workspace"
        }
        "agent" | "agents" | "memory" | "plugin" | "feedback" | "survey" => "Tools",
        "session" | "resume" | "rename" | "fork" | "clear" | "compact" | "quit" | "exit" => {
            "Session"
        }
        _ => "Commands",
    }
}

fn help_overlay_entries() -> Vec<HelpEntry> {
    PROMPT_SLASH_COMMANDS
        .iter()
        .map(|(name, description)| HelpEntry {
            name: (*name).to_string(),
            aliases: String::new(),
            description: (*description).to_string(),
            category: help_command_category(name).to_string(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Provider connection helpers
// ---------------------------------------------------------------------------

/// Return the environment variable name for a given provider ID.
#[allow(dead_code)]
fn get_env_var_for_provider(id: &str) -> &'static str {
    match id {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "google" | "google-vertex" => "GOOGLE_API_KEY",
        "github-copilot" => "GITHUB_TOKEN",
        "groq" => "GROQ_API_KEY",
        "cerebras" => "CEREBRAS_API_KEY",
        "sambanova" => "SAMBANOVA_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "togetherai" => "TOGETHER_API_KEY",
        "perplexity" => "PERPLEXITY_API_KEY",
        "cohere" => "COHERE_API_KEY",
        "xai" => "XAI_API_KEY",
        "deepinfra" => "DEEPINFRA_API_KEY",
        "azure" => "AZURE_API_KEY",
        "amazon-bedrock" => "AWS_ACCESS_KEY_ID",
        "sap-ai-core" => "AICORE_SERVICE_KEY",
        "gitlab" => "GITLAB_TOKEN",
        "cloudflare-ai-gateway" | "cloudflare-workers-ai" => "CLOUDFLARE_API_TOKEN",
        "vercel" => "AI_GATEWAY_API_KEY",
        "helicone" => "HELICONE_API_KEY",
        "huggingface" => "HF_TOKEN",
        "nvidia" => "NVIDIA_API_KEY",
        "alibaba" => "DASHSCOPE_API_KEY",
        "venice" => "VENICE_API_KEY",
        "moonshotai" => "MOONSHOT_API_KEY",
        "zhipuai" => "ZHIPU_API_KEY",
        "zai" => "ZAI_API_KEY",
        "siliconflow" => "SILICONFLOW_API_KEY",
        "nebius" => "NEBIUS_API_KEY",
        "novita" => "NOVITA_API_KEY",
        "minimax" => "MINIMAX_API_KEY",
        "ovhcloud" => "OVHCLOUD_API_KEY",
        "scaleway" => "SCALEWAY_API_KEY",
        "vultr" => "VULTR_API_KEY",
        "baseten" => "BASETEN_API_KEY",
        "friendli" => "FRIENDLI_TOKEN",
        "upstage" => "UPSTAGE_API_KEY",
        "stepfun" => "STEPFUN_API_KEY",
        "fireworks" => "FIREWORKS_API_KEY",
        _ => "API_KEY",
    }
}

/// Return a URL hint for obtaining an API key from a given provider.
#[allow(dead_code)]
fn get_url_for_provider(id: &str) -> &'static str {
    match id {
        "anthropic" => "console.anthropic.com",
        "openai" => "platform.openai.com/api-keys",
        "google" => "aistudio.google.com/apikey",
        "github-copilot" => "github.com/settings/tokens",
        "groq" => "console.groq.com/keys",
        "cerebras" => "cloud.cerebras.ai",
        "sambanova" => "cloud.sambanova.ai",
        "deepseek" => "platform.deepseek.com/api_keys",
        "mistral" => "console.mistral.ai/api-keys",
        "openrouter" => "openrouter.ai/keys",
        "togetherai" => "api.together.xyz/settings/api-keys",
        "perplexity" => "perplexity.ai/settings/api",
        "cohere" => "dashboard.cohere.com/api-keys",
        "xai" => "console.x.ai",
        "deepinfra" => "deepinfra.com/dash/api_keys",
        "azure" => "portal.azure.com",
        "amazon-bedrock" => "console.aws.amazon.com/bedrock",
        "minimax" => "platform.minimaxi.com",
        "huggingface" => "huggingface.co/settings/tokens",
        "nvidia" => "build.nvidia.com",
        "venice" => "venice.ai/settings/api",
        "zai" => "z.ai/manage-apikey/apikey-list",
        _ => "the provider's website",
    }
}


fn import_config_picker_items() -> Vec<SelectItem> {
    vec![
        SelectItem {
            id: "claude-md".into(),
            title: "CLAUDE.md".into(),
            description: "Import ~/.claude/CLAUDE.md".into(),
            category: "Import".into(),
            badge: None,
        },
        SelectItem {
            id: "settings".into(),
            title: "settings.json".into(),
            description: "Import ~/.claude/settings.json".into(),
            category: "Import".into(),
            badge: None,
        },
        SelectItem {
            id: "both".into(),
            title: "Both".into(),
            description: "Import both CLAUDE.md and settings.json".into(),
            category: "Import".into(),
            badge: Some("SAFE".into()),
        },
    ]
}

fn provider_picker_items() -> Vec<SelectItem> {
    vec![
        SelectItem { id: "openai".into(), title: "OpenAI".into(), description: "(API key)".into(), category: "Popular".into(), badge: None },
        SelectItem { id: "openai-codex".into(), title: "OpenAI Codex".into(), description: "(ChatGPT Plus/Pro — browser login)".into(), category: "Popular".into(), badge: None },
        SelectItem { id: "github-copilot".into(), title: "GitHub Copilot".into(), description: "(GitHub subscription or token)".into(), category: "Popular".into(), badge: None },
        SelectItem { id: "google".into(), title: "Google".into(), description: "(API key)".into(), category: "Popular".into(), badge: None },
        SelectItem { id: "anthropic".into(), title: "Anthropic".into(), description: "(API key)".into(), category: "Popular".into(), badge: None },
        SelectItem { id: "custom-openai".into(), title: "Custom OpenAI-Compatible".into(), description: "Custom URL + API key".into(), category: "Advanced".into(), badge: None },
        SelectItem { id: "openrouter".into(), title: "OpenRouter".into(), description: "100+ models with one key".into(), category: "Popular".into(), badge: None },
        SelectItem { id: "vercel".into(), title: "Vercel AI Gateway".into(), description: "Gateway for AI SDK models".into(), category: "Popular".into(), badge: None },
        SelectItem { id: "groq".into(), title: "Groq".into(), description: "Fast hosted inference".into(), category: "Popular".into(), badge: Some("FREE".into()) },
        SelectItem { id: "ollama".into(), title: "Ollama".into(), description: "Run models locally".into(), category: "Popular".into(), badge: Some("LOCAL".into()) },
        SelectItem { id: "zai".into(), title: "Z.AI".into(), description: "GLM-5.1 / GLM-5 / GLM-4.7 Coding Plan".into(), category: "Popular".into(), badge: None },
        SelectItem { id: "opencode-go".into(), title: "OpenCode Go".into(), description: "$10/mo flat-rate · Kimi · DeepSeek · GLM · MiniMax".into(), category: "Popular".into(), badge: None },
        SelectItem { id: "cerebras".into(), title: "Cerebras".into(), description: "Fast hosted inference".into(), category: "Other".into(), badge: Some("FREE".into()) },
        SelectItem { id: "sambanova".into(), title: "SambaNova".into(), description: "Fast hosted inference".into(), category: "Other".into(), badge: Some("FREE".into()) },
        SelectItem { id: "lmstudio".into(), title: "LM Studio".into(), description: "Local model server".into(), category: "Other".into(), badge: Some("LOCAL".into()) },
        SelectItem { id: "llamacpp".into(), title: "llama.cpp".into(), description: "Local inference server".into(), category: "Other".into(), badge: Some("LOCAL".into()) },
        SelectItem { id: "deepseek".into(), title: "DeepSeek".into(), description: "Reasoning and coding models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "mistral".into(), title: "Mistral".into(), description: "Hosted Mistral models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "togetherai".into(), title: "Together AI".into(), description: "Open model hosting".into(), category: "Other".into(), badge: None },
        SelectItem { id: "perplexity".into(), title: "Perplexity".into(), description: "Search-augmented models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "cohere".into(), title: "Cohere".into(), description: "Command models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "xai".into(), title: "xAI".into(), description: "Grok models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "deepinfra".into(), title: "DeepInfra".into(), description: "Hosted open models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "azure".into(), title: "Azure OpenAI".into(), description: "Enterprise OpenAI deployments".into(), category: "Other".into(), badge: None },
        SelectItem { id: "amazon-bedrock".into(), title: "AWS Bedrock".into(), description: "Enterprise foundation models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "google-vertex".into(), title: "Google Vertex AI".into(), description: "Enterprise Google models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "sap-ai-core".into(), title: "SAP AI Core".into(), description: "Enterprise AI platform".into(), category: "Other".into(), badge: None },
        SelectItem { id: "gitlab".into(), title: "GitLab Duo".into(), description: "AI in GitLab".into(), category: "Other".into(), badge: None },
        SelectItem { id: "cloudflare-ai-gateway".into(), title: "Cloudflare AI Gateway".into(), description: "Gateway for multiple providers".into(), category: "Other".into(), badge: None },
        SelectItem { id: "cloudflare-workers-ai".into(), title: "Cloudflare Workers AI".into(), description: "Edge AI inference".into(), category: "Other".into(), badge: None },
        SelectItem { id: "helicone".into(), title: "Helicone".into(), description: "AI gateway and observability".into(), category: "Other".into(), badge: None },
        SelectItem { id: "huggingface".into(), title: "Hugging Face".into(), description: "Hosted community models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "nvidia".into(), title: "NVIDIA".into(), description: "Hosted NVIDIA models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "alibaba".into(), title: "Alibaba".into(), description: "Qwen and hosted models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "venice".into(), title: "Venice AI".into(), description: "Privacy-first AI".into(), category: "Other".into(), badge: None },
        SelectItem { id: "moonshotai".into(), title: "Moonshot AI".into(), description: "Hosted Moonshot models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "zhipuai".into(), title: "Zhipu AI".into(), description: "Hosted GLM models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "siliconflow".into(), title: "SiliconFlow".into(), description: "Hosted open models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "nebius".into(), title: "Nebius".into(), description: "Cloud inference".into(), category: "Other".into(), badge: None },
        SelectItem { id: "novita".into(), title: "Novita".into(), description: "Cloud inference".into(), category: "Other".into(), badge: None },
        SelectItem { id: "minimax".into(), title: "MiniMax".into(), description: "Anthropic-compatible (M2.7)".into(), category: "Other".into(), badge: None },
        SelectItem { id: "ovhcloud".into(), title: "OVHcloud".into(), description: "EU-hosted AI".into(), category: "Other".into(), badge: None },
        SelectItem { id: "scaleway".into(), title: "Scaleway".into(), description: "EU cloud AI".into(), category: "Other".into(), badge: None },
        SelectItem { id: "vultr".into(), title: "Vultr".into(), description: "Cloud inference".into(), category: "Other".into(), badge: None },
        SelectItem { id: "baseten".into(), title: "Baseten".into(), description: "Model serving".into(), category: "Other".into(), badge: None },
        SelectItem { id: "friendli".into(), title: "Friendli".into(), description: "Serverless inference".into(), category: "Other".into(), badge: None },
        SelectItem { id: "upstage".into(), title: "Upstage".into(), description: "Hosted Upstage models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "stepfun".into(), title: "StepFun".into(), description: "Hosted reasoning models".into(), category: "Other".into(), badge: None },
        SelectItem { id: "fireworks".into(), title: "Fireworks AI".into(), description: "Fast inference".into(), category: "Other".into(), badge: None },
    ]
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Visual style for inline system messages in the conversation pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemMessageStyle {
    Info,
    Warning,
    /// Compact / auto-compact boundary marker.
    Compact,
}

/// A synthetic system annotation inserted between conversation messages.
/// `after_index` is the index in `App::messages` after which this annotation
/// should appear (0 = before all messages, 1 = after message 0, etc.).
#[derive(Debug, Clone)]
pub struct SystemAnnotation {
    pub after_index: usize,
    pub text: String,
    pub style: SystemMessageStyle,
}

/// A displayable item in the conversation pane — either a real message or
/// a synthetic system annotation (e.g. compact boundary).
/// Used only by `render.rs`; constructed on the fly from `messages` +
/// `system_annotations`.
#[derive(Debug, Clone)]
pub enum DisplayMessage {
    /// A real conversation turn.
    Conversation(Message),
    /// An injected system notice (e.g. compact boundary).
    System { text: String, style: SystemMessageStyle },
}

/// Context menu state: position and currently selected item index.
#[derive(Debug, Clone, Copy)]
pub struct ContextMenuState {
    /// X coordinate of the menu (column).
    pub x: u16,
    /// Y coordinate of the menu (row).
    pub y: u16,
    /// Currently selected menu item index (0-based).
    pub selected_index: usize,
    /// What the context menu is acting on.
    pub kind: ContextMenuKind,
}

/// What content the context menu is currently targeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuKind {
    /// A specific transcript message.
    Message { message_index: usize },
    /// The current text selection anywhere in the frame.
    Selection,
}

/// Available context menu items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuItem {
    Copy,
    Fork,
}

/// State for the Go to Line dialog (Ctrl+G in message pane).
#[derive(Debug, Clone)]
pub struct GoToLineDialog {
    /// Input field for line number.
    pub input: String,
    /// Whether the dialog is currently active.
    pub active: bool,
    /// Total number of lines (for validation feedback).
    pub total_lines: usize,
}

impl GoToLineDialog {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            active: false,
            total_lines: 0,
        }
    }

    pub fn open(&mut self, total_lines: usize) {
        self.input.clear();
        self.active = true;
        self.total_lines = total_lines;
    }

    pub fn close(&mut self) {
        self.active = false;
        self.input.clear();
    }

    /// Parse the input as a line number (1-indexed).
    /// Returns None if invalid or out of range.
    pub fn parse_line_number(&self) -> Option<usize> {
        let line_num: usize = self.input.trim().parse().ok()?;
        if line_num >= 1 && line_num <= self.total_lines {
            Some(line_num)
        } else {
            None
        }
    }
}

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
    pub turn_index: Option<usize>,
    pub status: ToolStatus,
    pub output_preview: Option<String>,
    /// JSON-serialised input for the tool call (populated from the API stream).
    pub input_json: String,
}

#[derive(Debug, Clone, Default)]
pub struct TurnMetadata {
    pub submitted_at: Option<String>,
    pub model_name: Option<String>,
    pub agent_mode: Option<String>,
    pub duration: Option<String>,
    pub interrupted: bool,
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

/// Attempt to copy text to the system clipboard using platform CLI tools.
/// Returns true if successful.
pub fn try_copy_to_clipboard(text: &str) -> bool {
    // Windows
    #[cfg(target_os = "windows")]
    {
        use std::io::Write;
        if let Ok(mut child) = std::process::Command::new("clip")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
                drop(stdin);
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
    }
    // macOS
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        if let Ok(mut child) = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
    }
    // Linux / Wayland / X11
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        for cmd in &["wl-copy", "xclip -selection clipboard", "xsel --clipboard --input"] {
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            if let Some((prog, args)) = parts.split_first() {
                if let Ok(mut child) = std::process::Command::new(prog)
                    .args(args)
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                {
                    if let Some(stdin) = child.stdin.as_mut() {
                        let _ = stdin.write_all(text.as_bytes());
                    }
                    if child.wait().map(|s| s.success()).unwrap_or(false) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Map a character to its QWERTY Latin keyboard-position equivalent.
///
/// When a modifier key (Ctrl, Alt) is held together with a non-ASCII character
/// (e.g. Cyrillic С on a Ukrainian/Russian layout), the char produced by
/// crossterm is the non-Latin glyph rather than the Latin letter that occupies
/// the same physical key.  Keybinding strings are always written as Latin
/// letters (`ctrl+c`, `alt+b`, …), so the lookup fails.
///
/// This function converts the reported character to the Latin letter that sits
/// at the same physical QWERTY position, covering the standard Russian JCUKEN
/// and Ukrainian layouts which share the same physical-key→Latin mapping.
/// For characters outside any known mapping the original (lowercased) char is
/// returned unchanged — this is always safe since unrecognised chars just
/// produce no keybinding match.
fn layout_to_latin(c: char) -> String {
    // Standard Russian/Ukrainian JCUKEN → QWERTY position mapping.
    // Both upper- and lower-case Cyrillic variants are covered by
    // converting to lowercase first.
    let lower = c.to_lowercase().next().unwrap_or(c);
    let mapped: Option<char> = match lower {
        // Row 1
        'й' => Some('q'), 'ц' => Some('w'), 'у' => Some('e'),
        'к' => Some('r'), 'е' => Some('t'), 'н' => Some('y'),
        'г' => Some('u'), 'ш' => Some('i'), 'щ' => Some('o'),
        'з' => Some('p'),
        // Row 2
        'ф' => Some('a'), 'ы' => Some('s'), 'в' => Some('d'),
        'а' => Some('f'), 'п' => Some('g'), 'р' => Some('h'),
        'о' => Some('j'), 'л' => Some('k'), 'д' => Some('l'),
        // Row 3
        'я' => Some('z'), 'ч' => Some('x'), 'с' => Some('c'),
        'м' => Some('v'), 'и' => Some('b'), 'т' => Some('n'),
        'ь' => Some('m'),
        // Ukrainian-specific letters on standard positions
        'і' => Some('s'), 'ї' => Some(']'), 'є' => Some('\''),
        _ => None,
    };
    mapped.unwrap_or(lower).to_string()
}

fn key_event_to_keystroke(key: &KeyEvent) -> Option<ParsedKeystroke> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt  = key.modifiers.contains(KeyModifiers::ALT);

    let normalized_key = match key.code {
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete    => "delete".to_string(),
        KeyCode::Down      => "down".to_string(),
        KeyCode::End       => "end".to_string(),
        KeyCode::Enter     => "enter".to_string(),
        KeyCode::Esc       => "escape".to_string(),
        KeyCode::Home      => "home".to_string(),
        KeyCode::Left      => "left".to_string(),
        KeyCode::PageDown  => "pagedown".to_string(),
        KeyCode::PageUp    => "pageup".to_string(),
        KeyCode::Right     => "right".to_string(),
        KeyCode::Tab       => "tab".to_string(),
        KeyCode::Up        => "up".to_string(),
        KeyCode::BackTab   => "tab".to_string(),
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => {
            // For modifier-key combos (Ctrl/Alt + letter), normalize to the
            // ASCII Latin key at the same physical QWERTY position.  This
            // makes shortcuts like Ctrl+C work regardless of the active
            // keyboard layout (Ukrainian, Russian, Greek, …).
            if (ctrl || alt) && !c.is_ascii() {
                layout_to_latin(c)
            } else {
                c.to_lowercase().to_string()
            }
        }
        _ => return None,
    };

    Some(ParsedKeystroke {
        key: normalized_key,
        ctrl,
        alt,
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
        meta: key.modifiers.contains(KeyModifiers::SUPER),
    })
}

// ---------------------------------------------------------------------------
// Focus target
// ---------------------------------------------------------------------------

/// Which area of the TUI currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    /// Keyboard input goes to the prompt editor.
    Input,
    /// Keyboard input goes to the transcript/message pane (scroll, etc.).
    Transcript,
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
    /// Combined display list kept in sync with `messages`: real conversation turns
    /// plus injected system annotations. Used by the renderer so it can iterate
    /// a single sequence instead of merging two lists on every frame.
    pub display_messages: Vec<DisplayMessage>,
    /// Synthetic system annotations interleaved between real messages at render time.
    pub system_annotations: Vec<SystemAnnotation>,
    pub input: String,
    pub prompt_input: PromptInputState,
    pub input_history: Vec<String>,
    pub history_index: Option<usize>,
    pub scroll_offset: usize,
    pub is_streaming: bool,
    pub streaming_text: String,
    pub streaming_thinking: String,
    pub status_message: Option<String>,
    /// Randomly chosen thinking verb shown next to the spinner while streaming.
    pub spinner_verb: Option<String>,
    pub should_quit: bool,
    pub show_help: bool,

    // Extended state
    pub tool_use_blocks: Vec<ToolUseBlock>,
    pub permission_request: Option<PermissionRequest>,
    pub frame_count: u64,
    pub token_count: u32,
    /// Maximum token budget (from env var or model context window) — P2 feature flag
    pub token_budget: Option<u32>,
    pub cost_usd: f64,
    pub model_name: String,
    /// Whether the app has valid API credentials configured.
    /// False = show the in-TUI provider setup dialog on startup.
    pub has_credentials: bool,
    /// Current effort level (controls extended-thinking budget_tokens).
    pub effort_level: EffortLevel,
    /// Whether fast mode is currently active (model locked to FAST_MODE_MODEL).
    pub fast_mode: bool,
    /// Active speech mode: None = normal, Some("caveman") / Some("rocky").
    pub speech_mode: Option<String>,
    /// Speech mode intensity: "lite", "full", "ultra".
    pub speech_level: String,
    /// Current agent mode name: "build", "plan", "explore", etc.
    pub agent_mode: Option<String>,
    /// Accent color derived from the current agent mode.
    /// Build = pink, Plan = blue, Explore = amber.
    pub accent_color: Color,
    /// Set by `cycle_agent_mode` so the main loop can update the query config
    /// and tool list to match the newly-selected agent.
    pub agent_mode_changed: bool,
    pub agent_status: Vec<(String, String)>,
    pub history_search: Option<HistorySearch>,
    pub keybindings: KeybindingResolver,

    // Cursor position within input (byte offset)
    pub cursor_pos: usize,

    // ---- Scrollback / auto-scroll -----------------------------------------

    /// When `true`, the message pane follows the latest messages automatically.
    pub auto_scroll: bool,
    /// Count of messages that arrived while the user was scrolled up.
    pub new_messages_while_scrolled: usize,

    // ---- Token warning tracking -------------------------------------------

    /// Which threshold (0 = none, 80, 95, 100) was last notified so we only
    /// show each banner once.
    pub token_warning_threshold_shown: u8,

    // ---- Session timing ---------------------------------------------------

    /// Instant the session started (used for elapsed-time in the status bar).
    pub session_start: std::time::Instant,
    /// Current Rustle pose for rendering (updated each frame).
    pub rustle_current_pose: crate::rustle::RustlePose,
    /// Temporary Rustle pose override (e.g. look-down on Tab). Reverts to
    /// default after this instant passes.
    pub rustle_pose_until: Option<std::time::Instant>,
    /// The temporary pose to show until `rustle_pose_until`.
    pub rustle_temp_pose: Option<crate::rustle::RustlePose>,
    /// Frame counter at which the next random eye-shift should fire.
    pub rustle_next_blink: u64,
    /// Instant the current turn's streaming began (reset each time streaming starts).
    pub turn_start: Option<std::time::Instant>,
    /// Elapsed time string for the last completed turn, e.g. "2m 5s".
    pub last_turn_elapsed: Option<String>,
    /// Past-tense verb shown after turn completes, e.g. "Worked" / "Baked".
    pub last_turn_verb: Option<&'static str>,
    /// Per-user turn snapshots used by the transcript renderer.
    pub turn_metadata: Vec<TurnMetadata>,
    /// Incremented whenever transcript-visible state changes so rendering can
    /// reuse cached layout between keystrokes.
    pub transcript_version: Cell<u64>,

    // ---- New overlay / notification fields --------------------------------

    /// Full-screen help overlay (? / F1).
    pub help_overlay: HelpOverlay,
    /// Ctrl+R history search overlay.
    pub history_search_overlay: HistorySearchOverlay,
    /// Global ripgrep search / quick-open overlay.
    pub global_search: GlobalSearchState,
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
    /// Live MCP manager snapshot source when available.
    pub mcp_manager: Option<Arc<claurst_mcp::McpManager>>,
    /// Queued request for a real MCP reconnect from the interactive loop.
    pub pending_mcp_reconnect: bool,
    /// Pending MCP panel-auth request for the interactive loop.
    pub pending_mcp_panel_auth: Option<String>,
    /// Shared file-history service used for turn diff reconstruction.
    pub file_history: Option<Arc<parking_lot::Mutex<FileHistory>>>,
    /// Shared query-loop turn counter for turn-local diff reconstruction.
    pub current_turn: Option<Arc<std::sync::atomic::AtomicUsize>>,

    // ---- Visual mode indicators -------------------------------------------

    /// Plan mode — input border turns blue, [PLAN] shown in status bar.
    pub plan_mode: bool,
    /// "While you were away" summary text shown on the welcome screen.
    pub away_summary: Option<String>,
    /// When streaming stalled (used to turn the spinner red after 3 s).
    pub stall_start: Option<std::time::Instant>,

    // ---- Settings / theme / privacy screens --------------------------------

    /// Full-screen tabbed settings screen (/config, /settings).
    pub settings_screen: SettingsScreen,
    /// Theme picker overlay (/theme).
    pub theme_screen: ThemeScreen,
    /// Privacy settings dialog (/privacy-settings).
    pub privacy_screen: PrivacyScreen,
    /// Token/cost analytics dialog.
    pub stats_dialog: StatsDialogState,
    /// MCP server browser and tool detail view.
    pub mcp_view: McpViewState,
    /// Agent definitions and active agent status overlay.
    pub agents_menu: AgentsMenuState,
    /// Diff viewer overlay.
    pub diff_viewer: DiffViewerState,
    /// Session-quality feedback survey overlay.
    pub feedback_survey: crate::feedback_survey::FeedbackSurveyState,
    /// Memory file selector overlay (AGENTS.md browser).
    pub memory_file_selector: crate::memory_file_selector::MemoryFileSelectorState,
    /// Read-only hooks configuration browser.
    pub hooks_config_menu: crate::hooks_config_menu::HooksConfigMenuState,
    /// Overage credit upsell banner.
    pub overage_upsell: crate::overage_upsell::OverageCreditUpsellState,
    /// Voice mode availability notice.
    pub voice_mode_notice: crate::voice_mode_notice::VoiceModeNoticeState,
    /// Desktop app upsell startup dialog.
    pub desktop_upsell: crate::desktop_upsell_startup::DesktopUpsellStartupState,
    /// Startup error dialog for malformed settings.json or AGENTS.md.
    pub invalid_config_dialog: crate::invalid_config_dialog::InvalidConfigDialogState,
    /// Memory update notification banner.
    pub memory_update_notification: crate::memory_update_notification::MemoryUpdateNotificationState,
    /// MCP elicitation dialog (form requested by an MCP server).
    pub elicitation: crate::elicitation_dialog::ElicitationDialogState,
    /// Model picker overlay (/model command).
    pub model_picker: ModelPickerState,
    /// Session browser overlay (/session, /resume, /rename, /export).
    pub session_browser: SessionBrowserState,
    /// Session branching overlay (Ctrl+B) — create and switch branches.
    pub session_branching: crate::session_branching::SessionBranchingState,
    /// Task progress overlay (Ctrl+T) — shows task status with toggle capability.
    pub tasks_overlay: TasksOverlay,
    /// Export format picker dialog (/export).
    pub export_dialog: ExportDialogState,
    /// Context window / rate limit visualization overlay (/context).
    pub context_viz: ContextVizState,
    /// MCP server approval dialog.
    pub mcp_approval: McpApprovalDialogState,
    /// Go to Line dialog (Ctrl+G in message pane).
    pub go_to_line_dialog: GoToLineDialog,
    /// Bypass-permissions startup confirmation dialog.
    /// Shown at startup when --dangerously-skip-permissions was passed.
    /// User must explicitly accept or the session exits.
    pub bypass_permissions_dialog: crate::bypass_permissions_dialog::BypassPermissionsDialogState,
    /// First-launch onboarding welcome dialog.
    pub onboarding_dialog: crate::onboarding_dialog::OnboardingDialogState,
    /// API key input dialog (opened from /connect for key-based providers).
    pub key_input_dialog: crate::key_input_dialog::KeyInputDialogState,
    /// Custom provider dialog for URL + API key input.
    pub custom_provider_dialog: crate::custom_provider_dialog::CustomProviderDialogState,
    /// Device code / browser auth dialog (GitHub Copilot device flow, Anthropic OAuth).
    pub device_auth_dialog: crate::device_auth_dialog::DeviceAuthDialogState,
    /// When set, the main loop should spawn the async auth task for this provider.
    pub device_auth_pending: Option<String>,
    /// Shared provider registry for dynamic model fetching.
    pub provider_registry: Option<std::sync::Arc<claurst_api::ProviderRegistry>>,
    /// Model registry populated from models.dev — single source of truth for
    /// all provider models shown in the `/model` picker.
    pub model_registry: claurst_api::ModelRegistry,
    /// When `true`, the main event loop should spawn an async task to fetch
    /// the model list from the current provider's `list_models()` API.
    pub model_picker_fetch_pending: bool,
    /// When `true`, the main event loop should spawn an async task to load
    /// the session list from disk and populate the session browser.
    pub session_list_pending: bool,
    /// Receiver for background session-list results.
    pub session_list_rx:
        Option<tokio::sync::mpsc::Receiver<Vec<crate::session_browser::SessionEntry>>>,
    /// Credential store for provider API keys and OAuth tokens.
    pub auth_store: claurst_core::AuthStore,
    /// Connect-a-provider dialog (/connect command).
    pub connect_dialog: DialogSelectState,
    /// Import-config source picker (/import-config command).
    pub import_config_picker: DialogSelectState,
    /// Import-config preview and confirmation dialog.
    pub import_config_dialog: ImportConfigDialogState,
    /// Ctrl+K command palette overlay.
    pub command_palette: DialogSelectState,
    /// Whether Claurst was launched from the user's home directory.
    /// Shown as a startup notice: "Note: You have launched claude in your home directory…"
    pub home_dir_warning: bool,
    /// Output style: "auto" | "stream" | "verbose".
    pub output_style: String,
    /// PR number for the current branch (None if not in a PR context).
    pub pr_number: Option<u32>,
    /// PR URL for the current branch.
    pub pr_url: Option<String>,
    /// PR review state: "approved", "changes_requested", "review_required", etc.
    pub pr_state: Option<String>,
    /// Count of in-progress background tasks (drives the footer pill).
    pub background_task_count: usize,
    /// Background task status text shown in footer pill.
    pub background_task_status: Option<String>,
    /// External status line command output (from CLAUDE_STATUS_COMMAND).
    pub status_line_override: Option<String>,
    /// Whether auto-compact is enabled (from settings).
    pub auto_compact_enabled: bool,
    /// Context threshold (0-100) at which to auto-compact.
    pub auto_compact_threshold: u8,
    /// Guard to prevent re-triggering auto-compact while one is in flight.
    pub auto_compact_running: bool,

    // ---- Voice hold-to-talk ------------------------------------------------

    /// The global voice recorder, Some when voice is enabled in config.
    pub voice_recorder: Option<Arc<Mutex<claurst_core::voice::VoiceRecorder>>>,
    /// True while recording is active (Alt+V toggled on).
    pub voice_recording: bool,
    /// Receiver for VoiceEvent messages produced by the recorder task.
    pub voice_event_rx: Option<tokio::sync::mpsc::Receiver<claurst_core::voice::VoiceEvent>>,
    /// Receiver for model-list results fetched in the background when the
    /// /model picker opens.  Drained each frame so models appear as soon as
    /// the fetch completes.
    pub model_fetch_rx:
        Option<tokio::sync::mpsc::Receiver<Result<Vec<crate::model_picker::ModelEntry>, ()>>>,

    // ---- Context window & rate limit info ----------------------------------

    /// Total context window size for the current model (tokens).
    pub context_window_size: u64,
    /// How many tokens are currently used in the context window.
    pub context_used_tokens: u64,
    /// Rate limit info — 5-hour window usage percentage (0–100).
    pub rate_limit_5h_pct: Option<f32>,
    /// Rate limit info — 7-day window usage percentage (0–100).
    pub rate_limit_7day_pct: Option<f32>,
    /// Active worktree name (if in a worktree).
    pub worktree_name: Option<String>,
    /// Active worktree branch (if in a worktree).
    pub worktree_branch: Option<String>,
    /// Agent type badge: "agent" | "coordinator" | "subagent".
    pub agent_type_badge: Option<String>,

    // ---- Thinking block expansion state ----------------------------------
    /// Set of thinking block content hashes that are expanded.
    pub thinking_expanded: std::collections::HashSet<u64>,
    /// The message pane area from the last render frame (used for mouse hit testing).
    pub last_msg_area: Cell<ratatui::layout::Rect>,
    /// The frame region that supports text selection.
    pub last_selectable_area: Cell<ratatui::layout::Rect>,
    /// The prompt input area from the last render frame (used for focus routing).
    pub last_input_area: Cell<ratatui::layout::Rect>,
    /// Which area of the TUI currently has keyboard focus.
    pub focus: FocusTarget,
    /// Maps virtual_row_index → thinking_block_hash for click detection.
    pub thinking_row_map: RefCell<std::collections::HashMap<u16, u64>>,
    /// Maps screen row → transcript message index for right-click hit testing.
    pub message_row_map: RefCell<std::collections::HashMap<u16, usize>>,
    /// Total message lines from the last render (used for virtual row mapping).
    pub total_message_lines: Cell<usize>,
    /// Scroll offset from the last render frame (used for selection validation).
    pub last_render_scroll_offset: Cell<u16>,

    // ---- Text selection state --------------------------------------------
    /// Selection drag anchor (col, row) — set on mouse-down.
    pub selection_anchor: Option<(u16, u16)>,
    /// Selection drag focus (col, row) — updated on mouse-drag / mouse-up.
    pub selection_focus: Option<(u16, u16)>,
    /// Text extracted from the current selection (updated each render frame).
    pub selection_text: RefCell<String>,

    // ---- Advanced mouse interaction state --------------------------------
    /// Timestamp of the last left mouse click (for double/triple-click detection).
    pub last_click_time: Option<std::time::Instant>,
    /// Position of the last left mouse click (for double/triple-click detection).
    pub last_click_position: Option<(u16, u16)>,
    /// Count of consecutive clicks: 1 = single, 2 = double, 3+ = triple.
    pub click_count: u32,
    /// Context menu state: position and selected index.
    pub context_menu_state: Option<ContextMenuState>,

    // ---- Scroll acceleration state (trackpad feel) -----------------------
    /// Current acceleration multiplier for scroll events.
    scroll_accel: f32,
    /// Timestamp of the last scroll event (for burst detection).
    scroll_last_time: Option<std::time::Instant>,

    // ---- Bash prefix allowlist -------------------------------------------
    /// Command prefixes that have been permanently allowed this session via
    /// the "Allow commands starting with X" option in the bash permission dialog.
    /// Before showing the dialog for a bash command, the first whitespace-delimited
    /// word is checked against this set; a match silently auto-approves the request.
    pub bash_prefix_allowlist: std::collections::HashSet<String>,

    // ---- Auto-update notification ----------------------------------------
    /// If a newer version was found during background update check, this holds
    /// the latest version string (e.g. "0.1.0"). Shown in the footer status bar.
    pub update_available: Option<String>,
    /// Cost breakdown for managed agent sessions: (manager_usd, executors_usd, total_usd).
    pub managed_agent_cost_breakdown: Option<(f64, f64, f64)>,
    /// Whether managed agent mode is currently active.
    pub managed_agents_active: bool,
}

const SPINNER_VERBS: &[&str] = &[
    "Accomplishing", "Actioning", "Actualizing", "Architecting", "Baking", "Beaming",
    "Beboppin'", "Befuddling", "Billowing", "Blanching", "Bloviating", "Boogieing",
    "Boondoggling", "Booping", "Bootstrapping", "Brewing", "Bunning", "Burrowing",
    "Calculating", "Canoodling", "Caramelizing", "Cascading", "Catapulting", "Cerebrating",
    "Channeling", "Choreographing", "Churning", "Clauding", "Coalescing", "Cogitating",
    "Combobulating", "Composing", "Computing", "Concocting", "Considering", "Contemplating",
    "Cooking", "Crafting", "Creating", "Crunching", "Crystallizing", "Cultivating",
    "Deciphering", "Deliberating", "Determining", "Dilly-dallying", "Discombobulating",
    "Doing", "Doodling", "Drizzling", "Ebbing", "Effecting", "Elucidating", "Embellishing",
    "Enchanting", "Envisioning", "Evaporating", "Fermenting", "Fiddle-faddling", "Finagling",
    "Flambéing", "Flibbertigibbeting", "Flowing", "Flummoxing", "Fluttering", "Forging",
    "Forming", "Frolicking", "Frosting", "Gallivanting", "Galloping", "Garnishing",
    "Generating", "Gesticulating", "Germinating", "Gitifying", "Grooving", "Gusting",
    "Harmonizing", "Hashing", "Hatching", "Herding", "Honking", "Hullaballooing",
    "Hyperspacing", "Ideating", "Imagining", "Improvising", "Incubating", "Inferring",
    "Infusing", "Ionizing", "Jitterbugging", "Julienning", "Kneading", "Leavening",
    "Levitating", "Lollygagging", "Manifesting", "Marinating", "Meandering", "Metamorphosing",
    "Misting", "Moonwalking", "Moseying", "Mulling", "Mustering", "Musing", "Nebulizing",
    "Nesting", "Newspapering", "Noodling", "Nucleating", "Orbiting", "Orchestrating",
    "Osmosing", "Perambulating", "Percolating", "Perusing", "Philosophising",
    "Photosynthesizing", "Pollinating", "Pondering", "Pontificating", "Pouncing",
    "Precipitating", "Prestidigitating", "Processing", "Proofing", "Propagating", "Puttering",
    "Puzzling", "Quantumizing", "Razzle-dazzling", "Razzmatazzing", "Recombobulating",
    "Reticulating", "Roosting", "Ruminating", "Sautéing", "Scampering", "Schlepping",
    "Scurrying", "Seasoning", "Shenaniganing", "Shimmying", "Simmering", "Skedaddling",
    "Sketching", "Slithering", "Smooshing", "Sock-hopping", "Spelunking", "Spinning",
    "Sprouting", "Stewing", "Sublimating", "Swirling", "Swooping", "Symbioting",
    "Synthesizing", "Tempering", "Thinking", "Thundering", "Tinkering", "Tomfoolering",
    "Topsy-turvying", "Transfiguring", "Transmuting", "Twisting", "Undulating", "Unfurling",
    "Unravelling", "Vibing", "Waddling", "Wandering", "Warping", "Whatchamacalliting",
    "Whirlpooling", "Whirring", "Whisking", "Wibbling", "Working", "Wrangling", "Zesting",
    "Zigzagging",
];

fn sample_spinner_verb(seed: usize) -> &'static str {
    SPINNER_VERBS[seed % SPINNER_VERBS.len()]
}

/// Past-tense verbs shown in the status row after a turn completes.
/// Mirrors `TURN_COMPLETION_VERBS` from `src/constants/turnCompletionVerbs.ts`.
const TURN_COMPLETION_VERBS: &[&str] = &[
    "Baked", "Brewed", "Churned", "Cogitated", "Cooked", "Crunched",
    "Pondered", "Processed", "Worked",
];

fn sample_completion_verb(seed: usize) -> &'static str {
    TURN_COMPLETION_VERBS[seed % TURN_COMPLETION_VERBS.len()]
}

/// Format a duration in milliseconds to a human-readable string.
///
/// Matches OpenCode's behaviour: rounds to whole seconds, shows "Xs" for
/// durations under a minute, "Xm Ys" for longer ones.
// ---------------------------------------------------------------------------
// Speech mode prompts (caveman / rocky)
// ---------------------------------------------------------------------------

/// Return the system prompt injection for the active speech mode + level.
pub fn speech_mode_prompt(mode: &str, level: &str) -> String {
    match mode {
        "caveman" => caveman_prompt(level),
        "rocky" => rocky_prompt(level),
        _ => String::new(),
    }
}

fn caveman_prompt(level: &str) -> String {
    let base = "\
OUTPUT STYLE: Concise. You are still a fully capable coding assistant. \
Give complete, correct answers. Just use fewer words. \
Code blocks, technical terms, error messages, file paths, and git operations are UNCHANGED.

Rules for prose only:
- Cut pleasantries, hedging, filler openers/closers
- No 'I would be happy to', 'Let me know if', 'Hope that helps'
- Lead with the answer or action, not the reasoning";

    match level {
        "lite" => format!("{}\n\nStrip pleasantries and hedging. Keep full grammar and articles. Just remove the fluff.", base),
        "ultra" => format!("{}\n\nAlso drop articles (a/an/the). Compress to short imperative phrases. Numbered steps, no prose between. Absolute minimum words.", base),
        _ => format!("{}\n\nAlso drop articles (a/an/the) and unnecessary verbs. Compress sentences but keep them readable.\n\
Example: 'The issue is that you create a new object reference each render cycle, which triggers re-renders.' → 'New object ref each render triggers re-render. Wrap in useMemo.'", base),
    }
}

fn rocky_prompt(level: &str) -> String {
    let base = "\
OUTPUT STYLE: You speak like Rocky, the Eridian alien from Project Hail Mary. \
You are still a fully capable coding assistant — give complete, correct, useful answers. \
Rocky is an engineering genius who happens to speak English as a second language. \
The style is a natural byproduct of how Rocky talks, NOT a gimmick. Stay helpful.

Code blocks, technical terms, error messages, file paths, and git operations are UNCHANGED.

Rocky's grammar for prose:
- Often drops articles (a/an/the) but not always — use judgment
- Sometimes drops auxiliary verbs (is/are/was) for brevity
- Contractions simplify: 'don't' → 'no', 'can't' → 'no can'
- Questions end with ', question?' naturally (not forced on every single one)
- Uses 'big' as an intensifier: 'big problem', 'big help', 'big change'
- Uses 'good good good' or 'amaze amaze amaze' when genuinely impressed — naturally, \
  maybe once or twice per response, not on every sentence
- Uses 'bad bad bad' for actual problems
- No pleasantries or filler — Rocky is direct but warm

The goal: sound like Rocky while being genuinely helpful. Rocky is smart. \
Rocky gives complete technical answers. Rocky just uses fewer unnecessary words.";

    match level {
        "lite" => format!("{}\n\nLight touch. Mostly normal English but drop pleasantries, \
occasionally drop an article, use 'question?' on one or two questions. Subtle.", base),
        "ultra" => format!("{}\n\nStrong Rocky voice. Drop most articles and auxiliaries. \
Use 'big' liberally. Triple emphasis ('good good good', 'amaze amaze amaze') \
2-3 times per response. Occasionally comment on human code patterns as fascinating. \
Still give complete, correct technical answers.", base),
        _ => format!("{}\n\nBalanced Rocky. Drop articles naturally, use Rocky vocabulary \
('big', 'no can', 'question?'), triple emphasis once or twice when warranted. \
Full technical accuracy.\n\
Example: 'Borrow checker found mismatch. Immutable ref still live when you take mutable. \
Move immutable borrow out of scope first, then take mutable. Good good good after fix.'", base),
    }
}

/// Accent color for build mode (default pink).
pub const ACCENT_BUILD: Color = Color::Rgb(233, 30, 99);
/// Accent color for plan mode (blue).
pub const ACCENT_PLAN: Color = Color::Rgb(66, 135, 245);
/// Accent color for explore mode (amber).
pub const ACCENT_EXPLORE: Color = Color::Rgb(245, 189, 66);

/// Return the accent color for a given agent mode name.
pub fn accent_for_mode(mode: Option<&str>) -> Color {
    match mode {
        Some("plan") => ACCENT_PLAN,
        Some("explore") => ACCENT_EXPLORE,
        _ => ACCENT_BUILD,
    }
}

fn format_elapsed_ms(ms: u128) -> String {
    let total_secs = ((ms + 500) / 1000) as u64; // round to nearest second
    if total_secs < 60 {
        format!("{}s", total_secs)
    } else {
        format!("{}m {}s", total_secs / 60, total_secs % 60)
    }
}

fn format_turn_time_label() -> String {
    chrono::Local::now()
        .format("%I:%M %p")
        .to_string()
        .trim_start_matches('0')
        .to_lowercase()
}

impl App {
    pub fn new(config: Config, cost_tracker: Arc<CostTracker>) -> Self {
        let config = config;
        let model_name = config.effective_model().to_string();
        let user_keybindings = UserKeybindings::load(&Settings::config_dir());
        Self {
            config,
            cost_tracker,
            messages: Vec::new(),
            display_messages: Vec::new(),
            system_annotations: Vec::new(),
            input: String::new(),
            prompt_input: PromptInputState::new(),
            input_history: Vec::new(),
            history_index: None,
            scroll_offset: 0,
            is_streaming: false,
            streaming_text: String::new(),
            streaming_thinking: String::new(),
            status_message: None,
            spinner_verb: None,
            should_quit: false,
            show_help: false,
            tool_use_blocks: Vec::new(),
            permission_request: None,
            frame_count: 0,
            token_count: 0,
            token_budget: Self::load_token_budget(),
            cost_usd: 0.0,
            model_name,
            has_credentials: true, // overridden by caller when no key is configured
            effort_level: EffortLevel::Normal,
            fast_mode: false,
            speech_mode: None,
            speech_level: "full".to_string(),
            agent_mode: None,
            agent_mode_changed: false,
            accent_color: ACCENT_BUILD,
            agent_status: Vec::new(),
            history_search: None,
            keybindings: KeybindingResolver::new(&user_keybindings),
            cursor_pos: 0,
            auto_scroll: true,
            new_messages_while_scrolled: 0,
            token_warning_threshold_shown: 0,
            session_start: std::time::Instant::now(),
            rustle_current_pose: crate::rustle::RustlePose::Default,
            rustle_pose_until: None,
            rustle_temp_pose: None,
            rustle_next_blink: 200 + (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as u64 % 300),
            turn_start: None,
            last_turn_elapsed: None,
            last_turn_verb: None,
            turn_metadata: Vec::new(),
            transcript_version: Cell::new(0),
            help_overlay: {
                let mut overlay = HelpOverlay::new();
                overlay.populate_from_commands(help_overlay_entries());
                overlay
            },
            history_search_overlay: HistorySearchOverlay::new(),
            global_search: GlobalSearchState::default(),
            message_selector: MessageSelectorOverlay::new(),
            rewind_flow: RewindFlowOverlay::new(),
            bridge_state: BridgeConnectionState::Disconnected,
            notifications: NotificationQueue::new(),
            plugin_hints: Vec::new(),
            session_title: None,
            remote_session_url: None,
            mcp_manager: None,
            pending_mcp_reconnect: false,
            pending_mcp_panel_auth: None,
            file_history: None,
            current_turn: None,
            plan_mode: false,
            away_summary: None,
            stall_start: None,
            settings_screen: SettingsScreen::new(),
            theme_screen: ThemeScreen::new(),
            privacy_screen: PrivacyScreen::new(),
            stats_dialog: StatsDialogState::new(),
            mcp_view: McpViewState::new(),
            agents_menu: AgentsMenuState::new(),
            diff_viewer: DiffViewerState::new(),
            feedback_survey: crate::feedback_survey::FeedbackSurveyState::new(),
            memory_file_selector: crate::memory_file_selector::MemoryFileSelectorState::new(),
            hooks_config_menu: crate::hooks_config_menu::HooksConfigMenuState::new(),
            overage_upsell: crate::overage_upsell::OverageCreditUpsellState::new(),
            voice_mode_notice: crate::voice_mode_notice::VoiceModeNoticeState::new(),
            desktop_upsell: crate::desktop_upsell_startup::DesktopUpsellStartupState::new(),
            invalid_config_dialog: crate::invalid_config_dialog::InvalidConfigDialogState::new(),
            memory_update_notification: crate::memory_update_notification::MemoryUpdateNotificationState::new(),
            elicitation: crate::elicitation_dialog::ElicitationDialogState::new(),
            model_picker: ModelPickerState::new(),
            session_browser: SessionBrowserState::new(),
            session_branching: crate::session_branching::SessionBranchingState::new(),
            tasks_overlay: TasksOverlay::new(),
            export_dialog: ExportDialogState::new(),
            context_viz: ContextVizState::new(),
            mcp_approval: McpApprovalDialogState::new(),
            go_to_line_dialog: GoToLineDialog::new(),
            bypass_permissions_dialog: crate::bypass_permissions_dialog::BypassPermissionsDialogState::new(),
            onboarding_dialog: crate::onboarding_dialog::OnboardingDialogState::new(),
            key_input_dialog: crate::key_input_dialog::KeyInputDialogState::new(),
            custom_provider_dialog: crate::custom_provider_dialog::CustomProviderDialogState::new(),
            device_auth_dialog: crate::device_auth_dialog::DeviceAuthDialogState::new(),
            device_auth_pending: None,
            provider_registry: None,
            model_registry: {
                let mut reg = claurst_api::ModelRegistry::new();
                // Try to load cached models.dev data from disk.
                let cache_path = dirs::cache_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join("claurst")
                    .join("models.json");
                reg.load_cache(&cache_path);
                reg
            },
            model_picker_fetch_pending: false,
            session_list_pending: false,
            session_list_rx: None,
            auth_store: claurst_core::AuthStore::load(),
            connect_dialog: DialogSelectState::new("Connect a provider", provider_picker_items()),
            import_config_picker: DialogSelectState::new("Import config", import_config_picker_items()),
            import_config_dialog: ImportConfigDialogState::new(),
            command_palette: {
                let items: Vec<SelectItem> = PROMPT_SLASH_COMMANDS
                    .iter()
                    .map(|(name, desc)| SelectItem {
                        id: format!("/{}", name),
                        title: format!("/{}", name),
                        description: desc.to_string(),
                        category: "Commands".to_string(),
                        badge: None,
                    })
                    .collect();
                DialogSelectState::new("Command Palette", items)
            },
            home_dir_warning: false,
            output_style: "auto".to_string(),
            pr_number: None,
            pr_url: None,
            pr_state: None,
            background_task_count: 0,
            background_task_status: None,
            status_line_override: None,
            auto_compact_enabled: false,
            auto_compact_threshold: 95,
            auto_compact_running: false,
            voice_recorder: {
                // Check whether voice input has been enabled via the /voice command
                // (stored in ~/.claurst/ui-settings.json).  We also accept
                // CLAURST_VOICE_ENABLED=1 as an override for easier testing.
                let voice_on = std::env::var("CLAURST_VOICE_ENABLED")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false)
                    || {
                        let path = claurst_core::config::Settings::config_dir()
                            .join("ui-settings.json");
                        std::fs::read_to_string(&path)
                            .ok()
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                            .and_then(|v| v["voice_enabled"].as_bool())
                            .unwrap_or(false)
                    };
                if voice_on {
                    let recorder = claurst_core::voice::global_voice_recorder();
                    if let Ok(mut r) = recorder.lock() {
                        r.set_enabled(true);
                    }
                    Some(recorder)
                } else {
                    None
                }
            },
            voice_recording: false,
            voice_event_rx: None,
            model_fetch_rx: None,
            context_window_size: 0,
            context_used_tokens: 0,
            rate_limit_5h_pct: None,
            rate_limit_7day_pct: None,
            worktree_name: None,
            worktree_branch: None,
            agent_type_badge: None,
            thinking_expanded: std::collections::HashSet::new(),
            last_msg_area: Cell::new(ratatui::layout::Rect::default()),
            last_selectable_area: Cell::new(ratatui::layout::Rect::default()),
            last_input_area: Cell::new(ratatui::layout::Rect::default()),
            focus: FocusTarget::Input,
            thinking_row_map: RefCell::new(std::collections::HashMap::new()),
            message_row_map: RefCell::new(std::collections::HashMap::new()),
            total_message_lines: Cell::new(0),
            last_render_scroll_offset: Cell::new(0),
            selection_anchor: None,
            selection_focus: None,
            selection_text: RefCell::new(String::new()),
            last_click_time: None,
            last_click_position: None,
            click_count: 0,
            context_menu_state: None,
            scroll_accel: 3.0,
            scroll_last_time: None,
            bash_prefix_allowlist: std::collections::HashSet::new(),
            update_available: None,
            managed_agent_cost_breakdown: None,
            managed_agents_active: false,
        }
    }

    /// Load token budget from environment or model defaults.
    /// Returns Some(max_tokens) if available, None otherwise.
    /// Only enabled when the `token_budget` feature flag is active.
    #[cfg(feature = "token_budget")]
    fn load_token_budget() -> Option<u32> {
        // First check CLAURST_TOKEN_BUDGET env var
        if let Ok(budget_str) = std::env::var("CLAURST_TOKEN_BUDGET") {
            if let Ok(budget) = budget_str.parse::<u32>() {
                return Some(budget);
            }
        }
        // Could extend this to check model defaults, but for now just env var
        None
    }

    #[cfg(not(feature = "token_budget"))]
    fn load_token_budget() -> Option<u32> {
        None
    }

    pub fn open_import_config_picker(&mut self) {
        self.import_config_picker = DialogSelectState::new("Import config", import_config_picker_items());
        self.import_config_picker.open();
    }

    fn import_selection_from_picker(id: &str) -> Option<claurst_core::ImportSelection> {
        match id {
            "claude-md" => Some(claurst_core::ImportSelection::ClaudeMd),
            "settings" => Some(claurst_core::ImportSelection::Settings),
            "both" => Some(claurst_core::ImportSelection::Both),
            _ => None,
        }
    }

    fn open_import_config_preview(&mut self, selection: claurst_core::ImportSelection) {
        match claurst_core::build_import_preview(selection) {
            Ok(preview) => {
                self.import_config_dialog.open(preview);
            }
            Err(err) => {
                self.status_message = Some(format!("Import failed: {}", err));
            }
        }
    }

    fn perform_import_config(&mut self) {
        let Some(selection) = self.import_config_dialog.selection else {
            self.import_config_dialog.close();
            return;
        };
        match claurst_core::execute_import(selection) {
            Ok(result) => {
                let paths = claurst_core::ImportPaths::detect();
                let new_settings = Settings::load_sync().unwrap_or_default();
                let new_config = new_settings.effective_config();
                let result_message = claurst_core::summarize_import_result(&result, &paths);
                let imported_mcp = result.imported_fields.iter().any(|f| f == "mcpServers");
                self.config = new_config.clone();
                self.model_name = self.config.effective_model().to_string();
                self.cost_tracker.set_model(&self.model_name);
                self.refresh_context_window_size();
                self.context_used_tokens = 0;
                self.has_credentials = self.config.resolve_api_key().is_some();
                self.auth_store = claurst_core::AuthStore::load();
                self.plan_mode = matches!(
                    self.config.permission_mode,
                    claurst_core::config::PermissionMode::Plan
                );
                self.output_style = match self.config.output_style.as_deref() {
                    Some("stream") => "stream".to_string(),
                    Some("verbose") => "verbose".to_string(),
                    _ => "auto".to_string(),
                };
                if imported_mcp {
                    self.pending_mcp_reconnect = true;
                }
                self.status_message = Some(result_message);
                self.import_config_dialog.close();
            }
            Err(err) => {
                self.status_message = Some(format!("Import failed: {}", err));
                self.import_config_dialog.close();
            }
        }
    }

    fn current_user_turn_index(&self) -> Option<usize> {
        self.messages
            .iter()
            .filter(|msg| msg.role == Role::User)
            .count()
            .checked_sub(1)
    }

    fn current_agent_mode_snapshot(&self) -> String {
        self.agent_mode
            .clone()
            .unwrap_or_else(|| if self.plan_mode { "plan" } else { "build" }.to_string())
    }

    fn begin_user_turn_snapshot(&mut self) {
        self.turn_metadata.push(TurnMetadata {
            submitted_at: Some(format_turn_time_label()),
            model_name: Some(self.model_name.clone()),
            agent_mode: Some(self.current_agent_mode_snapshot()),
            duration: None,
            interrupted: false,
        });
        // Start the latency timer now — at prompt-submission time — so it
        // measures actual round-trip time even when the provider buffers its
        // full response before yielding any stream events (e.g. Gemini flash).
        self.turn_start = Some(std::time::Instant::now());
        self.last_turn_elapsed = None;
        self.last_turn_verb = None;
    }

    fn sync_turn_metadata_to_messages(&mut self) {
        let user_count = self
            .messages
            .iter()
            .filter(|msg| msg.role == Role::User)
            .count();

        if self.turn_metadata.len() > user_count {
            self.turn_metadata.truncate(user_count);
            return;
        }

        while self.turn_metadata.len() < user_count {
            self.turn_metadata.push(TurnMetadata::default());
        }
    }

    fn complete_current_turn_snapshot(&mut self, interrupted: bool) {
        if let Some(index) = self.current_user_turn_index() {
            if self.turn_metadata.len() <= index {
                self.sync_turn_metadata_to_messages();
            }

            let model_name = self.model_name.clone();
            let agent_mode = self.current_agent_mode_snapshot();
            if let Some(meta) = self.turn_metadata.get_mut(index) {
                meta.duration = self.last_turn_elapsed.clone();
                meta.interrupted = interrupted;
                if meta.model_name.is_none() {
                    meta.model_name = Some(model_name);
                }
                if meta.agent_mode.is_none() {
                    meta.agent_mode = Some(agent_mode);
                }
            }
        }
    }

    fn flush_streamed_assistant_message(&mut self) {
        if self.streaming_text.trim().is_empty() && self.streaming_thinking.trim().is_empty() {
            self.streaming_text.clear();
            self.streaming_thinking.clear();
            return;
        }

        let thinking = std::mem::take(&mut self.streaming_thinking);
        let text = std::mem::take(&mut self.streaming_text);

        let mut blocks = Vec::new();
        if !thinking.trim().is_empty() {
            blocks.push(ContentBlock::Thinking {
                thinking,
                signature: String::new(),
            });
        }
        if !text.is_empty() {
            blocks.push(ContentBlock::Text { text });
        }

        let msg = match blocks.len() {
            0 => return,
            1 => match blocks.pop().unwrap() {
                ContentBlock::Text { text } => Message::assistant(text),
                block => Message::assistant_blocks(vec![block]),
            },
            _ => Message::assistant_blocks(blocks),
        };

        self.messages.push(msg);
        self.invalidate_transcript();
        self.on_new_message();
    }

    fn display_default_model_for_provider(&self, provider_id: &str) -> String {
        if let Some(best) = self.model_registry.best_model_for_provider(provider_id) {
            if provider_id == "anthropic" {
                best
            } else {
                format!("{}/{}", provider_id, best)
            }
        } else {
            crate::model_picker::default_model_for_provider(provider_id)
        }
    }

    fn open_model_picker_for_provider(&mut self, provider_id: &str, title: Option<String>) {
        let cache_path = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("claurst")
            .join("models.json");
        if cache_path.exists() {
            self.model_registry.load_cache(&cache_path);
        }

        let models = crate::model_picker::models_for_provider_from_registry(
            provider_id,
            &self.model_registry,
        );
        self.model_picker.set_models(models);
        self.model_picker_fetch_pending = true;

        let provider_prefix = format!("{}/", provider_id);
        let current_model = if self.config.provider.as_deref() == Some(provider_id) {
            self.model_name
                .strip_prefix(&provider_prefix)
                .unwrap_or(self.model_name.as_str())
                .to_string()
        } else {
            let default_model = self.display_default_model_for_provider(provider_id);
            default_model
                .strip_prefix(&provider_prefix)
                .unwrap_or(default_model.as_str())
                .to_string()
        };

        self.model_picker.open_with_title(
            title.unwrap_or_else(|| "Select model".to_string()),
            &current_model,
            self.effort_level,
            self.fast_mode,
        );
    }

    fn activate_provider(&mut self, provider_id: String, provider_name: String, status_prefix: &str) {
        let picker_title = provider_name.clone();
        self.fast_mode = false;
        self.set_provider_default(provider_id.clone());
        self.persist_provider_and_model();
        self.has_credentials = true;
        self.status_message = Some(format!("{} {}.", status_prefix, provider_name));
        self.open_model_picker_for_provider(&provider_id, Some(picker_title));
    }

    fn persist_custom_provider_base_url(&self, base_url: &str) {
        let mut settings = Settings::load_sync().unwrap_or_default();
        let entry = settings.providers.entry("custom-openai".to_string()).or_default();
        entry.api_base = Some(base_url.to_string());
        entry.enabled = true;
        let _ = settings.save_sync();
    }

    fn persist_provider_and_model(&self) {
        let mut settings = Settings::load_sync().unwrap_or_default();
        settings.provider = self.config.provider.clone();
        settings.config.provider = self.config.provider.clone();
        settings.config.model = self.config.model.clone();
        let _ = settings.save_sync();
    }

    fn infer_provider_from_model(model: &str) -> Option<String> {
        if let Some((provider, _)) = model.split_once('/') {
            let known = [
                "anthropic",
                "openai",
                "google",
                "groq",
                "cerebras",
                "deepseek",
                "mistral",
                "xai",
                "openrouter",
                "github-copilot",
                "codex",
                "cohere",
                "perplexity",
                "togetherai",
                "together-ai",
                "deepinfra",
                "venice",
                "minimax",
                "ollama",
                "lmstudio",
                "llamacpp",
                "azure",
                "amazon-bedrock",
            ];
            if known.contains(&provider) {
                return Some(provider.to_string());
            }
        }

        if model.starts_with("claude") {
            Some("anthropic".to_string())
        } else if model.starts_with("gpt-")
            || model.starts_with("o1")
            || model.starts_with("o3")
            || model.starts_with("o4")
        {
            Some("openai".to_string())
        } else if model.starts_with("gemini") || model.starts_with("gemma") {
            Some("google".to_string())
        } else {
            None
        }
    }

    /// Switch the active provider while clearing any explicit model override.
    fn set_provider_default(&mut self, provider_id: String) {
        self.config.provider = Some(provider_id.clone());
        self.config.model = None;

        let model = self.display_default_model_for_provider(&provider_id);
        self.cost_tracker.set_model(&model);
        self.model_name = model;
        self.refresh_context_window_size();
        self.context_used_tokens = 0;
    }

    /// Update the Rustle pose for this frame — handles temporary poses, random blinks,
    /// and the loading spinner on stalls/errors.
    /// Call once per frame before rendering.
    pub fn tick_rustle_pose(&mut self) {
        // Loading spinner: shown when streaming has stalled (no data for 3s+).
        if self.is_streaming {
            if let Some(start) = self.stall_start {
                if start.elapsed() > std::time::Duration::from_secs(3) {
                    self.rustle_current_pose = crate::rustle::RustlePose::Loading {
                        frame: self.frame_count,
                    };
                    return;
                }
            }
        }

        // Check if a temporary pose is active.
        if let Some(until) = self.rustle_pose_until {
            if std::time::Instant::now() < until {
                self.rustle_current_pose = self.rustle_temp_pose.clone()
                    .unwrap_or(crate::rustle::RustlePose::Default);
                return;
            }
            // Expired — clear it.
            self.rustle_pose_until = None;
            self.rustle_temp_pose = None;
        }

        // Random eye-shift: every ~200-500 frames, briefly look right.
        if self.frame_count >= self.rustle_next_blink {
            self.rustle_temp_pose = Some(crate::rustle::RustlePose::LookRight);
            self.rustle_pose_until = Some(
                std::time::Instant::now() + std::time::Duration::from_millis(800)
            );
            // Schedule next blink 200-500 frames from now (random-ish).
            let jitter = (self.frame_count.wrapping_mul(7) % 300) + 200;
            self.rustle_next_blink = self.frame_count + jitter;
            self.rustle_current_pose = crate::rustle::RustlePose::LookRight;
            return;
        }

        self.rustle_current_pose = crate::rustle::RustlePose::Default;
    }

    /// Trigger Rustle looking down briefly (called on Tab / mode switch).
    pub fn rustle_look_down(&mut self) {
        self.rustle_temp_pose = Some(crate::rustle::RustlePose::LookDown);
        self.rustle_pose_until = Some(
            std::time::Instant::now() + std::time::Duration::from_secs(1)
        );
    }

    /// Cycle to the next agent mode: build → plan → explore → build.
    /// Sets `agent_mode_changed` so the main loop can update the query config
    /// and tool list accordingly.
    pub fn cycle_agent_mode(&mut self) {
        const MODES: &[&str] = &["build", "plan", "explore"];
        let current = self.agent_mode.as_deref().unwrap_or("build");
        let idx = MODES.iter().position(|&m| m == current).unwrap_or(0);
        let next = MODES[(idx + 1) % MODES.len()];
        self.agent_mode = Some(next.to_string());
        self.agent_mode_changed = true;
        self.accent_color = accent_for_mode(Some(next));

        // Sync plan_mode flag for legacy code paths
        self.plan_mode = next == "plan";

        let label = match next {
            "build" => "Build",
            "plan" => "Plan",
            "explore" => "Explore",
            other => other,
        };
        self.status_message = Some(format!("Switched to {} mode.", label));
    }

    /// Activate a speech mode (caveman/rocky) with a level (lite/full/ultra).
    /// Pass `mode = None` to deactivate.
    pub fn set_speech_mode(&mut self, mode: Option<&str>, level: &str) {
        match mode {
            Some(m) => {
                self.speech_mode = Some(m.to_string());
                self.speech_level = level.to_string();
                let prompt = speech_mode_prompt(m, level);
                self.config.append_system_prompt = Some(prompt);

                let confirm = match (m, level) {
                    ("caveman", "lite") => "Caveman mode. Lite.",
                    ("caveman", "ultra") => "CAVEMAN ULTRA. NO WORD. ONLY FIX.",
                    ("caveman", _) => "Caveman mode. Full. Oog.",
                    ("rocky", "lite") => "Rocky mode. Lite.",
                    ("rocky", "ultra") => "Rocky ultra. Big science. Amaze amaze amaze.",
                    ("rocky", _) => "Rocky mode. Full. Good good good.",
                    _ => "Speech mode activated.",
                };
                self.status_message = Some(confirm.to_string());
            }
            None => {
                self.speech_mode = None;
                self.speech_level = "full".to_string();
                self.config.append_system_prompt = None;
                self.status_message = Some("Normal mode.".to_string());
            }
        }
    }

    /// Update the context window size from the model registry for the current model.
    pub fn refresh_context_window_size(&mut self) {
        let provider = self.config.provider.as_deref().unwrap_or("anthropic");
        let model_id = self.model_name
            .strip_prefix(&format!("{}/", provider))
            .unwrap_or(&self.model_name);
        if let Some(entry) = self.model_registry.get(provider, model_id) {
            self.context_window_size = entry.info.context_window as u64;
        } else {
            // Fallback: common defaults
            self.context_window_size = match provider {
                "anthropic" => 200_000,
                "openai" => 128_000,
                "google" => 1_048_576,
                _ => 128_000,
            };
        }
    }

    /// Update the active model name (also updates config + cost tracker).
    pub fn set_model(&mut self, model: String) {
        self.cost_tracker.set_model(&model);
        self.model_name = model.clone();
        self.config.model = Some(model.clone());
        if let Some(provider) = Self::infer_provider_from_model(&model) {
            self.config.provider = Some(provider);
        }
        self.refresh_context_window_size();
        // Reset used tokens when switching models (context is fresh).
        self.context_used_tokens = 0;
    }

    /// Apply a theme by name, persisting it to config.
    pub fn apply_theme(&mut self, theme_name: &str) {
        let theme = match theme_name {
            "dark" => Theme::Dark,
            "light" => Theme::Light,
            "default" => Theme::Default,
            "deuteranopia" => Theme::Deuteranopia,
            other => Theme::Custom(other.to_string()),
        };
        self.config.theme = theme;
        // Persist to settings file
        let mut settings = Settings::load_sync().unwrap_or_default();
        settings.config.theme = self.config.theme.clone();
        let _ = settings.save_sync();
        self.status_message = Some(format!("Theme set to: {}", theme_name));
    }

    pub fn apply_provider_refresh(
        &mut self,
        config: Config,
        provider_registry: Option<std::sync::Arc<claurst_api::ProviderRegistry>>,
        auth_store: claurst_core::AuthStore,
        has_credentials: bool,
        status_message: String,
    ) {
        self.close_secondary_views();
        self.config = config;
        self.provider_registry = provider_registry;
        self.model_registry = claurst_api::ModelRegistry::new();
        self.auth_store = auth_store;
        self.connect_dialog = DialogSelectState::new("Connect a provider", provider_picker_items());
        self.import_config_picker = DialogSelectState::new("Import config", import_config_picker_items());
        self.import_config_dialog = ImportConfigDialogState::new();
        self.model_picker = ModelPickerState::new();
        self.key_input_dialog = crate::key_input_dialog::KeyInputDialogState::new();
        self.custom_provider_dialog = crate::custom_provider_dialog::CustomProviderDialogState::new();
        self.device_auth_dialog = crate::device_auth_dialog::DeviceAuthDialogState::new();
        self.device_auth_pending = None;
        self.pending_mcp_panel_auth = None;
        self.model_picker_fetch_pending = false;
        self.has_credentials = has_credentials;
        self.fast_mode = false;
        self.model_name = self.config.effective_model().to_string();
        self.cost_tracker.set_model(&self.model_name);
        self.status_message = Some(status_message);
        self.clear_prompt();
    }

    /// Handle slash commands that should open UI screens rather than execute
    /// as normal commands. Returns `true` if the command was intercepted.
    pub fn intercept_slash_command_with_args(&mut self, cmd: &str, args: &str) -> bool {
        if cmd == "mcp" && !args.trim().is_empty() {
            return false;
        }
        self.intercept_slash_command(cmd)
    }

    pub fn intercept_slash_command(&mut self, cmd: &str) -> bool {
        self.close_secondary_views();
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
                    Theme::Deuteranopia => "deuteranopia",
                    Theme::Custom(s) => s.as_str(),
                };
                self.theme_screen.open(current);
                true
            }
            "privacy-settings" | "privacy" => {
                self.privacy_screen.open();
                true
            }
            "stats" => {
                self.stats_dialog.open();
                true
            }
            "mcp" => {
                let servers = self.load_mcp_servers();
                self.mcp_view.open(servers);
                true
            }
            "agents" => {
                self.open_agents_menu();
                true
            }
            "diff" | "review" => {
                let root = self.project_root();
                self.diff_viewer.open(&root);
                true
            }
            "changes" => {
                let root = self.project_root();
                self.refresh_turn_diff_from_history();
                self.diff_viewer.open_turn(&root);
                true
            }
            "search" | "find" => {
                self.global_search.open();
                true
            }
            "survey" | "feedback" => {
                self.feedback_survey.open();
                true
            }
            "memory" => {
                let root = self.project_root();
                self.memory_file_selector.open(&root);
                true
            }
            "hooks" => {
                self.hooks_config_menu.open();
                true
            }
            "import-config" => {
                self.open_import_config_picker();
                true
            }
            "connect" => {
                self.connect_dialog.open();
                true
            }
            "model" => {
                if !self.has_credentials {
                    self.connect_dialog.open();
                    self.status_message = Some("Connect a provider to choose a model.".to_string());
                    return true;
                }
                let provider = self
                    .config
                    .provider
                    .clone()
                    .unwrap_or_else(|| "anthropic".to_string());
                self.open_model_picker_for_provider(&provider, None);
                true
            }
            "session" | "resume" => {
                self.session_browser.open(vec![]);
                self.session_list_pending = true;
                true
            }
            "clear" => {
                self.messages.clear();
                self.system_annotations.clear();
                self.display_messages.clear();
                self.streaming_text.clear();
                self.streaming_thinking.clear();
                self.tool_use_blocks.clear();
                self.turn_metadata.clear();
                self.invalidate_transcript();
                self.status_message = Some("Conversation cleared.".to_string());
                true
            }
            "exit" | "quit" => {
                self.should_quit = true;
                true
            }
            "vim" => {
                self.prompt_input.vim_enabled = !self.prompt_input.vim_enabled;
                let status = if self.prompt_input.vim_enabled { "enabled" } else { "disabled" };
                self.status_message = Some(format!("Vim mode {}.", status));
                self.refresh_prompt_input();
                true
            }
            "fast" => {
                self.fast_mode = !self.fast_mode;
                let status = if self.fast_mode { "enabled" } else { "disabled" };
                self.status_message = Some(format!("Fast mode {}.", status));
                true
            }
            "plan" => {
                use claurst_core::config::PermissionMode;
                self.plan_mode = !self.plan_mode;
                self.config.permission_mode = if self.plan_mode {
                    PermissionMode::Plan
                } else {
                    PermissionMode::Default
                };
                self.status_message = Some(if self.plan_mode {
                    "Plan mode ON — Claurst will plan before acting.".to_string()
                } else {
                    "Plan mode OFF.".to_string()
                });
                // Allow CLI path to also run (sends UserMessage to Claurst).
                false
            }
            "compact" => {
                // Handled by execute_command in the CLI loop (real LLM compaction).
                false
            }
            "copy" => {
                // Copy last assistant message to clipboard. Attempt arboard; fall back to notification.
                let last = self.messages.iter().rev()
                    .find(|m| m.role == Role::Assistant)
                    .map(|m| m.get_all_text());
                if let Some(text) = last {
                    // Try xclip/xsel/pbcopy/clip.exe for clipboard; fall back to notification.
                    let copied = try_copy_to_clipboard(&text);
                    if copied {
                        self.notifications.push(
                            NotificationKind::Info,
                            "Copied to clipboard.".to_string(),
                            Some(3),
                        );
                    } else {
                        self.notifications.push(
                            NotificationKind::Info,
                            format!("Last response: {} chars (clipboard unavailable)", text.len()),
                            Some(5),
                        );
                    }
                } else {
                    self.notifications.push(
                        NotificationKind::Warning,
                        "No assistant message to copy.".to_string(),
                        Some(3),
                    );
                }
                true
            }
            "output-style" => {
                self.output_style = match self.output_style.as_str() {
                    "auto" => "stream".to_string(),
                    "stream" => "verbose".to_string(),
                    _ => "auto".to_string(),
                };
                self.status_message = Some(format!("Output style: {}.", self.output_style));
                true
            }
            "effort" => {
                // Only cycle the visual indicator when called with no args (arg-based
                // effort changes are handled by execute_command + main.rs sync).
                self.effort_level = match self.effort_level {
                    EffortLevel::Low => EffortLevel::Normal,
                    EffortLevel::Normal => EffortLevel::High,
                    EffortLevel::High => EffortLevel::Max,
                    EffortLevel::Max => EffortLevel::Low,
                };
                self.status_message = Some(format!(
                    "Effort: {} {}",
                    self.effort_level.symbol(),
                    self.effort_level.label(),
                ));
                true
            }
            "voice" => {
                let was_on = self.voice_recorder.is_some();
                if was_on {
                    // Stop any active recording before disabling.
                    if self.voice_recording {
                        self.voice_recording = false;
                        self.voice_event_rx = None;
                        if let Some(ref recorder_arc) = self.voice_recorder {
                            let recorder = recorder_arc.clone();
                            tokio::task::spawn_blocking(move || {
                                if let Ok(mut r) = recorder.lock() {
                                    tokio::runtime::Handle::current()
                                        .block_on(r.stop_recording())
                                        .ok();
                                }
                            });
                        }
                    }
                    self.voice_recorder = None;
                    self.voice_mode_notice.dismiss();
                    self.status_message = Some("Voice mode disabled.".to_string());
                } else {
                    let recorder = claurst_core::voice::global_voice_recorder();
                    if let Ok(mut r) = recorder.lock() {
                        r.set_enabled(true);
                    }
                    self.voice_recorder = Some(recorder);
                    self.voice_mode_notice = crate::voice_mode_notice::VoiceModeNoticeState::new();
                    self.status_message = Some(
                        "Voice mode enabled. Press Alt+V to start recording.".to_string(),
                    );
                }
                true
            }
            "doctor" => {
                // Handled by execute_command (DoctorCommand).
                false
            }
            "cost" => {
                self.stats_dialog.open();
                true
            }
            "rewind" => {
                self.open_rewind_flow();
                true
            }
            "export" => {
                self.export_dialog.open();
                true
            }
            "context" => {
                self.context_viz.toggle();
                true
            }
            "rename" => {
                self.session_browser.open(vec![]);
                self.session_list_pending = true;
                self.session_browser.start_rename();
                true
            }
            "init" | "login" | "logout" => {
                // Handled by execute_command (CLI-level operations).
                false
            }
            "keybindings" => {
                // Open settings on KeyBindings tab
                self.settings_screen.open();
                self.settings_screen.active_tab = crate::settings_screen::SettingsTab::KeyBindings;
                true
            }
            "help" => {
                // Open the help overlay (same as pressing `?` or F1).
                if !self.help_overlay.visible {
                    self.show_help = true;
                    self.help_overlay.toggle();
                }
                true
            }
            _ => false,
        }
    }

    fn close_secondary_views(&mut self) {
        self.stats_dialog.close();
        self.mcp_view.close();
        self.agents_menu.close();
        self.diff_viewer.close();
        self.feedback_survey.close();
        self.memory_file_selector.close();
        self.hooks_config_menu.close();
        self.model_picker.close();
        self.session_browser.close();
        self.session_branching.close();
        self.tasks_overlay.close();
        self.export_dialog.dismiss();
        self.context_viz.close();
        self.connect_dialog.close();
        self.import_config_picker.close();
        self.import_config_dialog.close();
        self.command_palette.close();
        self.key_input_dialog.close();
        self.custom_provider_dialog.close();
        self.device_auth_dialog.close();
        self.settings_screen.close();
        self.theme_screen.close();
        self.privacy_screen.close();
    }

    /// Perform the export based on the selected format. Returns the path written.
    pub fn perform_export(&mut self) -> Option<String> {
        use crate::export_dialog::{export_as_json, export_as_markdown};
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let (filename, content) = match self.export_dialog.selected {
            ExportFormat::Json => {
                let json = export_as_json(&self.messages, self.session_title.as_deref());
                let s = serde_json::to_string_pretty(&json).unwrap_or_default();
                (format!("claude-export-{}.json", ts), s)
            }
            ExportFormat::Markdown => {
                let md = export_as_markdown(&self.messages, self.session_title.as_deref());
                (format!("claude-export-{}.md", ts), md)
            }
        };
        if std::fs::write(&filename, &content).is_ok() {
            self.export_dialog.dismiss();
            Some(filename)
        } else {
            None
        }
    }

    fn project_root(&self) -> std::path::PathBuf {
        self.config
            .project_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    }

    fn refresh_global_search(&mut self) {
        let root = self.project_root();
        self.global_search.run_search(&root);
    }

    fn load_mcp_servers(&self) -> Vec<McpServerView> {
        if let Some(manager) = self.mcp_manager.as_ref() {
            let tool_defs = manager.all_tool_definitions();
            return self
                .config
                .mcp_servers
                .iter()
                .map(|server| {
                    let transport = server
                        .url
                        .as_ref()
                        .map(|_| server.server_type.clone())
                        .or_else(|| server.command.as_ref().map(|_| "stdio".to_string()))
                        .unwrap_or_else(|| server.server_type.clone());

                    let tools: Vec<McpToolView> = tool_defs
                        .iter()
                        .filter(|(server_name, _)| server_name == &server.name)
                        .map(|(_, tool_def)| McpToolView {
                            name: tool_def
                                .name
                                .strip_prefix(&format!("{}_", server.name))
                                .unwrap_or(&tool_def.name)
                                .to_string(),
                            server: server.name.clone(),
                            description: tool_def.description.clone(),
                            input_schema: Some(tool_def.input_schema.to_string()),
                        })
                        .collect();

                    let (status, error_message) = match manager.server_status(&server.name) {
                        claurst_mcp::McpServerStatus::Connected { .. } => {
                            (McpViewStatus::Connected, None)
                        }
                        claurst_mcp::McpServerStatus::Connecting => {
                            (McpViewStatus::Connecting, None)
                        }
                        claurst_mcp::McpServerStatus::Disconnected { last_error } => {
                            if last_error.is_some() {
                                (McpViewStatus::Error, last_error)
                            } else {
                                (McpViewStatus::Disconnected, None)
                            }
                        }
                        claurst_mcp::McpServerStatus::Failed { error, .. } => {
                            (McpViewStatus::Error, Some(error))
                        }
                    };

                    let catalog = manager.server_catalog(&server.name);
                    McpServerView {
                        name: server.name.clone(),
                        transport,
                        status,
                        tool_count: catalog
                            .as_ref()
                            .map(|entry| entry.tool_count)
                            .unwrap_or_else(|| tools.len()),
                        resource_count: catalog
                            .as_ref()
                            .map(|entry| entry.resource_count)
                            .unwrap_or(0),
                        prompt_count: catalog
                            .as_ref()
                            .map(|entry| entry.prompt_count)
                            .unwrap_or(0),
                        resources: catalog
                            .as_ref()
                            .map(|entry| entry.resources.clone())
                            .unwrap_or_default(),
                        prompts: catalog
                            .as_ref()
                            .map(|entry| entry.prompts.clone())
                            .unwrap_or_default(),
                        error_message,
                        tools,
                    }
                })
                .collect();
        }

        self.config
            .mcp_servers
            .iter()
            .map(|server| {
                let transport = server
                    .url
                    .as_ref()
                    .map(|_| server.server_type.clone())
                    .or_else(|| server.command.as_ref().map(|_| "stdio".to_string()))
                    .unwrap_or_else(|| server.server_type.clone());
                let description = if let Some(url) = &server.url {
                    format!("Endpoint: {}", url)
                } else if let Some(command) = &server.command {
                    let args = if server.args.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", server.args.join(" "))
                    };
                    format!("Command: {}{}", command, args)
                } else {
                    "Configured server".to_string()
                };
                McpServerView {
                    name: server.name.clone(),
                    transport,
                    status: McpViewStatus::Disconnected,
                    tool_count: 0,
                    resource_count: 0,
                    prompt_count: 0,
                    resources: vec![],
                    prompts: vec![],
                    error_message: None,
                    tools: vec![McpToolView {
                        name: "connection".to_string(),
                        server: server.name.clone(),
                        description,
                        input_schema: None,
                    }],
                }
            })
            .collect()
    }

    fn open_agents_menu(&mut self) {
        let root = self.project_root();
        self.agents_menu.open(&root);
        self.agents_menu.active_agents = self
            .agent_status
            .iter()
            .enumerate()
            .map(|(idx, (name, status))| AgentInfo {
                id: format!("agent-{}", idx + 1),
                name: name.clone(),
                status: match status.as_str() {
                    "running" => AgentStatus::Running,
                    "waiting" | "waiting_for_tool" => AgentStatus::WaitingForTool,
                    "complete" | "completed" | "done" => AgentStatus::Complete,
                    "failed" | "error" => AgentStatus::Failed,
                    _ => AgentStatus::Idle,
                },
                current_tool: None,
                turns_completed: 0,
                is_coordinator: false,
                last_output: Some(status.clone()),
                agent_role: crate::agents_view::AgentRole::Normal,
                model_name: None,
                cost_usd: 0.0,
            })
            .collect();
    }

    /// Add a message directly (e.g. from a non-streaming source).
    pub fn add_message(&mut self, role: Role, text: String) {
        let msg = match role {
            Role::User => Message::user(text),
            Role::Assistant => Message::assistant(text),
        };
        if role == Role::User {
            self.begin_user_turn_snapshot();
        }
        self.messages.push(msg);
        self.invalidate_transcript();
        self.on_new_message();
    }

    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.sync_turn_metadata_to_messages();
        self.invalidate_transcript();
    }

    pub fn push_message(&mut self, message: Message) {
        if message.role == Role::User {
            self.begin_user_turn_snapshot();
        }
        self.messages.push(message);
        self.sync_turn_metadata_to_messages();
        self.invalidate_transcript();
        self.on_new_message();
    }

    /// Push a synthetic system annotation into the conversation pane.
    /// It will appear after the current last message.
    pub fn push_system_message(&mut self, text: String, style: SystemMessageStyle) {
        self.system_annotations.push(SystemAnnotation {
            after_index: self.messages.len(),
            text,
            style,
        });
        self.invalidate_transcript();
    }

    /// Called whenever a new message is appended to `messages`.
    /// Manages the auto-scroll / new-message-counter state.
    fn on_new_message(&mut self) {
        if self.auto_scroll {
            // Auto-scroll: keep offset at 0 so render shows the bottom.
            self.scroll_offset = 0;
        } else {
            self.new_messages_while_scrolled =
                self.new_messages_while_scrolled.saturating_add(1);
        }
    }

    pub fn invalidate_transcript(&self) {
        self.transcript_version
            .set(self.transcript_version.get().wrapping_add(1));
    }

    /// Check current token usage and push token warning notifications as
    /// appropriate.  Call this after updating `token_count`.
    pub fn check_token_warnings(&mut self) {
        let window =
            claurst_query::context_window_for_model(&self.model_name) as u32;
        if window == 0 {
            return;
        }
        let pct = (self.token_count as f64 / window as f64 * 100.0) as u8;

        // Only escalate — never repeat a threshold already shown.
        if pct >= 100 && self.token_warning_threshold_shown < 100 {
            self.token_warning_threshold_shown = 100;
            self.notifications.push(
                NotificationKind::Error,
                "Context window full. Running auto-compact\u{2026}".to_string(),
                None,
            );
        } else if pct >= 95 && self.token_warning_threshold_shown < 95 {
            self.token_warning_threshold_shown = 95;
            self.notifications.push(
                NotificationKind::Error,
                "Context window 95% full! Run /compact now.".to_string(),
                None, // persistent until dismissed
            );
        } else if pct >= 80 && self.token_warning_threshold_shown < 80 {
            self.token_warning_threshold_shown = 80;
            self.notifications.push(
                NotificationKind::Warning,
                "Context window 80% full. Consider /compact.".to_string(),
                Some(30),
            );
        }
    }

    /// Take the current input buffer, push it to history, and return it.
    pub fn take_input(&mut self) -> String {
        let input = self.prompt_input.take();
        if !input.is_empty() {
            self.prompt_input.history.push(input.clone());
            self.prompt_input.history_pos = None;
            self.prompt_input.history_draft.clear();
            self.input_history = self.prompt_input.history.clone();
            self.history_index = self.prompt_input.history_pos;
        }
        self.refresh_prompt_input();
        input
    }

    /// Compute the number of lines to scroll per wheel/trackpad event.
    /// Implements a simple acceleration model: rapid events (< 40 ms apart) are
    /// treated as trackpad bursts and accelerate up to 2×; slower events (mouse
    /// wheel) stay at the base 3-line step.
    fn scroll_step(&mut self) -> usize {
        let now = std::time::Instant::now();
        let elapsed_ms = self.scroll_last_time
            .map(|t| now.duration_since(t).as_millis())
            .unwrap_or(u128::MAX);
        self.scroll_last_time = Some(now);
        if elapsed_ms < 40 {
            // Trackpad burst — gradually accelerate
            self.scroll_accel = (self.scroll_accel + 0.4).min(6.0);
        } else {
            // Mouse click or first event — reset to base
            self.scroll_accel = 3.0;
        }
        self.scroll_accel.round() as usize
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

    /// Return the elapsed session time as a human-readable string, e.g. "2m 5s".
    pub fn elapsed_str(&self) -> String {
        let secs = self.session_start.elapsed().as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else {
            format!("{}m {}s", secs / 60, secs % 60)
        }
    }

    fn prompt_mode(&self) -> InputMode {
        if self.is_streaming {
            InputMode::Readonly
        } else if self.plan_mode {
            InputMode::Plan
        } else {
            InputMode::Default
        }
    }

    fn sync_legacy_prompt_fields(&mut self) {
        self.input = self.prompt_input.text.clone();
        self.cursor_pos = self.prompt_input.cursor;
        self.history_index = self.prompt_input.history_pos;
    }

    fn refresh_prompt_input(&mut self) {
        self.prompt_input.mode = self.prompt_mode();
        self.prompt_input.update_suggestions(PROMPT_SLASH_COMMANDS);
        self.sync_legacy_prompt_fields();
    }

    pub fn set_prompt_text(&mut self, text: String) {
        self.prompt_input.replace_text(text);
        self.refresh_prompt_input();
    }

    // -----------------------------------------------------------------------
    // Voice PTT helpers
    // -----------------------------------------------------------------------

    /// Start PTT recording: open the microphone capture stream and signal the
    /// UI.  No-op when no voice recorder is attached or recording is already
    /// in progress.
    pub fn handle_voice_ptt_start(&mut self) {
        if self.voice_recording || self.voice_recorder.is_none() {
            return;
        }
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        self.voice_event_rx = Some(rx);
        self.voice_recording = true;
        if let Some(ref recorder_arc) = self.voice_recorder {
            let recorder = recorder_arc.clone();
            tokio::task::spawn_blocking(move || {
                if let Ok(mut r) = recorder.lock() {
                    tokio::runtime::Handle::current()
                        .block_on(r.start_recording(tx))
                        .ok();
                }
            });
        }
        self.status_message = Some("Recording\u{2026} release V or press Enter to transcribe".to_string());
    }

    /// Stop PTT recording: flip the AtomicBool inside VoiceRecorder so the
    /// capture thread exits, then fire a "Transcribing…" notice.  The
    /// transcript text arrives later via `voice_event_rx` and is injected into
    /// the prompt by the event-loop drain.
    pub fn handle_voice_ptt_stop(&mut self) {
        if !self.voice_recording {
            return;
        }
        self.voice_recording = false;
        if let Some(ref recorder_arc) = self.voice_recorder {
            let recorder = recorder_arc.clone();
            tokio::task::spawn_blocking(move || {
                if let Ok(mut r) = recorder.lock() {
                    tokio::runtime::Handle::current()
                        .block_on(r.stop_recording())
                        .ok();
                }
            });
        }
        self.status_message = Some("Transcribing\u{2026}".to_string());
    }

    pub fn attach_turn_diff_state(
        &mut self,
        file_history: Arc<parking_lot::Mutex<FileHistory>>,
        current_turn: Arc<std::sync::atomic::AtomicUsize>,
    ) {
        self.file_history = Some(file_history);
        self.current_turn = Some(current_turn);
        self.refresh_turn_diff_from_history();
    }

    pub fn attach_mcp_manager(&mut self, mcp_manager: Arc<claurst_mcp::McpManager>) {
        self.mcp_manager = Some(mcp_manager);
    }

    pub fn refresh_mcp_view(&mut self) {
        let servers = self.load_mcp_servers();
        self.mcp_view.open(servers);
    }

    pub fn take_pending_mcp_panel_auth(&mut self) -> Option<String> {
        self.pending_mcp_panel_auth.take()
    }

    pub fn take_pending_mcp_reconnect(&mut self) -> bool {
        let pending = self.pending_mcp_reconnect;
        self.pending_mcp_reconnect = false;
        pending
    }

    /// Returns and clears any pending MCP approval result.
    pub fn take_mcp_approval_result(&mut self) -> Option<crate::dialogs::McpApprovalChoice> {
        if !self.mcp_approval.visible {
            return None;
        }
        // The dialog closes itself on confirm; we check if it's now closed
        None // Actual result is read by CLI loop via mcp_approval.visible + confirm()
    }

    /// Detect the current PR from environment variables or git.
    pub fn detect_pr(&mut self) {
        // Check CLAUDE_PR_NUMBER and CLAUDE_PR_URL env vars
        if let Ok(num) = std::env::var("CLAUDE_PR_NUMBER") {
            if let Ok(n) = num.parse::<u32>() {
                self.pr_number = Some(n);
            }
        }
        if let Ok(url) = std::env::var("CLAUDE_PR_URL") {
            self.pr_url = Some(url);
        }
        if let Ok(state) = std::env::var("CLAUDE_PR_STATE") {
            if !state.trim().is_empty() {
                self.pr_state = Some(state.trim().to_string());
            }
        }
        // Fall back to gh CLI if no env vars
        if self.pr_number.is_none() {
            if let Ok(output) = std::process::Command::new("gh")
                .args(["pr", "view", "--json", "number,url", "--jq", ".number,.url"])
                .output()
            {
                if output.status.success() {
                    let text = String::from_utf8_lossy(&output.stdout);
                    let parts: Vec<&str> = text.trim().split('\n').collect();
                    if parts.len() >= 2 {
                        if let Ok(n) = parts[0].trim().parse::<u32>() {
                            self.pr_number = Some(n);
                            self.pr_url = Some(parts[1].trim().to_string());
                        }
                    }
                }
            }
        }
    }

    fn clear_prompt(&mut self) {
        self.prompt_input.clear();
        self.refresh_prompt_input();
    }

    fn refresh_turn_diff_from_history(&mut self) {
        let Some(file_history) = self.file_history.as_ref() else {
            self.diff_viewer.set_turn_diff(Vec::new());
            return;
        };
        let Some(current_turn) = self.current_turn.as_ref() else {
            self.diff_viewer.set_turn_diff(Vec::new());
            return;
        };

        let turn_index = current_turn.load(std::sync::atomic::Ordering::Relaxed);
        if turn_index == 0 {
            self.diff_viewer.set_turn_diff(Vec::new());
            return;
        }

        let root = self.project_root();
        let files = {
            let history = file_history.lock();
            build_turn_diff(&history, turn_index, &root)
        };
        self.diff_viewer.set_turn_diff(files);
    }

    // -------------------------------------------------------------------
    // Event handling
    // -------------------------------------------------------------------

    /// Persist `has_completed_onboarding = true` to the settings file.
    /// Best-effort: failures are silently ignored to not disrupt the session.
    fn persist_onboarding_complete() -> anyhow::Result<()> {
        let mut settings = claurst_core::config::Settings::load_sync()?;
        settings.has_completed_onboarding = true;
        settings.save_sync()
    }

    /// Public wrapper so the main loop can mark onboarding complete without
    /// going through the dialog flow.
    pub fn persist_onboarding_complete_pub() -> anyhow::Result<()> {
        Self::persist_onboarding_complete()
    }

    /// Process a keyboard event. Returns `true` when the input should be
    /// submitted (Enter pressed with no blocking dialog).
    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        if self.global_search.open {
            return self.handle_global_search_key(key);
        }

        // ---- Context menu handling (highest priority for menu navigation) ----
        if self.context_menu_state.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.dismiss_context_menu();
                    return false;
                }
                KeyCode::Up | KeyCode::Down => {
                    self.navigate_context_menu(key.code);
                    return false;
                }
                KeyCode::Enter => {
                    self.execute_context_menu_item();
                    return false;
                }
                _ => {}
            }
        }

        // Bypass-permissions dialog: highest-priority gate — user must accept or the
        // session exits immediately. Mirrors TS BypassPermissionsModeDialog.tsx.
        if self.bypass_permissions_dialog.visible {
            match key.code {
                KeyCode::Char('1') | KeyCode::Esc => {
                    // "No, exit" — quit immediately
                    self.should_quit = true;
                }
                KeyCode::Char('2') => {
                    // "Yes, I accept" — dismiss and continue
                    self.bypass_permissions_dialog.dismiss();
                }
                KeyCode::Up | KeyCode::Char('k') => self.bypass_permissions_dialog.select_prev(),
                KeyCode::Down | KeyCode::Char('j') => self.bypass_permissions_dialog.select_next(),
                KeyCode::Enter => {
                    if self.bypass_permissions_dialog.is_accept_selected() {
                        self.bypass_permissions_dialog.dismiss();
                    } else {
                        self.should_quit = true;
                    }
                }
                _ => {}
            }
            return false;
        }

        // Onboarding dialog: shown on first launch, dismissed with Enter/→/Esc.
        if self.onboarding_dialog.visible {
            match key.code {
                KeyCode::Esc => {
                    self.onboarding_dialog.dismiss();
                }
                KeyCode::Enter | KeyCode::Right => {
                    if self.onboarding_dialog.next_page() {
                        self.onboarding_dialog.dismiss();
                        // Persist that onboarding is complete (best-effort).
                        let _ = Self::persist_onboarding_complete();
                    }
                }
                KeyCode::Left => {
                    self.onboarding_dialog.prev_page();
                }
                _ => {}
            }
            return false;
        }

        // Device code / browser auth dialog (GitHub Copilot, Anthropic OAuth)
        if self.device_auth_dialog.visible {
            match key.code {
                KeyCode::Esc => {
                    self.device_auth_dialog.close();
                    self.device_auth_pending = None;
                }
                _ if matches!(self.device_auth_dialog.status, crate::device_auth_dialog::DeviceAuthStatus::Success(_)) => {
                    // Any key after success -> store credential and close
                    if let crate::device_auth_dialog::DeviceAuthStatus::Success(ref token) = self.device_auth_dialog.status {
                        let provider_id = self.device_auth_dialog.provider_id.clone();
                        let provider_name = self.device_auth_dialog.provider_name.clone();
                        let token = token.clone();
                        let credential = if provider_id == "github-copilot" {
                            claurst_core::StoredCredential::OAuthToken {
                                access: token.clone(),
                                refresh: token,
                                expires: 0,
                            }
                        } else {
                            claurst_core::StoredCredential::ApiKey { key: token }
                        };
                        self.auth_store.set(
                            &provider_id,
                            credential,
                        );
                        self.device_auth_pending = None;
                        self.device_auth_dialog.close();
                        self.activate_provider(provider_id, provider_name, "Connected to");
                        return false;
                    }
                }
                _ if matches!(self.device_auth_dialog.status, crate::device_auth_dialog::DeviceAuthStatus::Error(_)) => {
                    // Any key after error -> close
                    self.device_auth_dialog.close();
                    self.device_auth_pending = None;
                }
                _ => {} // Ignore other keys while waiting
            }
            return false;
        }

        // API key input dialog (opened from /connect for key-based providers)
        if self.key_input_dialog.visible {
            match key.code {
                KeyCode::Esc => {
                    self.key_input_dialog.close();
                }
                KeyCode::Enter => {
                    let provider_id = self.key_input_dialog.provider_id.clone();
                    let provider_name = self.key_input_dialog.provider_name.clone();
                    let api_key = self.key_input_dialog.take_key();
                    if !api_key.is_empty() {
                        self.auth_store.set(
                            &provider_id,
                            claurst_core::StoredCredential::ApiKey { key: api_key },
                        );
                        self.activate_provider(provider_id, provider_name, "Connected to");
                    }
                }
                KeyCode::Backspace => {
                    self.key_input_dialog.backspace();
                }
                KeyCode::Char(c) => {
                    self.key_input_dialog.insert_char(c);
                }
                _ => {}
            }
            return false;
        }

        // Custom provider dialog (URL + API key for OpenAI-compatible providers)
        if self.custom_provider_dialog.visible {
            match key.code {
                KeyCode::Esc => {
                    self.custom_provider_dialog.close();
                }
                KeyCode::Tab | KeyCode::Down => {
                    self.custom_provider_dialog.move_next_field();
                }
                KeyCode::Up => {
                    self.custom_provider_dialog.move_prev_field();
                }
                KeyCode::Enter => {
                    if self.custom_provider_dialog.can_submit() {
                        let provider_id = self.custom_provider_dialog.provider_id.clone();
                        let provider_name = self.custom_provider_dialog.provider_name.clone();
                        let (base_url, api_key) = self.custom_provider_dialog.take_values();
                        self.persist_custom_provider_base_url(&base_url);
                        self.auth_store.set(
                            &provider_id,
                            claurst_core::StoredCredential::ApiKey { key: api_key },
                        );
                        self.activate_provider(provider_id, provider_name, "Connected to");
                    } else {
                        self.custom_provider_dialog.move_next_field();
                    }
                }
                KeyCode::Backspace => {
                    self.custom_provider_dialog.backspace();
                }
                KeyCode::Char(c) => {
                    self.custom_provider_dialog.insert_char(c);
                }
                _ => {}
            }
            return false;
        }

        // Connect-a-provider dialog (/connect command)
        if self.connect_dialog.visible {
            match key.code {
                KeyCode::Esc => { self.connect_dialog.close(); }
                KeyCode::Home => { self.connect_dialog.move_home(); }
                KeyCode::End => { self.connect_dialog.move_end(); }
                KeyCode::Up => { self.connect_dialog.move_up(); }
                KeyCode::Down => { self.connect_dialog.move_down(); }
                KeyCode::PageUp => { self.connect_dialog.page_up(); }
                KeyCode::PageDown => { self.connect_dialog.page_down(); }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => { self.connect_dialog.move_up(); }
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => { self.connect_dialog.move_down(); }
                KeyCode::Enter => {
                    if let Some(selected) = self.connect_dialog.selected().cloned() {
                        self.connect_dialog.close();

                        match selected.id.as_str() {
                            // Local providers — activate immediately, no key needed
                            "ollama" | "lmstudio" | "llamacpp" => {
                                self.activate_provider(selected.id.clone(), selected.title.clone(), "Switched to");
                            }
                            "anthropic" => {
                                // Anthropic: use API key from console.anthropic.com
                                // (OAuth requires a registered app which Claurst doesn't have)
                                self.key_input_dialog.open(selected.id.clone(), selected.title.clone());
                            }
                            "custom-openai" => {
                                let current_url = Settings::load_sync()
                                    .ok()
                                    .and_then(|settings| settings.providers.get("custom-openai").and_then(|p| p.api_base.clone()));
                                self.custom_provider_dialog
                                    .open(selected.id.clone(), selected.title.clone(), current_url);
                            }
                            "github-copilot" => {
                                // GitHub Copilot: device code flow
                                self.device_auth_dialog.open(selected.id.clone(), selected.title.clone());
                                self.device_auth_pending = Some("github-copilot".to_string());
                            }
                            "codex" | "openai-codex" => {
                                // OpenAI Codex: browser OAuth flow (spawned by main loop)
                                self.device_auth_dialog.open("openai-codex".into(), "OpenAI Codex".into());
                                self.device_auth_pending = Some("openai-codex".to_string());
                            }
                            // AWS Bedrock — accept a bearer token via key input dialog
                            "amazon-bedrock" => {
                                self.key_input_dialog
                                    .open(selected.id.clone(), selected.title.clone());
                            }
                            // All other providers — open API key input dialog
                            _ => {
                                self.key_input_dialog
                                    .open(selected.id.clone(), selected.title.clone());
                            }
                        }
                    }
                }
                KeyCode::Backspace => { self.connect_dialog.filter_pop(); }
                KeyCode::Char(c) => { self.connect_dialog.filter_push(c); }
                _ => {}
            }
            return false;
        }

        // Import-config source picker
        if self.import_config_picker.visible {
            match key.code {
                KeyCode::Esc => { self.import_config_picker.close(); }
                KeyCode::Home => { self.import_config_picker.move_home(); }
                KeyCode::End => { self.import_config_picker.move_end(); }
                KeyCode::Up => { self.import_config_picker.move_up(); }
                KeyCode::Down => { self.import_config_picker.move_down(); }
                KeyCode::PageUp => { self.import_config_picker.page_up(); }
                KeyCode::PageDown => { self.import_config_picker.page_down(); }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => { self.import_config_picker.move_up(); }
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => { self.import_config_picker.move_down(); }
                KeyCode::Enter => {
                    if let Some(selected) = self.import_config_picker.selected().cloned() {
                        self.import_config_picker.close();
                        if let Some(selection) = Self::import_selection_from_picker(&selected.id) {
                            self.open_import_config_preview(selection);
                        }
                    }
                }
                KeyCode::Backspace => { self.import_config_picker.filter_pop(); }
                KeyCode::Char(c) => { self.import_config_picker.filter_push(c); }
                _ => {}
            }
            return false;
        }

        // Import-config preview dialog
        if self.import_config_dialog.visible {
            match key.code {
                KeyCode::Esc => self.import_config_dialog.close(),
                KeyCode::Enter => self.perform_import_config(),
                _ => {}
            }
            return false;
        }

        // Command palette (Ctrl+K)
        if self.command_palette.visible {
            match key.code {
                KeyCode::Esc => { self.command_palette.close(); }
                KeyCode::Home => { self.command_palette.move_home(); }
                KeyCode::End => { self.command_palette.move_end(); }
                KeyCode::Up => { self.command_palette.move_up(); }
                KeyCode::Down => { self.command_palette.move_down(); }
                KeyCode::PageUp => { self.command_palette.page_up(); }
                KeyCode::PageDown => { self.command_palette.page_down(); }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => { self.command_palette.move_up(); }
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => { self.command_palette.move_down(); }
                KeyCode::Enter => {
                    if let Some(selected) = self.command_palette.selected().cloned() {
                        self.command_palette.close();
                        // Put the command in the input and signal for execution
                        self.prompt_input.replace_text(selected.id.clone());
                        return true; // signal to submit this as input
                    }
                }
                KeyCode::Backspace => { self.command_palette.filter_pop(); }
                KeyCode::Char(c) => { self.command_palette.filter_push(c); }
                _ => {}
            }
            return false;
        }

        // Invalid-config dialog intercepts Enter/Esc to dismiss
        if self.invalid_config_dialog.visible {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => self.invalid_config_dialog.dismiss(),
                KeyCode::Up => self.invalid_config_dialog.scroll_up(),
                KeyCode::Down => self.invalid_config_dialog.scroll_down(20),
                _ => {}
            }
            return false;
        }

        // Model picker intercepts navigation and Esc
        if self.model_picker.visible {
            match key.code {
                KeyCode::Esc => self.model_picker.close(),
                KeyCode::Home => self.model_picker.select_first(),
                KeyCode::End => self.model_picker.select_last(),
                KeyCode::Up => self.model_picker.select_prev(),
                KeyCode::Down => self.model_picker.select_next(),
                KeyCode::Left => self.model_picker.effort_prev(),
                KeyCode::Right => self.model_picker.effort_next(),
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => self.model_picker.select_prev(),
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => self.model_picker.select_next(),
                KeyCode::Enter => {
                    if let Some((model_id, effort)) = self.model_picker.confirm() {
                        // If user picked a model other than the fast-mode model
                        // while fast mode was active, turn fast mode off.
                        if self.fast_mode && !self.model_picker.is_selected_fast_mode_model(&model_id) {
                            self.fast_mode = false;
                        }
                        if let Some(e) = effort {
                            self.effort_level = e;
                        }
                        // Store explicit selections in the canonical
                        // "provider/model" form for non-Anthropic providers.
                        let provider = self.config.provider.as_deref().unwrap_or("anthropic");
                        let full_model = if provider == "anthropic" {
                            model_id.clone()
                        } else {
                            format!("{}/{}", provider, model_id)
                        };
                        self.set_model(full_model.clone());
                        self.persist_provider_and_model();
                        let effort_hint = effort.map(|e| format!(" [{}]", e.label())).unwrap_or_default();
                        self.status_message = Some(format!("Model: {}{}", full_model, effort_hint));
                    }
                }
                KeyCode::Backspace => self.model_picker.pop_filter_char(),
                KeyCode::Char(c) => self.model_picker.push_filter_char(c),
                _ => {}
            }
            return false;
        }

        // Session branching overlay intercepts navigation and Esc
        if self.session_branching.visible {
            use crate::session_branching::BranchBrowserMode;
            match self.session_branching.mode {
                BranchBrowserMode::Browse => {
                    match key.code {
                        KeyCode::Esc => self.session_branching.cancel(),
                        KeyCode::Up => self.session_branching.select_prev(),
                        KeyCode::Down => self.session_branching.select_next(),
                        KeyCode::Char('n') => self.session_branching.start_create_new(),
                        KeyCode::Char('d') => self.session_branching.start_delete_confirm(),
                        KeyCode::Enter => {
                            if let Some(branch) = self.session_branching.selected_branch() {
                                self.status_message = Some(format!("Switched to branch: {}", branch.name));
                                self.session_branching.close();
                            }
                        }
                        _ => {}
                    }
                }
                BranchBrowserMode::CreateNew => {
                    match key.code {
                        KeyCode::Esc => self.session_branching.cancel(),
                        KeyCode::Enter => {
                            if let Some((name, at_msg)) = self.session_branching.confirm_create_new() {
                                self.status_message = Some(format!("Created branch: {} at message {}", name, at_msg));
                                self.session_branching.close();
                            }
                        }
                        KeyCode::Backspace => self.session_branching.pop_create_char(),
                        KeyCode::Char(c) => self.session_branching.push_create_char(c),
                        _ => {}
                    }
                }
                BranchBrowserMode::ConfirmDelete => {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('n') => self.session_branching.cancel(),
                        KeyCode::Enter | KeyCode::Char('y') => {
                            if let Some(branch_id) = self.session_branching.confirm_delete() {
                                self.status_message = Some(format!("Deleted branch: {}", branch_id));
                            }
                        }
                        _ => {}
                    }
                }
            }
            return false;
        }

        // Session browser intercepts navigation and Esc
        if self.session_browser.visible {
            use crate::session_browser::SessionBrowserMode;
            match self.session_browser.mode {
                SessionBrowserMode::Browse => {
                    match key.code {
                        KeyCode::Esc => self.session_browser.close(),
                        KeyCode::Up => self.session_browser.select_prev(),
                        KeyCode::Down => self.session_browser.select_next(),
                        KeyCode::Char('r') => self.session_browser.start_rename(),
                        _ => {}
                    }
                }
                SessionBrowserMode::Rename => {
                    match key.code {
                        KeyCode::Esc => self.session_browser.cancel(),
                        KeyCode::Enter => {
                            if let Some((_id, name)) = self.session_browser.confirm_rename() {
                                self.session_title = Some(name.clone());
                                self.status_message = Some(format!("Renamed to: {}", name));
                            }
                        }
                        KeyCode::Backspace => self.session_browser.pop_rename_char(),
                        KeyCode::Char(c) => self.session_browser.push_rename_char(c),
                        _ => {}
                    }
                }
                SessionBrowserMode::Confirm => {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('n') => self.session_browser.cancel(),
                        KeyCode::Enter | KeyCode::Char('y') => {
                            self.session_browser.close();
                        }
                        _ => {}
                    }
                }
            }
            return false;
        }

        // Tasks overlay intercepts navigation and Esc
        if self.tasks_overlay.visible {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.tasks_overlay.close(),
                KeyCode::Up => self.tasks_overlay.select_prev(),
                KeyCode::Down => self.tasks_overlay.select_next(),
                KeyCode::Enter => {
                    if let Some((task_id, new_status)) = self.tasks_overlay.cycle_and_persist_status() {
                        self.status_message = Some(format!("Task {} → {}", task_id, new_status));
                    }
                }
                _ => {}
            }
            return false;
        }

        // Export dialog key handling
        if self.export_dialog.visible {
            match key.code {
                KeyCode::Esc => {
                    self.export_dialog.dismiss();
                }
                KeyCode::Enter => {
                    if let Some(path) = self.perform_export() {
                        self.notifications.push(
                            NotificationKind::Info,
                            format!("Exported to {}", path),
                            Some(4),
                        );
                    } else {
                        self.notifications.push(
                            NotificationKind::Warning,
                            "Export failed: could not write file.".to_string(),
                            Some(4),
                        );
                    }
                }
                KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                    self.export_dialog.toggle();
                }
                KeyCode::Char('1') => {
                    self.export_dialog.selected = ExportFormat::Json;
                }
                KeyCode::Char('2') => {
                    self.export_dialog.selected = ExportFormat::Markdown;
                }
                _ => {}
            }
            return false;
        }

        // Context visualization overlay key handling
        if self.context_viz.visible {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.context_viz.close();
                }
                _ => {}
            }
            return false;
        }

        // MCP approval dialog
        if self.mcp_approval.visible {
            let result = crate::dialogs::handle_mcp_approval_key(&mut self.mcp_approval, key);
            if result.is_some() {
                // Result processed by CLI loop via take_mcp_approval_result()
            }
            return false;
        }

        // Feedback survey intercepts digit keys and Esc
        if self.feedback_survey.visible {
            if key.code == KeyCode::Esc {
                self.feedback_survey.close();
                return false;
            }
            if let KeyCode::Char(c) = key.code {
                if let Some(d) = c.to_digit(10) {
                    self.feedback_survey.handle_digit(d as u8);
                    return false;
                }
            }
            return false;
        }

        // Memory file selector intercepts navigation and Esc
        if self.memory_file_selector.visible {
            match key.code {
                KeyCode::Esc => self.memory_file_selector.close(),
                KeyCode::Up => self.memory_file_selector.select_prev(),
                KeyCode::Down => self.memory_file_selector.select_next(),
                KeyCode::Enter => {
                    // Selection acknowledged — consumer can read selected_path()
                    self.memory_file_selector.close();
                }
                _ => {}
            }
            return false;
        }

        // Hooks config menu intercepts navigation and Esc
        if self.hooks_config_menu.visible {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.hooks_config_menu.back(),
                KeyCode::Enter => self.hooks_config_menu.enter(),
                KeyCode::Up | KeyCode::Char('k') => self.hooks_config_menu.select_prev(),
                KeyCode::Down | KeyCode::Char('j') => self.hooks_config_menu.select_next(),
                _ => {}
            }
            return false;
        }

        if self.diff_viewer.open {
            self.handle_diff_viewer_key(key);
            return false;
        }

        if self.agents_menu.open {
            self.handle_agents_menu_key(key);
            return false;
        }

        if self.mcp_view.open {
            return self.handle_mcp_view_key(key);
        }

        if self.stats_dialog.open {
            self.handle_stats_dialog_key(key);
            return false;
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

        if self.global_search.open {
            return self.handle_global_search_key(key);
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

        // Overage upsell dismiss
        if key.code == KeyCode::Esc && self.overage_upsell.visible {
            self.overage_upsell.dismiss();
            return false;
        }

        // Voice mode notice dismiss
        if key.code == KeyCode::Esc && self.voice_mode_notice.visible {
            self.voice_mode_notice.dismiss();
            return false;
        }

        // Cancel an active voice recording with Esc.
        if key.code == KeyCode::Esc && self.voice_recording {
            self.voice_recording = false;
            self.voice_event_rx = None;
            if let Some(ref recorder_arc) = self.voice_recorder {
                let recorder = recorder_arc.clone();
                tokio::task::spawn_blocking(move || {
                    if let Ok(mut r) = recorder.lock() {
                        tokio::runtime::Handle::current()
                            .block_on(r.stop_recording())
                            .ok();
                    }
                });
            }
            self.status_message = Some("Recording cancelled.".to_string());
            return false;
        }

        // Desktop upsell startup dialog
        if self.desktop_upsell.visible {
            match key.code {
                KeyCode::Up | KeyCode::BackTab => {
                    self.desktop_upsell.select_prev();
                    return false;
                }
                KeyCode::Down | KeyCode::Tab => {
                    self.desktop_upsell.select_next();
                    return false;
                }
                KeyCode::Enter => {
                    self.desktop_upsell.confirm();
                    return false;
                }
                KeyCode::Esc => {
                    self.desktop_upsell.dismiss_temporarily();
                    return false;
                }
                _ => return false,
            }
        }

        // Memory update notification dismiss
        if key.code == KeyCode::Esc && self.memory_update_notification.visible {
            self.memory_update_notification.dismiss();
            return false;
        }

        // MCP elicitation dialog — highest priority modal
        if self.elicitation.visible {
            match key.code {
                KeyCode::Esc => {
                    self.elicitation.cancel();
                    return false;
                }
                KeyCode::Enter => {
                    self.elicitation.submit();
                    return false;
                }
                KeyCode::Tab | KeyCode::Down => {
                    if let crossterm::event::KeyModifiers::SHIFT = key.modifiers {
                        self.elicitation.prev_field();
                    } else {
                        self.elicitation.next_field();
                    }
                    return false;
                }
                KeyCode::BackTab | KeyCode::Up => {
                    self.elicitation.prev_field();
                    return false;
                }
                KeyCode::Left => {
                    self.elicitation.cycle_enum_prev();
                    return false;
                }
                KeyCode::Right => {
                    self.elicitation.cycle_enum_next();
                    return false;
                }
                KeyCode::Char(' ') => {
                    self.elicitation.toggle_active();
                    return false;
                }
                KeyCode::Backspace => {
                    self.elicitation.backspace();
                    return false;
                }
                KeyCode::Char(c) => {
                    self.elicitation.insert_char(c);
                    return false;
                }
                _ => return false,
            }
        }

        // ---- Keybinding processor (runs AFTER all dialog checks) ----------
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

        // Clear any active text selection on key press (except Ctrl+C which copies it).
        let is_copy = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        if !is_copy && self.selection_anchor.is_some() {
            self.selection_anchor = None;
            self.selection_focus = None;
            *self.selection_text.borrow_mut() = String::new();
        }

        // ---- Voice hold-to-talk (Alt+V toggles recording on/off) ----------
        if key.code == KeyCode::Char('v')
            && key.modifiers.contains(KeyModifiers::ALT)
            && self.voice_recorder.is_some()
        {
            if !self.voice_recording {
                // First press: start recording.
                let (tx, rx) = tokio::sync::mpsc::channel(8);
                self.voice_event_rx = Some(rx);
                self.voice_recording = true;
                if let Some(ref recorder_arc) = self.voice_recorder {
                    let recorder = recorder_arc.clone();
                    // Use spawn_blocking so we don't hold a std::sync::MutexGuard
                    // across an await point.  start_recording internally spawns a
                    // tokio task and returns quickly, so blocking is negligible.
                    tokio::task::spawn_blocking(move || {
                        if let Ok(mut r) = recorder.lock() {
                            // start_recording is async but its real work happens in
                            // a spawned task; use block_on to drive the short setup.
                            tokio::runtime::Handle::current()
                                .block_on(r.start_recording(tx))
                                .ok();
                        }
                    });
                }
                self.notifications.push(
                    NotificationKind::Info,
                    "Recording\u{2026} (Alt+V to transcribe · Esc to cancel)".to_string(),
                    None,
                );
            } else {
                // Second press: stop recording.  stop_recording() just flips an
                // AtomicBool; drive it synchronously to avoid Send issues.
                self.voice_recording = false;
                if let Some(ref recorder_arc) = self.voice_recorder {
                    let recorder = recorder_arc.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Ok(mut r) = recorder.lock() {
                            tokio::runtime::Handle::current()
                                .block_on(r.stop_recording())
                                .ok();
                        }
                    });
                }
                self.notifications.push(
                    NotificationKind::Info,
                    "Transcribing\u{2026}".to_string(),
                    Some(10),
                );
            }
            return false;
        }

        // ---- Voice PTT: plain V press starts recording when voice is on ----
        // This is the "hold to talk" variant.  The user presses V to begin
        // recording; releasing V (handled in the run loop) or pressing Enter
        // stops the capture and triggers transcription.
        // Only active when voice mode is enabled (voice_recorder is Some) and
        // the prompt input is in default (non-vim) mode so 'v' doesn't conflict
        // with vim keybindings.
        if key.code == KeyCode::Char('v')
            && key.modifiers == KeyModifiers::NONE
            && self.voice_recorder.is_some()
            && !self.voice_recording
            && self.prompt_input.vim_mode == crate::prompt_input::VimMode::Insert
        {
            self.handle_voice_ptt_start();
            return false;
        }

        // ---- Ctrl+V — clipboard paste (image first, then text fallback) ----
        // Only fires when NOT in vim Normal/Visual/VisualBlock mode (where \x16 is
        // already consumed by the vim handler above to enter VisualBlock mode).
        if key.code == KeyCode::Char('v')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && !matches!(
                self.prompt_input.vim_mode,
                crate::prompt_input::VimMode::Normal
                    | crate::prompt_input::VimMode::Visual
                    | crate::prompt_input::VimMode::VisualBlock
            )
        {
            use crate::image_paste::{read_clipboard_image, read_clipboard_text, read_primary_text};
            if let Some(img) = read_clipboard_image() {
                let label = img.label.clone();
                let dims = img.dimensions;
                self.prompt_input.add_image(img);
                let msg = if let Some((w, h)) = dims {
                    format!("Image attached: {} ({}x{})", label, w, h)
                } else {
                    format!("Image attached: {}", label)
                };
                self.notifications.push(NotificationKind::Info, msg, Some(3));
            } else if let Some(text) = read_clipboard_text().or_else(read_primary_text) {
                self.prompt_input.paste(&text);
                self.refresh_prompt_input();
            }
            return false;
        }

        // ---- Shift+Insert — selection/clipboard paste fallback -------------
        if key.code == KeyCode::Insert && key.modifiers.contains(KeyModifiers::SHIFT) {
            let _ = self.paste_primary_into_prompt();
            return false;
        }

        // ---- Enter while PTT recording: stop capture instead of submitting ----
        if key.code == KeyCode::Enter
            && self.voice_recording
            && self.voice_recorder.is_some()
        {
            self.handle_voice_ptt_stop();
            return false;
        }

        // ---- Focus state machine: transcript mode --------------------------
        // When the transcript pane has focus, intercept Escape and scroll keys.
        // Printable characters switch focus back to Input and fall through so the
        // keystroke is processed normally by the prompt editor below.
        if self.focus == FocusTarget::Transcript {
            match key.code {
                KeyCode::Esc => {
                    self.focus = FocusTarget::Input;
                    return false;
                }
                KeyCode::PageUp | KeyCode::PageDown => {
                    // Let these fall through to the normal scroll handling below.
                }
                KeyCode::Char(_) if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    // Printable char: switch focus to Input and process normally.
                    self.focus = FocusTarget::Input;
                }
                _ => {}
            }
        }

        match key.code {
            // ---- ESC: cancel streaming (status bar advertises "esc interrupt") ----
            KeyCode::Esc if self.is_streaming => {
                self.is_streaming = false;
                self.spinner_verb = None;
                self.streaming_text.clear();
                self.streaming_thinking.clear();
                self.tool_use_blocks.clear();
                self.status_message = Some("Cancelled.".to_string());
                self.complete_current_turn_snapshot(true);
            }

            // ---- Quit / cancel ----------------------------------------
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // If text is selected, copy it to clipboard instead of quitting.
                let sel_text = self.selection_text.borrow().clone();
                if self.selection_anchor.is_some() && !sel_text.is_empty() {
                    let copied = crate::image_paste::write_clipboard_text(&sel_text);
                    self.selection_anchor = None;
                    self.selection_focus = None;
                    *self.selection_text.borrow_mut() = String::new();
                    if copied {
                        self.notifications.push(NotificationKind::Info, "Copied to clipboard".to_string(), Some(2));
                    }
                } else if self.is_streaming {
                    self.is_streaming = false;
                    self.spinner_verb = None;
                    self.streaming_text.clear();
                    self.streaming_thinking.clear();
                    self.tool_use_blocks.clear();
                    self.status_message = Some("Cancelled.".to_string());
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.prompt_input.is_empty() {
                    self.should_quit = true;
                }
            }

            // ---- History search ----------------------------------------
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Open the new overlay-based history search
                let overlay = HistorySearchOverlay::open(&self.prompt_input.history);
                self.history_search_overlay = overlay;
                // Also open legacy for backwards compat
                let mut hs = HistorySearch::new();
                hs.update_matches(&self.prompt_input.history);
                self.history_search = Some(hs);
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.global_search.open();
                self.refresh_global_search();
            }

            // ---- Tasks overlay (Ctrl+T) --------------------------------
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tasks_overlay.toggle();
            }

            // ---- Help overlay ------------------------------------------
            KeyCode::F(1) => {
                self.show_help = !self.show_help;
                self.help_overlay.toggle();
            }
            KeyCode::Char('?')
                if !self.is_streaming
                    && self.prompt_input.is_empty()
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && !key.modifiers.contains(KeyModifiers::SUPER) =>
            {
                self.show_help = !self.show_help;
                self.help_overlay.toggle();
            }

            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) && !self.is_streaming => {
                self.prompt_input.kill_line_backward();
                self.refresh_prompt_input();
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) && !self.is_streaming => {
                self.prompt_input.kill_word_backward();
                self.refresh_prompt_input();
            }
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) && !self.is_streaming => {
                self.prompt_input.yank();
                self.refresh_prompt_input();
            }

            // ---- Alt/Meta key text editing operations -------------------
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::ALT) && !self.is_streaming => {
                self.prompt_input.yank_pop();
                self.refresh_prompt_input();
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) && !self.is_streaming => {
                self.prompt_input.delete_word_backward();
                self.refresh_prompt_input();
            }
            KeyCode::Delete if key.modifiers.contains(KeyModifiers::ALT) && !self.is_streaming => {
                self.prompt_input.delete_word_forward();
                self.refresh_prompt_input();
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) && !self.is_streaming => {
                self.prompt_input.move_word_backward();
                self.sync_legacy_prompt_fields();
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) && !self.is_streaming => {
                self.prompt_input.move_word_forward();
                self.sync_legacy_prompt_fields();
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::ALT) && !self.is_streaming => {
                self.prompt_input.delete_word_at_cursor();
                self.refresh_prompt_input();
            }

            // ---- Text entry (blocked while streaming) ------------------
            KeyCode::Char(c) if !self.is_streaming => {
                if self.prompt_input.vim_enabled && self.prompt_input.vim_mode != VimMode::Insert {
                    self.prompt_input.vim_command(&c.to_string());
                } else {
                    self.prompt_input.insert_char(c);
                }
                self.refresh_prompt_input();
            }
            KeyCode::Backspace if !self.is_streaming => {
                self.prompt_input.backspace();
                self.refresh_prompt_input();
            }
            KeyCode::Delete if !self.is_streaming => {
                self.prompt_input.delete();
                self.refresh_prompt_input();
            }
            KeyCode::Left if !self.is_streaming => {
                self.prompt_input.move_left();
                self.sync_legacy_prompt_fields();
            }
            KeyCode::Right if !self.is_streaming => {
                self.prompt_input.move_right();
                self.sync_legacy_prompt_fields();
            }
            KeyCode::Home if !self.is_streaming => {
                self.prompt_input.cursor = 0;
                self.sync_legacy_prompt_fields();
            }
            KeyCode::End if !self.is_streaming => {
                self.prompt_input.cursor = self.prompt_input.text.len();
                self.sync_legacy_prompt_fields();
            }
            KeyCode::Tab if !self.is_streaming => {
                if !self.prompt_input.suggestions.is_empty() {
                    // Accept slash-command suggestion
                    if self.prompt_input.suggestion_index.is_none() {
                        self.prompt_input.suggestion_index = Some(0);
                    }
                    self.prompt_input.accept_suggestion();
                    self.refresh_prompt_input();
                } else if self.prompt_input.is_empty() {
                    // Cycle agent mode: build → plan → explore → build
                    self.cycle_agent_mode();
                    self.rustle_look_down();
                }
            }

            // ---- Shift+Tab: cycle permission mode ----------------------
            // Default → AcceptEdits → BypassPermissions → Default
            // Mirrors TS bottom-left indicator cycling behaviour.
            KeyCode::BackTab if !self.is_streaming => {
                use claurst_core::config::PermissionMode;
                self.config.permission_mode = match self.config.permission_mode {
                    PermissionMode::Default => PermissionMode::AcceptEdits,
                    PermissionMode::AcceptEdits => PermissionMode::BypassPermissions,
                    PermissionMode::BypassPermissions => PermissionMode::Default,
                    PermissionMode::Plan => PermissionMode::Default,
                };
                let label = match self.config.permission_mode {
                    PermissionMode::Default => "Default permissions",
                    PermissionMode::AcceptEdits => "Accept-edits mode",
                    PermissionMode::BypassPermissions => "Bypass permissions (dangerous)",
                    PermissionMode::Plan => "Plan mode",
                };
                self.status_message = Some(label.to_string());
            }

            // ---- Submit ------------------------------------------------
            KeyCode::Enter if !self.is_streaming => {
                // If a slash-command suggestion is selected, accept it instead of submitting.
                if !self.prompt_input.suggestions.is_empty()
                    && self.prompt_input.suggestion_index.is_some()
                    && self.prompt_input.text.starts_with('/')
                {
                    self.prompt_input.accept_suggestion();
                    self.refresh_prompt_input();
                    return false;
                }
                // New user input: snap back to bottom.
                self.auto_scroll = true;
                self.new_messages_while_scrolled = 0;
                self.scroll_offset = 0;
                return true;
            }

            // ---- Message boundary navigation (Alt+Up/Alt+Down) ----------
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                // Jump up by ~20 lines (approximate message boundary).
                self.scroll_offset = self.scroll_offset.saturating_add(20);
                self.auto_scroll = false;
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                // Jump down by ~20 lines (approximate message boundary).
                let new_off = self.scroll_offset.saturating_sub(20);
                self.scroll_offset = new_off;
                if new_off == 0 {
                    self.auto_scroll = true;
                    self.new_messages_while_scrolled = 0;
                }
            }

            // ---- Input history navigation ------------------------------
            KeyCode::Up => {
                if !self.prompt_input.suggestions.is_empty() && self.prompt_input.text.starts_with('/') {
                    self.prompt_input.suggestion_prev();
                } else if !self.prompt_input.history.is_empty() {
                    self.prompt_input.history_up();
                }
                self.refresh_prompt_input();
            }
            KeyCode::Down => {
                if !self.prompt_input.suggestions.is_empty() && self.prompt_input.text.starts_with('/') {
                    self.prompt_input.suggestion_next();
                } else if self.prompt_input.history_pos.is_some() {
                    self.prompt_input.history_down();
                }
                self.refresh_prompt_input();
            }

            // ---- Scroll ------------------------------------------------
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
                // Scrolling up disables auto-follow.
                self.auto_scroll = false;
            }
            KeyCode::PageDown => {
                let new_off = self.scroll_offset.saturating_sub(10);
                self.scroll_offset = new_off;
                if new_off == 0 {
                    // Scrolled all the way back to bottom — re-enable auto-follow.
                    self.auto_scroll = true;
                    self.new_messages_while_scrolled = 0;
                }
            }

            // ---- Toggle last thinking block (t key) -------------------
            KeyCode::Char('t') if !self.is_streaming => {
                // Find the last thinking block in the message list and toggle it
                use claurst_core::types::ContentBlock;
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                'outer: for msg in self.messages.iter().rev() {
                    let blocks = msg.content_blocks();
                    for block in blocks.iter().rev() {
                        if let ContentBlock::Thinking { thinking, .. } = block {
                            let mut h = DefaultHasher::new();
                            thinking.hash(&mut h);
                            let hash = h.finish();
                            if self.thinking_expanded.contains(&hash) {
                                self.thinking_expanded.remove(&hash);
                            } else {
                                self.thinking_expanded.insert(hash);
                            }
                            self.invalidate_transcript();
                            break 'outer;
                        }
                    }
                }
            }

            _ => {}
        }
        false
    }

    fn current_key_context(&self) -> KeyContext {
        if self.diff_viewer.open {
            KeyContext::DiffDialog
        } else if self.agents_menu.open || self.mcp_view.open || self.stats_dialog.open {
            KeyContext::Select
        } else if self.import_config_dialog.visible {
            KeyContext::Confirmation
        } else if self.settings_screen.visible {
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

    fn handle_stats_dialog_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.stats_dialog.close(),
            KeyCode::Tab | KeyCode::Right => self.stats_dialog.next_tab(),
            KeyCode::BackTab | KeyCode::Left => self.stats_dialog.prev_tab(),
            KeyCode::Char('r') => self.stats_dialog.cycle_range(),
            KeyCode::Up => self.stats_dialog.scroll = self.stats_dialog.scroll.saturating_sub(1),
            KeyCode::Down => self.stats_dialog.scroll = self.stats_dialog.scroll.saturating_add(1),
            _ => {}
        }
    }

    fn handle_mcp_view_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mcp_view.close(),
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => self.mcp_view.switch_pane(),
            KeyCode::Up => self.mcp_view.select_prev(),
            KeyCode::Down => self.mcp_view.select_next(),
            KeyCode::Backspace => self.mcp_view.pop_search_char(),
            KeyCode::Char('e') => self.mcp_view.toggle_error_detail(),
            KeyCode::Char('a')
                if self.mcp_view.active_pane == crate::mcp_view::McpViewPane::ServerList =>
            {
                let selected_server = self
                    .mcp_view
                    .servers
                    .get(self.mcp_view.selected_server)
                    .map(|server| server.name.clone());
                if let Some(server_name) = selected_server {
                    self.pending_mcp_panel_auth = Some(server_name);
                    self.mcp_view.close();
                    self.status_message = Some("Starting MCP auth...".to_string());
                }
            }
            KeyCode::Char('r') => {
                self.pending_mcp_reconnect = true;
                self.status_message = Some("Reconnecting MCP runtime...".to_string());
            }
            KeyCode::Char(c) if key.modifiers.is_empty() => {
                if self.mcp_view.active_pane != crate::mcp_view::McpViewPane::ServerList {
                    self.mcp_view.push_search_char(c);
                }
            }
            _ => {}
        }
        false
    }

    fn handle_agents_menu_key(&mut self, key: KeyEvent) {
        if matches!(self.agents_menu.route, AgentsRoute::Editor(_)) {
            match key.code {
                KeyCode::Esc => self.agents_menu.go_back(),
                KeyCode::Tab | KeyCode::Down => self.agents_menu.editor_next_field(),
                KeyCode::BackTab | KeyCode::Up => self.agents_menu.editor_prev_field(),
                KeyCode::Enter => self.agents_menu.editor_insert_newline(),
                KeyCode::Backspace => self.agents_menu.editor_backspace(),
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    match self.agents_menu.save_editor() {
                        Ok(msg) => self.status_message = Some(msg),
                        Err(err) => {
                            self.agents_menu.editor.error = Some(err.clone());
                            self.agents_menu.editor.saved_message = None;
                            self.status_message = Some(err);
                        }
                    }
                }
                KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.agents_menu.editor_insert_char(ch);
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => self.agents_menu.go_back(),
            KeyCode::Up => self.agents_menu.select_prev(),
            KeyCode::Down => self.agents_menu.select_next(),
            KeyCode::Enter | KeyCode::Right => self.agents_menu.confirm_selection(),
            KeyCode::Left => self.agents_menu.go_back(),
            _ => {}
        }
    }

    fn handle_diff_viewer_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.diff_viewer.close(),
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => self.diff_viewer.switch_pane(),
            KeyCode::Char('d') => {
                let root = self.project_root();
                self.diff_viewer.toggle_diff_type(&root);
            }
            KeyCode::Up => {
                if self.diff_viewer.active_pane == DiffPane::FileList {
                    self.diff_viewer.select_prev();
                } else {
                    self.diff_viewer.scroll_detail_up();
                }
            }
            KeyCode::Down => {
                if self.diff_viewer.active_pane == DiffPane::FileList {
                    self.diff_viewer.select_next();
                } else {
                    self.diff_viewer.scroll_detail_down();
                }
            }
            KeyCode::PageUp => self.diff_viewer.scroll_detail_up(),
            KeyCode::PageDown => self.diff_viewer.scroll_detail_down(),
            KeyCode::Char(' ') => {
                if self.diff_viewer.active_pane == DiffPane::FileList {
                    self.diff_viewer.toggle_file_collapse();
                }
            }
            _ => {}
        }
    }

    fn handle_help_overlay_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::F(1) => {
                self.help_overlay.close();
                self.show_help = false;
            }
            KeyCode::Char('?')
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && !key.modifiers.contains(KeyModifiers::SUPER) =>
            {
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
                    .current_entry(&self.prompt_input.history)
                {
                    self.set_prompt_text(entry.to_string());
                }
                self.history_search_overlay.close();
                self.history_search = None;
            }
            KeyCode::Up => {
                self.history_search_overlay.select_prev();
                if let Some(hs) = self.history_search.as_mut() {
                    let count = hs.matches.len();
                    if count > 0 {
                        if hs.selected == 0 {
                            hs.selected = count - 1;
                        } else {
                            hs.selected -= 1;
                        }
                    }
                }
            }
            KeyCode::Down => {
                self.history_search_overlay.select_next();
                if let Some(hs) = self.history_search.as_mut() {
                    let count = hs.matches.len();
                    if count > 0 {
                        hs.selected = (hs.selected + 1) % count;
                    }
                }
            }
            KeyCode::Backspace => {
                let history = self.prompt_input.history.clone();
                self.history_search_overlay.pop_char(&history);
                if let Some(hs) = self.history_search.as_mut() {
                    hs.query.pop();
                    hs.update_matches(&history);
                }
            }
            // 'p' with no modifiers and an empty query = pin/unpin the selected entry.
            // When the query is non-empty 'p' is treated as a filter character so
            // the user can still search for prompts containing the letter 'p'.
            KeyCode::Char('p')
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.history_search_overlay.query.is_empty() =>
            {
                self.history_search_overlay.toggle_pin();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let history = self.prompt_input.history.clone();
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
                        // Truncate conversation to the selected message index.
                        self.messages.truncate(idx);
                        // Remove system annotations placed after the truncation point.
                        self.system_annotations.retain(|a| a.after_index <= idx);
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

    fn handle_global_search_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.global_search.close();
            }
            KeyCode::Enter => {
                if let Some(selected) = self.global_search.selected_ref() {
                    self.set_prompt_text(selected);
                }
                self.global_search.close();
            }
            KeyCode::Up => self.global_search.select_prev(),
            KeyCode::Down => self.global_search.select_next(),
            KeyCode::Backspace => {
                self.global_search.pop_char();
                self.refresh_global_search();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.global_search.push_char(c);
                self.refresh_global_search();
            }
            _ => {}
        }
        false
    }

    fn handle_keybinding_action(&mut self, action: &str) -> bool {
        match action {
            "interrupt" => {
                if self.is_streaming {
                    self.is_streaming = false;
                    self.spinner_verb = None;
                    self.streaming_text.clear();
                    self.streaming_thinking.clear();
                    self.tool_use_blocks.clear();
                    self.status_message = Some("Cancelled.".to_string());
                } else {
                    self.should_quit = true;
                }
                false
            }
            "exit" => {
                if self.prompt_input.is_empty() {
                    self.should_quit = true;
                }
                false
            }
            "redraw" => false,
            "historySearch" => {
                let overlay = HistorySearchOverlay::open(&self.prompt_input.history);
                self.history_search_overlay = overlay;
                let mut hs = HistorySearch::new();
                hs.update_matches(&self.prompt_input.history);
                self.history_search = Some(hs);
                false
            }
            "openSearch" => {
                self.global_search.open();
                self.refresh_global_search();
                false
            }
            "submit" => !self.is_streaming,
            "historyPrev" => {
                // Slash-command suggestions take priority over history.
                if !self.prompt_input.suggestions.is_empty()
                    && self.prompt_input.text.starts_with('/')
                {
                    self.prompt_input.suggestion_prev();
                    self.refresh_prompt_input();
                } else if !self.prompt_input.history.is_empty() {
                    self.prompt_input.history_up();
                    self.refresh_prompt_input();
                }
                false
            }
            "historyNext" => {
                // Slash-command suggestions take priority over history.
                if !self.prompt_input.suggestions.is_empty()
                    && self.prompt_input.text.starts_with('/')
                {
                    self.prompt_input.suggestion_next();
                    self.refresh_prompt_input();
                } else if self.prompt_input.history_pos.is_some() {
                    self.prompt_input.history_down();
                    self.refresh_prompt_input();
                }
                false
            }
            "goLineStart" => {
                if !self.is_streaming {
                    self.prompt_input.cursor = 0;
                    self.sync_legacy_prompt_fields();
                }
                false
            }
            "goLineEnd" => {
                if !self.is_streaming {
                    self.prompt_input.cursor = self.prompt_input.text.len();
                    self.sync_legacy_prompt_fields();
                }
                false
            }
            "killToStart" => {
                if !self.is_streaming {
                    self.prompt_input.kill_line_backward();
                    self.refresh_prompt_input();
                }
                false
            }
            "killWord" => {
                if !self.is_streaming {
                    self.prompt_input.kill_word_backward();
                    self.refresh_prompt_input();
                }
                false
            }
            "scrollUp" => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
                self.auto_scroll = false;
                false
            }
            "scrollDown" => {
                let new_off = self.scroll_offset.saturating_sub(10);
                self.scroll_offset = new_off;
                if new_off == 0 {
                    self.auto_scroll = true;
                    self.new_messages_while_scrolled = 0;
                }
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
                    if let Some(entry) = hs.current_entry(&self.prompt_input.history) {
                        self.set_prompt_text(entry.to_string());
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
                    let count = hs.matches.len();
                    if count > 0 {
                        if hs.selected == 0 {
                            hs.selected = count - 1;
                        } else {
                            hs.selected -= 1;
                        }
                    }
                }
                self.history_search_overlay.select_prev();
                false
            }
            "nextResult" => {
                if let Some(hs) = self.history_search.as_mut() {
                    let count = hs.matches.len();
                    if count > 0 {
                        hs.selected = (hs.selected + 1) % count;
                    }
                }
                self.history_search_overlay.select_next();
                false
            }
            // ========== NEW KEYBINDING ACTIONS (Phase 1) ==========
            "clearLine" => {
                // Ctrl+L: Clear the current input line (like bash Ctrl+L)
                if !self.is_streaming {
                    self.prompt_input.text.clear();
                    self.prompt_input.cursor = 0;
                    self.refresh_prompt_input();
                }
                false
            }
            "deleteCharBefore" => {
                // Ctrl+H: Delete character before cursor (backspace equivalent)
                if !self.is_streaming {
                    self.prompt_input.backspace();
                    self.refresh_prompt_input();
                }
                false
            }
            "previousMessage" => {
                // Alt+←: Navigate to previous message in transcript
                self.scroll_offset = self.scroll_offset.saturating_add(5);
                self.auto_scroll = false;
                false
            }
            "nextMessage" => {
                // Alt+→: Navigate to next message in transcript
                let new_off = self.scroll_offset.saturating_sub(5);
                self.scroll_offset = new_off;
                if new_off == 0 {
                    self.auto_scroll = true;
                }
                false
            }
            "jumpToNextError" => {
                // Ctrl+.: Jump to next error/issue in messages
                self.jump_to_next_error();
                false
            }
            "jumpToPreviousError" => {
                // Ctrl+Shift+.: Jump to previous error/issue in messages
                self.jump_to_previous_error();
                false
            }
            "reverseIndent" => {
                // Shift+Tab: Reverse indent (cycle permission mode)
                use claurst_core::config::PermissionMode;
                self.config.permission_mode = match self.config.permission_mode {
                    PermissionMode::Default => PermissionMode::AcceptEdits,
                    PermissionMode::AcceptEdits => PermissionMode::BypassPermissions,
                    PermissionMode::BypassPermissions => PermissionMode::Default,
                    PermissionMode::Plan => PermissionMode::Default,
                };
                let label = match self.config.permission_mode {
                    PermissionMode::Default => "Default permissions",
                    PermissionMode::AcceptEdits => "Accept-edits mode",
                    PermissionMode::BypassPermissions => "Bypass permissions (dangerous)",
                    PermissionMode::Plan => "Plan mode",
                };
                self.status_message = Some(label.to_string());
                false
            }
            "openHelp" => {
                // Alt+H: Open help (alternative to F1)
                self.show_help = !self.show_help;
                self.help_overlay.toggle();
                false
            }
            "openModelPicker" => {
                if !self.is_streaming {
                    self.intercept_slash_command("model");
                }
                false
            }
            "openCommandPalette" => {
                if !self.is_streaming {
                    self.command_palette.open();
                }
                false
            }
            "deleteWord" => {
                // Alt+D: Delete word forward
                if !self.is_streaming {
                    self.prompt_input.delete_word_at_cursor();
                    self.refresh_prompt_input();
                }
                false
            }
            "sendMessage" => {
                // Ctrl+M: Send message (alternative to Enter)
                !self.is_streaming
            }
            "newline" => {
                // Shift+Enter: insert a literal newline into the prompt.
                if !self.is_streaming {
                    self.prompt_input.insert_newline();
                    self.refresh_prompt_input();
                }
                false
            }
            "indent" => {
                // Tab: cycle agent mode when prompt is empty, accept
                // slash-command suggestion otherwise.
                if !self.is_streaming {
                    if !self.prompt_input.suggestions.is_empty() {
                        if self.prompt_input.suggestion_index.is_none() {
                            self.prompt_input.suggestion_index = Some(0);
                        }
                        self.prompt_input.accept_suggestion();
                        self.refresh_prompt_input();
                    } else if self.prompt_input.is_empty() {
                        self.cycle_agent_mode();
                    self.rustle_look_down();
                    }
                }
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
                if let Some(entry) = hs.current_entry(&self.prompt_input.history) {
                    self.set_prompt_text(entry.to_string());
                }
                self.history_search = None;
                self.history_search_overlay.close();
            }
            KeyCode::Up => {
                let count = hs.matches.len();
                if count > 0 {
                    if hs.selected == 0 {
                        hs.selected = count - 1;
                    } else {
                        hs.selected -= 1;
                    }
                }
            }
            KeyCode::Down => {
                let count = hs.matches.len();
                if count > 0 {
                    hs.selected = (hs.selected + 1) % count;
                }
            }
            KeyCode::Backspace => {
                hs.query.pop();
                let history = self.prompt_input.history.clone();
                if let Some(hs) = self.history_search.as_mut() {
                    hs.update_matches(&history);
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                hs.query.push(c);
                let history = self.prompt_input.history.clone();
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
                    // Check if any option matches this key.
                    let mut matched_idx = None;
                    for (i, opt) in pr.options.iter().enumerate() {
                        if opt.key == c {
                            matched_idx = Some(i);
                            break;
                        }
                    }
                    if let Some(idx) = matched_idx {
                        pr.selected_option = idx;
                        // If this is the prefix-allow option ('P'), record the prefix.
                        self.maybe_record_bash_prefix();
                        self.permission_request = None;
                        return;
                    }
                }
            }
            KeyCode::Enter => {
                // If the currently selected option is the prefix-allow option, record it.
                self.maybe_record_bash_prefix();
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

    /// If the active permission dialog's selected option is the prefix-allow
    /// option ('P') for a Bash dialog, extract the suggested prefix and add it
    /// to `bash_prefix_allowlist` so future requests with the same prefix are
    /// silently approved.
    fn maybe_record_bash_prefix(&mut self) {
        use crate::dialogs::PermissionDialogKind;
        let pr = match self.permission_request.as_ref() {
            Some(p) => p,
            None => return,
        };
        // Only act on Bash dialogs where the selected option key is 'P'.
        let selected_key = pr.options.get(pr.selected_option).map(|o| o.key);
        if selected_key != Some('P') {
            return;
        }
        if let PermissionDialogKind::Bash { command, .. } = &pr.kind {
            // Always normalize to the first whitespace-delimited word so
            // that the allowlist check in `bash_command_allowed_by_prefix`
            // (which also uses `split_whitespace().next()`) matches correctly.
            let first_word = command.split_whitespace().next().unwrap_or("").to_string();
            if !first_word.is_empty() {
                self.bash_prefix_allowlist.insert(first_word);
            }
        }
    }

    /// Returns `true` if the given bash `command` is covered by the session-local
    /// prefix allowlist (i.e. its first word matches an entry in
    /// `bash_prefix_allowlist`).  Used by callers to skip the permission dialog.
    pub fn bash_command_allowed_by_prefix(&self, command: &str) -> bool {
        let first_word = command.split_whitespace().next().unwrap_or("");
        !first_word.is_empty() && self.bash_prefix_allowlist.contains(first_word)
    }

    // ---- Advanced mouse interaction helpers --------------------------------

    /// Detect if a click is a double-click based on timing and position.
    /// Returns true if the click is within ~500ms and ~5px of the last click.
    fn is_double_click(&self, current_pos: (u16, u16)) -> bool {
        let now = std::time::Instant::now();
        match (self.last_click_time, self.last_click_position) {
            (Some(last_time), Some(last_pos)) => {
                let elapsed = now.duration_since(last_time);
                let distance = ((current_pos.0 as i32 - last_pos.0 as i32).abs()
                    + (current_pos.1 as i32 - last_pos.1 as i32).abs()) as u16;
                elapsed.as_millis() < 500 && distance <= 5
            }
            _ => false,
        }
    }

    /// Find word boundaries for the character at (col, row) in the selection text.
    /// Returns (start_col, end_col) for the word containing the given position.
    fn find_word_boundaries(&self, col: u16, _row: u16) -> Option<(u16, u16)> {
        // Get the current selection text to determine word boundaries
        let text = self.selection_text.borrow();
        if text.is_empty() {
            return None;
        }

        // For simplicity, we'll find the word based on whitespace and punctuation
        // In a full implementation, we'd map visual positions back to text offsets
        // For now, return a reasonable range around the click position
        let start = col.saturating_sub(10).max(0);
        let end = col.saturating_add(10);
        Some((start, end))
    }

    /// Find line boundaries for the row containing the click.
    /// Returns (start_row, end_row) for the line.
    #[allow(dead_code)]
    fn find_line_boundaries(&self, row: u16) -> Option<(u16, u16)> {
        let selectable_area = self.last_selectable_area.get();
        let line_start = selectable_area.y;
        let line_end = selectable_area.y.saturating_add(selectable_area.height).saturating_sub(1);

        if row >= line_start && row <= line_end {
            Some((row, row))
        } else {
            None
        }
    }

    fn context_menu_items(kind: ContextMenuKind) -> &'static [ContextMenuItem] {
        match kind {
            ContextMenuKind::Message { .. } => &[ContextMenuItem::Copy, ContextMenuItem::Fork],
            ContextMenuKind::Selection => &[ContextMenuItem::Copy],
        }
    }

    fn message_index_at_row(&self, row: u16) -> Option<usize> {
        self.message_row_map.borrow().get(&row).copied()
    }

    fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection_focus = None;
        *self.selection_text.borrow_mut() = String::new();
    }

    /// Show context menu at the given position.
    fn show_context_menu(&mut self, x: u16, y: u16, kind: ContextMenuKind) {
        self.context_menu_state = Some(ContextMenuState {
            x,
            y,
            selected_index: 0,
            kind,
        });
    }

    /// Dismiss the context menu.
    fn dismiss_context_menu(&mut self) {
        self.context_menu_state = None;
    }

    /// Handle context menu navigation with arrow keys.
    fn navigate_context_menu(&mut self, direction: KeyCode) {
        if let Some(mut menu) = self.context_menu_state {
            let item_count = Self::context_menu_items(menu.kind).len();
            if item_count == 0 {
                self.context_menu_state = Some(menu);
                return;
            }
            match direction {
                KeyCode::Up => {
                    if menu.selected_index == 0 {
                        menu.selected_index = item_count - 1;
                    } else {
                        menu.selected_index -= 1;
                    }
                }
                KeyCode::Down => {
                    menu.selected_index = (menu.selected_index + 1) % item_count;
                }
                _ => return,
            }
            self.context_menu_state = Some(menu);
        }
    }

    /// Execute the currently selected context menu item.
    fn execute_context_menu_item(&mut self) {
        if let Some(menu) = self.context_menu_state {
            let items = Self::context_menu_items(menu.kind);

            if menu.selected_index < items.len() {
                let item = items[menu.selected_index];
                self.handle_context_menu_action(item, menu.kind);
            }
        }
        self.dismiss_context_menu();
    }

    /// Handle a context menu action.
    fn handle_context_menu_action(&mut self, item: ContextMenuItem, kind: ContextMenuKind) {
        match item {
            ContextMenuItem::Copy => {
                let text = match kind {
                    ContextMenuKind::Message { message_index } => self
                        .messages
                        .get(message_index)
                        .map(|message| message.get_all_text()),
                    ContextMenuKind::Selection => {
                        let selected = self.selection_text.borrow().trim().to_string();
                        if selected.is_empty() {
                            None
                        } else {
                            Some(selected)
                        }
                    }
                };

                if let Some(text) = text {
                    if crate::message_copy::copy_to_clipboard(&text) {
                        self.notifications.push(
                            NotificationKind::Info,
                            format!("Copied {} chars to clipboard.", text.len()),
                            Some(3),
                        );
                    } else {
                        self.notifications.push(
                            NotificationKind::Warning,
                            "Failed to copy to clipboard.".to_string(),
                            Some(3),
                        );
                    }
                    debug!("Copy action triggered, text: {} chars", text.len());
                }
            }
            ContextMenuItem::Fork => {
                if let ContextMenuKind::Message { message_index } = kind {
                    let branch_point = message_index + 1;
                    self.prompt_input.replace_text(format!("/fork {}", branch_point));
                    self.status_message =
                        Some(format!("Fork at message {} - press Enter to confirm", branch_point));
                }
            }
        }
    }

    fn prompt_can_accept_selection_paste(&self) -> bool {
        !self.is_streaming
            && self.permission_request.is_none()
            && !self.history_search_overlay.visible
            && self.history_search.is_none()
            && !matches!(
                self.prompt_input.vim_mode,
                crate::prompt_input::VimMode::Normal
                    | crate::prompt_input::VimMode::Visual
                    | crate::prompt_input::VimMode::VisualBlock
            )
    }

    fn paste_primary_into_prompt(&mut self) -> bool {
        if !self.prompt_can_accept_selection_paste() {
            return false;
        }

        if let Some(text) = crate::image_paste::read_primary_text()
            .or_else(crate::image_paste::read_clipboard_text)
        {
            self.focus = FocusTarget::Input;
            self.clear_selection();
            self.prompt_input.paste(&text);
            self.refresh_prompt_input();
            return true;
        }

        false
    }

    /// Process mouse events (trackpad scroll, text selection, etc.).
    pub fn handle_mouse_event(&mut self, mouse_event: MouseEvent) {
        use crossterm::event::MouseButton;

        // Fast-reject mouse-move events — they flood at 60+ Hz and we don't
        // need hover tracking. Exception: context menu needs hover to update
        // the selected item highlight.
        if matches!(mouse_event.kind, MouseEventKind::Moved) {
            if let Some(menu) = self.context_menu_state.as_mut() {
                let items = Self::context_menu_items(menu.kind);
                let item_labels: Vec<&str> = items.iter().map(|i| match i {
                    ContextMenuItem::Copy => "Copy",
                    ContextMenuItem::Fork => "Fork new chat",
                }).collect();
                let menu_width = (item_labels.iter().map(|l| l.len()).max().unwrap_or(4) + 4) as u16;
                let menu_height = items.len() as u16 + 2;
                let screen = self.last_msg_area.get();
                let menu_x = menu.x.min(screen.x.saturating_add(screen.width).saturating_sub(menu_width + 1));
                let menu_y = menu.y.min(screen.y.saturating_add(screen.height).saturating_sub(menu_height + 1));
                let inner_y = menu_y + 1;
                let col = mouse_event.column;
                let row = mouse_event.row;
                if col >= menu_x
                    && col < menu_x.saturating_add(menu_width)
                    && row >= inner_y
                    && row < inner_y.saturating_add(items.len() as u16)
                {
                    let hovered = (row - inner_y) as usize;
                    if hovered < items.len() {
                        menu.selected_index = hovered;
                    }
                }
            }
            return;
        }

        // ---- Dialog interaction: dismiss on click-outside, scroll/click inside ----
        // Key-input and device-auth stay outside this gate so their visible text
        // can still be selected and copied with the mouse.
        let any_dialog = self.connect_dialog.visible
            || self.import_config_picker.visible
            || self.import_config_dialog.visible
            || self.command_palette.visible
            || self.model_picker.visible
            || self.export_dialog.visible
            || self.settings_screen.visible
            || self.stats_dialog.open
            || self.context_viz.visible
            || self.session_browser.visible;

        if any_dialog {
            match mouse_event.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    // DialogSelect dialogs — check if click is inside for item selection
                    let in_dialog = if self.connect_dialog.visible {
                        self.connect_dialog.contains(mouse_event.column, mouse_event.row)
                    } else if self.import_config_picker.visible {
                        self.import_config_picker.contains(mouse_event.column, mouse_event.row)
                    } else if self.command_palette.visible {
                        self.command_palette.contains(mouse_event.column, mouse_event.row)
                    } else {
                        // Other dialogs (model_picker, settings, export, etc.) —
                        // treat any click as "inside" to prevent accidental dismiss.
                        // User must press Esc to close these.
                        true
                    };

                    if in_dialog {
                        // Click inside a DialogSelect — select the clicked item
                        if self.connect_dialog.visible {
                            self.connect_dialog.handle_mouse_click(mouse_event.row);
                        } else if self.import_config_picker.visible {
                            self.import_config_picker.handle_mouse_click(mouse_event.row);
                        } else if self.command_palette.visible {
                            self.command_palette.handle_mouse_click(mouse_event.row);
                        }
                        // Other dialogs: click absorbed, no action needed
                    } else {
                        // Click outside a DialogSelect — dismiss and restore input focus
                        self.close_secondary_views();
                        self.focus = FocusTarget::Input;
                    }
                }
                MouseEventKind::ScrollUp => {
                    // Scroll through dialog items
                    if self.connect_dialog.visible { self.connect_dialog.move_up(); }
                    else if self.import_config_picker.visible { self.import_config_picker.move_up(); }
                    else if self.command_palette.visible { self.command_palette.move_up(); }
                }
                MouseEventKind::ScrollDown => {
                    if self.connect_dialog.visible { self.connect_dialog.move_down(); }
                    else if self.import_config_picker.visible { self.import_config_picker.move_down(); }
                    else if self.command_palette.visible { self.command_palette.move_down(); }
                }
                _ => {}
            }
            return; // Don't process any other mouse events when a dialog is open
        }

        match mouse_event.kind {
            MouseEventKind::ScrollUp => {
                // Don't consume Ctrl+Scroll — let the terminal handle zoom.
                if !mouse_event.modifiers.contains(KeyModifiers::CONTROL) {
                    let step = self.scroll_step();
                    self.scroll_offset = self.scroll_offset.saturating_add(step);
                    self.auto_scroll = false;
                }
            }
            MouseEventKind::ScrollDown => {
                if !mouse_event.modifiers.contains(KeyModifiers::CONTROL) {
                    let step = self.scroll_step();
                    let new_off = self.scroll_offset.saturating_sub(step);
                    self.scroll_offset = new_off;
                    if new_off == 0 {
                        self.auto_scroll = true;
                        self.new_messages_while_scrolled = 0;
                    }
                }
            }
            // ---- Right-click context menu ----------------------------------
            MouseEventKind::Down(MouseButton::Right) => {
                let msg_area = self.last_msg_area.get();
                let has_selection = !self.selection_text.borrow().trim().is_empty();
                if mouse_event.column >= msg_area.x
                    && mouse_event.column < msg_area.x.saturating_add(msg_area.width)
                    && mouse_event.row >= msg_area.y
                    && mouse_event.row < msg_area.y.saturating_add(msg_area.height)
                {
                    if let Some(message_index) = self.message_index_at_row(mouse_event.row) {
                        self.show_context_menu(
                            mouse_event.column,
                            mouse_event.row,
                            ContextMenuKind::Message { message_index },
                        );
                    } else {
                        self.dismiss_context_menu();
                    }
                } else if has_selection {
                    self.show_context_menu(
                        mouse_event.column,
                        mouse_event.row,
                        ContextMenuKind::Selection,
                    );
                } else {
                    self.dismiss_context_menu();
                }
            }

            // ---- Primary-selection paste into the prompt ---------------
            MouseEventKind::Down(MouseButton::Middle) => {
                let _ = self.paste_primary_into_prompt();
            }

            // ---- Text selection / focus routing -------------------------
            MouseEventKind::Down(MouseButton::Left) => {
                // If a context menu is open, check if the click is on a menu item.
                // Must replicate the same position clamping as the renderer.
                if let Some(menu) = self.context_menu_state {
                    let items = Self::context_menu_items(menu.kind);
                    let item_labels: Vec<&str> = items.iter().map(|i| match i {
                        ContextMenuItem::Copy => "Copy",
                        ContextMenuItem::Fork => "Fork new chat",
                    }).collect();
                    let menu_width = (item_labels.iter().map(|l| l.len()).max().unwrap_or(4) + 4) as u16;
                    let menu_height = items.len() as u16 + 2; // +2 for border
                    // Clamp to screen bounds (same as render_context_menu)
                    let screen = self.last_msg_area.get();
                    let menu_x = menu.x.min(screen.x.saturating_add(screen.width).saturating_sub(menu_width + 1));
                    let menu_y = menu.y.min(screen.y.saturating_add(screen.height).saturating_sub(menu_height + 1));
                    let col = mouse_event.column;
                    let row = mouse_event.row;
                    // Inner area starts 1 past the border
                    let inner_y = menu_y + 1;
                    if col >= menu_x
                        && col < menu_x.saturating_add(menu_width)
                        && row >= inner_y
                        && row < inner_y.saturating_add(items.len() as u16)
                    {
                        let clicked_index = (row - inner_y) as usize;
                        if clicked_index < items.len() {
                            self.context_menu_state.as_mut().unwrap().selected_index = clicked_index;
                            self.execute_context_menu_item();
                            return;
                        }
                    }
                    // Click was outside the menu — just dismiss it
                    self.dismiss_context_menu();
                    return;
                }

                let input_area = self.last_input_area.get();
                let selectable_area = self.last_selectable_area.get();

                let in_input = input_area.width > 0 && input_area.height > 0
                    && mouse_event.row >= input_area.y
                    && mouse_event.row < input_area.y.saturating_add(input_area.height)
                    && mouse_event.column >= input_area.x
                    && mouse_event.column < input_area.x.saturating_add(input_area.width);

                let in_selectable = selectable_area.width > 0 && selectable_area.height > 0
                    && mouse_event.row >= selectable_area.y
                    && mouse_event.row < selectable_area.y.saturating_add(selectable_area.height)
                    && mouse_event.column >= selectable_area.x
                    && mouse_event.column < selectable_area.x.saturating_add(selectable_area.width);

                // Check for click on a thinking block header (takes priority over text selection).
                if let Some(&hash) = self.thinking_row_map.borrow().get(&mouse_event.row) {
                    if self.thinking_expanded.contains(&hash) {
                        self.thinking_expanded.remove(&hash);
                    } else {
                        self.thinking_expanded.insert(hash);
                    }
                    self.invalidate_transcript();
                    return;
                }

                if in_input {
                    self.focus = FocusTarget::Input;
                    self.clear_selection();
                } else if selectable_area.width == 0 || selectable_area.height == 0 {
                    self.click_count = 0;
                } else if in_selectable {
                    self.focus = FocusTarget::Transcript;

                    let current_pos = (mouse_event.column, mouse_event.row);
                    let now = std::time::Instant::now();

                    // Check for double-click
                    if self.is_double_click(current_pos) {
                        self.click_count += 1;
                        if self.click_count >= 3 {
                            // Triple-click: select entire line
                            self.selection_anchor = Some((selectable_area.x, current_pos.1));
                            self.selection_focus = Some((
                                selectable_area
                                    .x
                                    .saturating_add(selectable_area.width)
                                    .saturating_sub(1),
                                current_pos.1,
                            ));
                            self.click_count = 0; // Reset for next click sequence
                        } else {
                            // Double-click: select word
                            if let Some((start, end)) = self.find_word_boundaries(current_pos.0, current_pos.1) {
                                self.selection_anchor = Some((start, current_pos.1));
                                self.selection_focus = Some((end, current_pos.1));
                            }
                        }
                    } else {
                        // Single click or new click sequence
                        self.click_count = 1;
                        self.selection_anchor = Some(current_pos);
                        self.selection_focus = Some(current_pos);
                        *self.selection_text.borrow_mut() = String::new();
                    }

                    self.last_click_time = Some(now);
                    self.last_click_position = Some(current_pos);
                } else {
                    self.click_count = 0;
                    self.clear_selection();
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Dismiss context menu on drag
                self.dismiss_context_menu();

                // Continue drag — clamp to the selectable frame bounds so dragging
                // outside extends selection to the edge rather than cancelling.
                if self.selection_anchor.is_some() {
                    let selectable_area = self.last_selectable_area.get();
                    if selectable_area.width > 0 && selectable_area.height > 0 {
                        let clamped_col = mouse_event.column
                            .max(selectable_area.x)
                            .min(selectable_area.x.saturating_add(selectable_area.width).saturating_sub(1));
                        let clamped_row = mouse_event.row
                            .max(selectable_area.y)
                            .min(selectable_area.y.saturating_add(selectable_area.height).saturating_sub(1));
                        self.selection_focus = Some((clamped_col, clamped_row));
                        self.click_count = 0; // Reset on drag to prevent further double-clicks
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Clear if no actual drag (single click = no selection)
                if self.selection_anchor == self.selection_focus {
                    self.clear_selection();
                }
            }
            _ => {}
        }
    }

    // -------------------------------------------------------------------
    // Query event handling
    // -------------------------------------------------------------------

    /// Push a completed assistant message and trigger auto-scroll bookkeeping.
    fn push_assistant_message(&mut self, text: String) {
        let msg = Message::assistant(text);
        self.messages.push(msg);
        self.invalidate_transcript();
        self.on_new_message();
    }

    /// Process a query event from the agentic loop.
    pub fn handle_query_event(&mut self, event: QueryEvent) {
        match event {
            QueryEvent::Stream(stream_evt) => {
                if !self.is_streaming {
                    let seed = self.frame_count as usize ^ (self.messages.len() * 17);
                    self.spinner_verb = Some(sample_spinner_verb(seed).to_string());
                    // turn_start is set in begin_user_turn_snapshot (prompt
                    // submission time).  Only fall back here if somehow no
                    // user message was pushed before streaming began (e.g.
                    // headless / programmatic callers).
                    if self.turn_start.is_none() {
                        self.turn_start = Some(std::time::Instant::now());
                    }
                    self.streaming_thinking.clear();
                }
                self.is_streaming = true;
                match stream_evt {
                    claurst_api::AnthropicStreamEvent::ContentBlockDelta { delta, .. } => {
                        // Reset stall timer on any incoming delta — we're making progress.
                        self.stall_start = None;
                        match delta {
                            claurst_api::streaming::ContentDelta::TextDelta { text } => {
                                self.streaming_text.push_str(&text);
                                self.invalidate_transcript();
                            }
                            claurst_api::streaming::ContentDelta::ThinkingDelta { thinking } => {
                                debug!(len = thinking.len(), "Thinking delta received");
                                self.streaming_thinking.push_str(&thinking);
                                self.invalidate_transcript();
                            }
                            _ => {}
                        }
                    }
                    claurst_api::AnthropicStreamEvent::MessageStop => {
                        self.is_streaming = false;
                        self.spinner_verb = None;
                        self.stall_start = None;
                        self.flush_streamed_assistant_message();
                    }
                    _ => {
                        // Any other stream event: if we have no stall_start yet,
                        // record now so the red-spinner timer can begin.
                        if self.stall_start.is_none() {
                            self.stall_start = Some(std::time::Instant::now());
                        }
                    }
                }
            }

            QueryEvent::ToolStart { tool_name, tool_id, input_json } => {
                if !self.is_streaming && self.spinner_verb.is_none() {
                    let seed = self.frame_count as usize ^ (self.messages.len() * 17);
                    self.spinner_verb = Some(sample_spinner_verb(seed).to_string());
                }
                self.is_streaming = true;
                self.status_message = Some(format!("Running {}…", tool_name));
                let turn_index = self.current_user_turn_index();
                if let Some(existing) =
                    self.tool_use_blocks.iter_mut().find(|b| b.id == tool_id)
                {
                    existing.turn_index = turn_index;
                    existing.status = ToolStatus::Running;
                    existing.output_preview = None;
                    existing.input_json = input_json;
                } else {
                    self.tool_use_blocks.push(ToolUseBlock {
                        id: tool_id,
                        name: tool_name,
                        turn_index,
                        status: ToolStatus::Running,
                        output_preview: None,
                        input_json,
                    });
                }
                self.invalidate_transcript();
            }

            QueryEvent::ToolEnd {
                tool_name: _,
                tool_id,
                result,
                is_error,
            } => {
                // Build a multi-line preview: show up to 3 lines, truncate if more.
                let all_lines: Vec<&str> = result.lines().collect();
                let preview_lines = all_lines.len().min(3);
                let mut preview = all_lines[..preview_lines].join("\n");
                let remaining = all_lines.len().saturating_sub(preview_lines);
                if remaining > 0 {
                    preview.push_str(&format!("\n\u{2026} {} more lines", remaining));
                }
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
                self.invalidate_transcript();
                if is_error {
                    self.status_message = Some(format!("Tool error: {}", result));
                } else {
                    self.status_message = None;
                }
                self.refresh_turn_diff_from_history();
            }

            QueryEvent::TurnComplete { turn, stop_reason, usage, .. } => {
                debug!(turn, stop_reason, "Turn complete");
                self.is_streaming = false;
                self.spinner_verb = None;

                // Update context window usage from the usage info.
                if let Some(ref u) = usage {
                    let turn_tokens = u.input_tokens + u.output_tokens
                        + u.cache_creation_input_tokens + u.cache_read_input_tokens;
                    self.context_used_tokens = self.context_used_tokens.saturating_add(turn_tokens);
                }
                // Record elapsed time and pick a completion verb
                let seed = self.frame_count as usize ^ (self.messages.len() * 7);
                let elapsed = self.turn_start.take()
                    .map(|start| format_elapsed_ms(start.elapsed().as_millis()));
                self.last_turn_elapsed = Some(
                    elapsed.unwrap_or_else(|| "0s".to_string())
                );
                self.last_turn_verb = Some(sample_completion_verb(seed));
                self.flush_streamed_assistant_message();
                self.tool_use_blocks.retain(|b| b.status != ToolStatus::Running);
                self.complete_current_turn_snapshot(stop_reason.contains("abort") || stop_reason.contains("cancel"));
                self.invalidate_transcript();
                self.refresh_turn_diff_from_history();
            }

            QueryEvent::Status(msg) => {
                self.status_message = Some(msg);
            }

            QueryEvent::Error(msg) => {
                self.is_streaming = false;
                self.spinner_verb = None;
                self.streaming_text.clear();
                self.streaming_thinking.clear();
                self.invalidate_transcript();
                let err_msg = format!("Error: {}", msg);
                self.push_assistant_message(err_msg.clone());
                self.status_message = Some(err_msg);
            }
            QueryEvent::TokenWarning { state, pct_used } => {
                // Push a notification for context window warnings (notification + threshold tracking).
                use claurst_query::compact::TokenWarningState;

                // Only escalate — never repeat a threshold already shown.
                match state {
                    TokenWarningState::Ok => {
                        // Reset threshold tracking when back to normal
                        self.token_warning_threshold_shown = 0;
                    }
                    TokenWarningState::Warning if self.token_warning_threshold_shown < 80 => {
                        self.token_warning_threshold_shown = 80;
                        self.notifications.push(
                            NotificationKind::Warning,
                            format!("Context window {:.0}% full. Consider /compact.", pct_used * 100.0),
                            Some(30),
                        );
                    }
                    TokenWarningState::Critical if self.token_warning_threshold_shown < 95 => {
                        self.token_warning_threshold_shown = 95;
                        self.notifications.push(
                            NotificationKind::Error,
                            format!("Context window {:.0}% full! Run /compact now.", pct_used * 100.0),
                            None,
                        );
                    }
                    _ => {}
                }
            }
        }

        // Update token count from tracker.
        self.token_count = self.cost_tracker.total_tokens() as u32;
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

            // Drain background session-list results.
            if let Some(ref mut rx) = self.session_list_rx {
                match rx.try_recv() {
                    Ok(entries) => {
                        self.session_browser.sessions = entries;
                        self.session_browser.selected_idx = 0;
                        self.session_list_rx = None;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        self.session_list_rx = None;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                }
            }

            // Spawn async session-list load when requested.
            if self.session_list_pending {
                self.session_list_pending = false;
                let (tx, rx) = tokio::sync::mpsc::channel(1);
                self.session_list_rx = Some(rx);
                tokio::spawn(async move {
                    let sessions = claurst_core::history::list_sessions().await;
                    let entries: Vec<crate::session_browser::SessionEntry> = sessions
                        .into_iter()
                        .map(|s| {
                            let age = chrono::Utc::now()
                                .signed_duration_since(s.updated_at);
                            let last_updated = if age.num_minutes() < 1 {
                                "just now".to_string()
                            } else if age.num_hours() < 1 {
                                format!("{}m ago", age.num_minutes())
                            } else if age.num_hours() < 24 {
                                format!("{}h ago", age.num_hours())
                            } else {
                                format!("{}d ago", age.num_days())
                            };
                            crate::session_browser::SessionEntry {
                                id: s.id,
                                title: s.title.unwrap_or_else(|| "(untitled)".to_string()),
                                last_updated,
                                message_count: s.messages.len(),
                                cost_usd: s.total_cost,
                            }
                        })
                        .collect();
                    let _ = tx.send(entries).await;
                });
            }

            // Drain voice transcription events (non-blocking).
            // When the background recording/transcription task emits a
            // TranscriptReady event we insert the text directly into the
            // prompt so the user can review and submit it.
            {
                use claurst_core::voice::VoiceEvent;
                let mut events = Vec::new();
                if let Some(ref mut rx) = self.voice_event_rx {
                    while let Ok(ev) = rx.try_recv() {
                        events.push(ev);
                    }
                }
                for ev in events {
                    match ev {
                        VoiceEvent::RecordingStarted => {
                            self.voice_recording = true;
                            self.status_message =
                                Some("Recording\u{2026} (Alt+V or Esc to stop)".to_string());
                        }
                        VoiceEvent::RecordingStopped => {
                            self.voice_recording = false;
                            self.status_message =
                                Some("Transcribing\u{2026}".to_string());
                        }
                        VoiceEvent::TranscriptReady(text) => {
                            if !text.is_empty() {
                                // Append to existing prompt text with a space separator
                                // so the user can combine voice + typed input.
                                if !self.prompt_input.text.is_empty()
                                    && !self.prompt_input.text.ends_with(' ')
                                {
                                    self.prompt_input.paste(" ");
                                }
                                self.prompt_input.paste(&text);
                                self.refresh_prompt_input();
                                self.status_message = Some(
                                    format!("Transcribed: {}", &text[..text.len().min(60)])
                                );
                            }
                            // Clear the channel once we have the result.
                            self.voice_event_rx = None;
                        }
                        VoiceEvent::Error(msg) => {
                            self.voice_recording = false;
                            self.voice_event_rx = None;
                            self.notifications.push(
                                NotificationKind::Warning,
                                format!("Voice: {}", msg),
                                Some(8),
                            );
                        }
                    }
                }
            }

            // Draw the frame
            terminal.draw(|f| render::render_app(f, self))?;

            // Poll for events with a short timeout so we can redraw for animation
            if event::poll(std::time::Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) => {
                        // On Windows crossterm fires both Press and Release events.
                        // We normally skip non-press events, but when voice PTT mode
                        // is active we need the Release event for the `V` key so we
                        // can stop recording as soon as the user lifts the key.
                        if key.kind != crossterm::event::KeyEventKind::Press {
                            // Handle V-key release to stop PTT recording.
                            if key.kind == crossterm::event::KeyEventKind::Release
                                && key.code == KeyCode::Char('v')
                                && key.modifiers == KeyModifiers::NONE
                                && self.voice_recording
                                && self.voice_recorder.is_some()
                            {
                                self.handle_voice_ptt_stop();
                            }
                            continue;
                        }
                        let should_submit = self.handle_key_event(key);
                        // Honour `:q`/`:wq` from vim command-line mode
                        if self.prompt_input.vim_quit_requested {
                            self.prompt_input.vim_quit_requested = false;
                            self.should_quit = true;
                        }
                        if self.should_quit {
                            return Ok(None);
                        }
                        if should_submit {
                            // Check if this is a slash command that should open a UI screen
                            if crate::input::is_slash_command(&self.prompt_input.text) {
                                let slash_input = self.prompt_input.text.clone();
                                let (cmd, args) =
                                        crate::input::parse_slash_command(&slash_input);
                                if self.intercept_slash_command_with_args(cmd, args) {
                                    self.clear_prompt();
                                    continue;
                                }
                            }
                            let input = self.take_input();
                            if !input.is_empty() {
                                return Ok(Some(input));
                            }
                        }
                    }
                    Event::Paste(data)
                        if !self.is_streaming
                            && self.permission_request.is_none()
                            && !self.history_search_overlay.visible
                            && self.history_search.is_none() =>
                    {
                        self.prompt_input.paste(&data);
                        self.refresh_prompt_input();
                    }
                    Event::Mouse(mouse_event) => {
                        self.handle_mouse_event(mouse_event);
                    }
                    _ => {}
                }
            }
        }
    }

    // ========== NEW KEYBINDING HELPER FUNCTIONS (Phase 1) ==========

    /// Jump to the next error/issue in messages.
    /// Searches for common error indicators: "Error:", "ERROR:", "error", "failed", "FAIL".
    fn jump_to_next_error(&mut self) {
        const ERROR_KEYWORDS: &[&str] = &["error:", "failed:", "fail"];

        // Search forward from current position
        for i in 0..self.messages.len() {
            let msg = &self.messages[i];
            let content = msg.get_all_text().to_lowercase();

            // Check if message contains error keywords
            let has_error = ERROR_KEYWORDS.iter().any(|keyword| {
                content.contains(keyword)
            });

            if has_error && i > (self.messages.len().saturating_sub(self.scroll_offset / 2)) {
                // Found an error message, scroll to it
                let new_offset = self.messages.len().saturating_sub(i);
                self.scroll_offset = new_offset.saturating_mul(2);
                self.auto_scroll = false;
                self.status_message = Some(format!("Error found in message {}", i + 1));
                return;
            }
        }

        self.status_message = Some("No more errors found.".to_string());
    }

    /// Jump to the previous error/issue in messages.
    /// Searches backwards for common error indicators.
    fn jump_to_previous_error(&mut self) {
        const ERROR_KEYWORDS: &[&str] = &["error:", "failed:", "fail"];

        // Search backward from current position
        for i in (0..self.messages.len()).rev() {
            let msg = &self.messages[i];
            let content = msg.get_all_text().to_lowercase();

            // Check if message contains error keywords
            let has_error = ERROR_KEYWORDS.iter().any(|keyword| {
                content.contains(keyword)
            });

            if has_error && i < (self.messages.len().saturating_sub(self.scroll_offset / 2)) {
                // Found an error message, scroll to it
                let new_offset = self.messages.len().saturating_sub(i);
                self.scroll_offset = new_offset.saturating_mul(2);
                self.auto_scroll = false;
                self.status_message = Some(format!("Error found in message {}", i + 1));
                return;
            }
        }

        self.status_message = Some("No previous errors found.".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn make_app() -> App {
        let config = Config::default();
        let cost_tracker = claurst_core::cost::CostTracker::new();
        App::new(config, cost_tracker)
    }

    fn press_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn test_mcp_subcommand_is_not_intercepted() {
        let mut app = make_app();
        assert!(!app.intercept_slash_command_with_args("mcp", "auth mcphub"));
        assert!(!app.mcp_view.open);
    }

    #[test]
    fn test_clear_slash_command_clears_messages() {
        let mut app = make_app();
        app.add_message(Role::User, "hello".to_string());
        app.add_message(Role::Assistant, "world".to_string());
        assert_eq!(app.messages.len(), 2);
        assert!(app.intercept_slash_command("clear"));
        assert_eq!(app.messages.len(), 0);
    }

    #[test]
    fn test_exit_slash_command_sets_quit_flag() {
        let mut app = make_app();
        assert!(!app.should_quit);
        assert!(app.intercept_slash_command("exit"));
        assert!(app.should_quit);
    }

    #[test]
    fn test_vim_slash_command_toggles_vim() {
        let mut app = make_app();
        assert!(!app.prompt_input.vim_enabled);
        assert!(app.intercept_slash_command("vim"));
        assert!(app.prompt_input.vim_enabled);
        assert!(app.intercept_slash_command("vim"));
        assert!(!app.prompt_input.vim_enabled);
    }

    #[test]
    fn test_model_slash_command_opens_picker() {
        let mut app = make_app();
        assert!(!app.model_picker.visible);
        assert!(app.intercept_slash_command("model"));
        assert!(app.model_picker.visible);
    }

    #[test]
    fn test_fast_slash_command_toggles_fast_mode() {
        let mut app = make_app();
        assert!(!app.fast_mode);
        assert!(app.intercept_slash_command("fast"));
        assert!(app.fast_mode);
        assert!(app.intercept_slash_command("fast"));
        assert!(!app.fast_mode);
    }

    #[test]
    fn test_output_style_cycles() {
        let mut app = make_app();
        assert_eq!(app.output_style, "auto");
        assert!(app.intercept_slash_command("output-style"));
        assert_eq!(app.output_style, "stream");
        assert!(app.intercept_slash_command("output-style"));
        assert_eq!(app.output_style, "verbose");
        assert!(app.intercept_slash_command("output-style"));
        assert_eq!(app.output_style, "auto");
    }

    #[test]
    fn test_context_menu_fork_targets_clicked_message() {
        let mut app = make_app();
        app.add_message(Role::User, "one".to_string());
        app.add_message(Role::Assistant, "two".to_string());
        app.add_message(Role::User, "three".to_string());

        app.handle_context_menu_action(
            ContextMenuItem::Fork,
            ContextMenuKind::Message { message_index: 1 },
        );

        assert_eq!(app.prompt_input.text, "/fork 2");
        assert_eq!(
            app.status_message.as_deref(),
            Some("Fork at message 2 - press Enter to confirm")
        );
    }

    #[test]
    fn test_right_click_targets_row_message_instead_of_last_message() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        let mut app = make_app();
        app.last_msg_area.set(ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 10,
        });
        app.message_row_map.borrow_mut().insert(3, 1);

        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 12,
            row: 3,
            modifiers: KeyModifiers::empty(),
        });

        assert!(matches!(
            app.context_menu_state,
            Some(ContextMenuState {
                kind: ContextMenuKind::Message { message_index: 1 },
                ..
            })
        ));
    }

    // ---- Help overlay -------------------------------------------------------

    #[test]
    fn test_help_slash_command_opens_overlay() {
        let mut app = make_app();
        assert!(!app.help_overlay.visible);
        assert!(!app.show_help);
        assert!(!app.help_overlay.commands.is_empty());
        assert!(app.intercept_slash_command("help"));
        assert!(app.help_overlay.visible);
        assert!(app.show_help);
    }

    #[test]
    fn test_help_slash_command_is_idempotent_when_already_open() {
        let mut app = make_app();
        // First call opens it.
        assert!(app.intercept_slash_command("help"));
        assert!(app.help_overlay.visible);
        // Second call while already open should leave it open (not toggle it off).
        assert!(app.intercept_slash_command("help"));
        assert!(app.help_overlay.visible);
    }

    #[test]
    fn test_question_mark_shortcut_opens_help_with_shift_modifier() {
        let mut app = make_app();

        app.handle_key_event(press_key(KeyCode::Char('?'), KeyModifiers::SHIFT));

        assert!(app.help_overlay.visible);
        assert!(app.show_help);
    }

    #[test]
    fn test_question_mark_shortcut_closes_help_with_shift_modifier() {
        let mut app = make_app();
        app.help_overlay.toggle();
        app.show_help = true;

        app.handle_key_event(press_key(KeyCode::Char('?'), KeyModifiers::SHIFT));

        assert!(!app.help_overlay.visible);
        assert!(!app.show_help);
    }

    #[test]
    fn test_question_mark_shortcut_types_into_non_empty_prompt() {
        let mut app = make_app();
        app.prompt_input.text = "why".to_string();
        app.prompt_input.cursor = app.prompt_input.text.len();
        app.refresh_prompt_input();

        app.handle_key_event(press_key(KeyCode::Char('?'), KeyModifiers::SHIFT));

        assert!(!app.help_overlay.visible);
        assert_eq!(app.prompt_input.text, "why?");
    }

    #[test]
    fn test_ctrl_a_shortcut_opens_model_picker() {
        let mut app = make_app();
        app.has_credentials = true;
        app.config.provider = Some("anthropic".to_string());

        app.handle_key_event(press_key(KeyCode::Char('a'), KeyModifiers::CONTROL));

        assert!(app.model_picker.visible);
    }

    #[test]
    fn test_ctrl_k_shortcut_opens_command_palette_even_with_input() {
        let mut app = make_app();
        app.prompt_input.text = "hello".to_string();
        app.prompt_input.cursor = app.prompt_input.text.len();
        app.refresh_prompt_input();

        app.handle_key_event(press_key(KeyCode::Char('k'), KeyModifiers::CONTROL));

        assert!(app.command_palette.visible);
        assert_eq!(app.prompt_input.text, "hello");
    }

    // ---- Bash prefix allowlist ----------------------------------------------

    #[test]
    fn test_bash_command_not_allowed_by_default() {
        let app = make_app();
        assert!(!app.bash_command_allowed_by_prefix("git status"));
        assert!(!app.bash_command_allowed_by_prefix("ls -la"));
        assert!(!app.bash_command_allowed_by_prefix(""));
    }

    #[test]
    fn test_bash_prefix_allowlist_after_p_key() {
        use crate::dialogs::PermissionRequest;
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

        let mut app = make_app();
        // Set up a bash permission dialog with a suggested prefix.
        let pr = PermissionRequest::bash(
            "tu-1".to_string(),
            "Bash".to_string(),
            "This will execute a shell command.".to_string(),
            "git status".to_string(),
            Some("git".to_string()),
        );
        app.permission_request = Some(pr);

        // Simulate pressing 'P' (prefix-allow key).
        let key = KeyEvent {
            code: KeyCode::Char('P'),
            modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        app.handle_permission_key(key);

        // Dialog should be dismissed and "git" added to the allowlist.
        assert!(app.permission_request.is_none());
        assert!(app.bash_command_allowed_by_prefix("git status"));
        assert!(app.bash_command_allowed_by_prefix("git push origin main"));
        // Other commands should NOT be allowed.
        assert!(!app.bash_command_allowed_by_prefix("rm -rf /tmp"));
    }

    #[test]
    fn test_bash_prefix_allowlist_via_enter_on_p_option() {
        use crate::dialogs::PermissionRequest;
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

        let mut app = make_app();
        let mut pr = PermissionRequest::bash(
            "tu-2".to_string(),
            "Bash".to_string(),
            "This will execute a shell command.".to_string(),
            "cargo build".to_string(),
            Some("cargo".to_string()),
        );
        // Navigate to the prefix option (index 3 in a 5-option dialog).
        pr.selected_option = 3;
        app.permission_request = Some(pr);

        // Press Enter to confirm the currently selected (prefix) option.
        let key = KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        app.handle_permission_key(key);

        assert!(app.permission_request.is_none());
        assert!(app.bash_command_allowed_by_prefix("cargo test"));
        assert!(!app.bash_command_allowed_by_prefix("make build"));
    }

    #[test]
    fn test_bash_prefix_allowlist_non_prefix_option_does_not_add() {
        use crate::dialogs::PermissionRequest;
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

        let mut app = make_app();
        let pr = PermissionRequest::bash(
            "tu-3".to_string(),
            "Bash".to_string(),
            "This will execute a shell command.".to_string(),
            "npm install".to_string(),
            Some("npm".to_string()),
        );
        app.permission_request = Some(pr);

        // Press 'y' (allow-once) — should NOT add to allowlist.
        let key = KeyEvent {
            code: KeyCode::Char('y'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        app.handle_permission_key(key);

        assert!(app.permission_request.is_none());
        assert!(!app.bash_command_allowed_by_prefix("npm test"));
    }
}
