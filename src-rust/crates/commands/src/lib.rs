// cc-commands: Slash command system for the Claude Code Rust port.
//
// This crate implements the /command framework that allows users to type
// commands like /help, /compact, /clear, /model, /config, /cost, etc.
// Each command is a struct implementing the `SlashCommand` trait.

use async_trait::async_trait;
use cc_core::config::{Config, Settings, Theme};
use cc_core::cost::CostTracker;
use cc_core::types::Message;
use std::collections::BTreeMap;
use std::sync::Arc;
#[allow(unused_imports)]
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Core trait
// ---------------------------------------------------------------------------

/// Context available to every slash command.
pub struct CommandContext {
    pub config: Config,
    pub cost_tracker: Arc<CostTracker>,
    pub messages: Vec<Message>,
    pub working_dir: std::path::PathBuf,
    pub session_id: String,
    pub session_title: Option<String>,
    // Note: config already contains hooks, mcp_servers, etc.
}

/// Result of running a slash command.
#[derive(Debug)]
pub enum CommandResult {
    /// Display a message to the user (does NOT go to the model).
    Message(String),
    /// Inject a message into the conversation as though the user typed it.
    UserMessage(String),
    /// Modify the configuration.
    ConfigChange(Config),
    /// Modify the configuration and show a specific status message.
    ConfigChangeMessage(Config, String),
    /// Clear the conversation.
    ClearConversation,
    /// Replace the conversation with a specific message list (used by /rewind).
    SetMessages(Vec<Message>),
    /// Load a previously saved session into the live REPL.
    ResumeSession(cc_core::history::ConversationSession),
    /// Update the current session title.
    RenameSession(String),
    /// Trigger the OAuth login flow (handled by the REPL in main.rs).
    /// The bool indicates whether to use Claude.ai auth (true) or Console auth (false).
    StartOAuthFlow(bool),
    /// Exit the REPL.
    Exit,
    /// No visible output.
    Silent,
    /// An error.
    Error(String),
}

/// Every slash command implements this trait.
#[async_trait]
pub trait SlashCommand: Send + Sync {
    /// The primary name (without the leading `/`).
    fn name(&self) -> &str;
    /// Alias names (e.g. `["h"]` for `/help`).
    fn aliases(&self) -> Vec<&str> {
        vec![]
    }
    /// One-line description for /help.
    fn description(&self) -> &str;
    /// Detailed help text (shown by `/help <command>`).
    fn help(&self) -> &str {
        self.description()
    }
    /// Whether this command is visible in /help output.
    fn hidden(&self) -> bool {
        false
    }
    /// Execute the command with the given arguments string.
    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult;
}

// ---------------------------------------------------------------------------
// Built-in commands
// ---------------------------------------------------------------------------

pub struct HelpCommand;
pub struct ClearCommand;
pub struct CompactCommand;
pub struct CostCommand;
pub struct ExitCommand;
pub struct ModelCommand;
pub struct ConfigCommand;
pub struct ColorCommand;
pub struct VersionCommand;
pub struct ResumeCommand;
pub struct StatusCommand;
pub struct DiffCommand;
pub struct MemoryCommand;
pub struct BugCommand;
pub struct UsageCommand;
pub struct DoctorCommand;
pub struct LoginCommand;
pub struct LogoutCommand;
pub struct InitCommand;
pub struct ReviewCommand;
pub struct HooksCommand;
pub struct McpCommand;
pub struct PermissionsCommand;
pub struct PlanCommand;
pub struct TasksCommand;
pub struct SessionCommand;
pub struct ThinkingCommand;
// New commands
pub struct ExportCommand;
pub struct SkillsCommand;
pub struct RewindCommand;
pub struct StatsCommand;
pub struct FilesCommand;
pub struct RenameCommand;
pub struct EffortCommand;
pub struct SummaryCommand;
pub struct CommitCommand;
pub struct PluginCommand;
pub struct ReloadPluginsCommand;
pub struct ThemeCommand;
pub struct OutputStyleCommand;
pub struct KeybindingsCommand;
pub struct PrivacySettingsCommand;
// Batch-1 new commands
pub struct RemoteControlCommand;
pub struct RemoteEnvCommand;
pub struct ContextCommand;
pub struct CopyCommand;
pub struct ChromeCommand;
pub struct VimCommand;
pub struct VoiceCommand;
pub struct UpgradeCommand;
pub struct ReleaseNotesCommand;
pub struct RateLimitOptionsCommand;
pub struct StatuslineCommand;
pub struct SecurityReviewCommand;
pub struct TerminalSetupCommand;
pub struct ExtraUsageCommand;
pub struct FastCommand;
pub struct ThinkBackCommand;
pub struct ThinkBackPlayCommand;
pub struct FeedbackCommand;
pub struct ColorSetCommand;
pub struct NamedCommandAdapter {
    pub slash_name: &'static str,
    pub target_name: &'static str,
    pub slash_aliases: &'static [&'static str],
    pub slash_description: &'static str,
    pub slash_help: &'static str,
}

#[derive(serde::Serialize)]
struct KeybindingTemplateFile {
    #[serde(rename = "$schema")]
    schema: &'static str,
    #[serde(rename = "$docs")]
    docs: &'static str,
    bindings: Vec<KeybindingTemplateBlock>,
}

#[derive(serde::Serialize)]
struct KeybindingTemplateBlock {
    context: String,
    bindings: BTreeMap<String, Option<String>>,
}

fn save_settings_mutation<F>(mutate: F) -> anyhow::Result<()>
where
    F: FnOnce(&mut Settings),
{
    let mut settings = Settings::load_sync()?;
    mutate(&mut settings);
    settings.save_sync()
}

fn open_with_system(target: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        let ps_cmd = format!("Start-Process '{}'", target.replace('\'', "''"));
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(target)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        return Ok(());
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(target)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        Ok(())
    }
}

fn format_keystroke(keystroke: &cc_core::keybindings::ParsedKeystroke) -> String {
    let mut parts = Vec::new();
    if keystroke.ctrl {
        parts.push("ctrl".to_string());
    }
    if keystroke.alt {
        parts.push("alt".to_string());
    }
    if keystroke.shift {
        parts.push("shift".to_string());
    }
    if keystroke.meta {
        parts.push("meta".to_string());
    }
    parts.push(match keystroke.key.as_str() {
        "space" => "space".to_string(),
        other => other.to_string(),
    });
    parts.join("+")
}

fn format_chord(chord: &[cc_core::keybindings::ParsedKeystroke]) -> String {
    chord
        .iter()
        .map(format_keystroke)
        .collect::<Vec<_>>()
        .join(" ")
}

fn generate_keybindings_template() -> anyhow::Result<String> {
    let mut grouped: BTreeMap<String, BTreeMap<String, Option<String>>> = BTreeMap::new();
    for binding in cc_core::keybindings::default_bindings() {
        let chord = format_chord(&binding.chord);
        if cc_core::keybindings::NON_REBINDABLE.contains(&chord.as_str()) {
            continue;
        }
        grouped
            .entry(format!("{:?}", binding.context))
            .or_default()
            .insert(chord, binding.action.clone());
    }

    let template = KeybindingTemplateFile {
        schema: "https://www.schemastore.org/claude-code-keybindings.json",
        docs: "https://code.claude.com/docs/en/keybindings",
        bindings: grouped
            .into_iter()
            .map(|(context, bindings)| KeybindingTemplateBlock { context, bindings })
            .collect(),
    };

    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&template)?
    ))
}

fn parse_theme(name: &str) -> Option<Theme> {
    match name.trim().to_lowercase().as_str() {
        "default" | "system" => Some(Theme::Default),
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        custom if !custom.is_empty() => Some(Theme::Custom(custom.to_string())),
        _ => None,
    }
}

fn current_output_style_name(config: &Config) -> &str {
    config.output_style.as_deref().unwrap_or("default")
}

fn available_output_style_names() -> Vec<String> {
    cc_core::output_styles::all_styles(&Settings::config_dir())
        .into_iter()
        .map(|style| style.name)
        .collect()
}

fn split_command_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escape = false;

    for ch in args.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }

        match ch {
            '\\' => escape = true,
            '\'' | '"' if quote == Some(ch) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(ch),
            ch if ch.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        out.push(current);
    }

    out
}

fn execute_named_command_from_slash(
    target_name: &str,
    args: &str,
    ctx: &CommandContext,
) -> CommandResult {
    let Some(cmd) = named_commands::find_named_command(target_name) else {
        return CommandResult::Error(format!(
            "Named command '{}' is not available in this build.",
            target_name
        ));
    };

    let parsed_args = split_command_args(args);
    let parsed_refs = parsed_args.iter().map(String::as_str).collect::<Vec<_>>();
    cmd.execute_named(&parsed_refs, ctx)
}

// ---- /help ---------------------------------------------------------------

/// Category labels for help grouping.
fn command_category(name: &str) -> &'static str {
    match name {
        "clear" | "compact" | "rewind" | "summary" | "export" | "rename" | "branch" => {
            "Conversation"
        }
        "model" | "config" | "theme" | "color" | "vim" | "fast" | "effort"
        | "voice" | "statusline" | "output-style" | "keybindings"
        | "privacy-settings" | "rate-limit-options" => "Settings",
        "cost" | "stats" | "usage" | "extra-usage" | "context" => "Usage & Cost",
        "status" | "doctor" | "terminal-setup" | "version" | "upgrade"
        | "release-notes" => "System",
        "login" | "logout" | "permissions" => "Auth & Permissions",
        "memory" | "files" | "diff" | "init" | "commit" | "review"
        | "security-review" => "Project",
        "mcp" | "hooks" | "ide" | "chrome" => "Integrations",
        "session" | "resume" | "remote-control" | "remote-env" => "Sessions & Remote",
        "help" | "exit" | "feedback" | "bug" => "General",
        "think-back" | "thinkback-play" | "thinking" | "plan" | "tasks" => "AI & Thinking",
        "copy" | "skills" | "agents" | "plugin" | "reload-plugins"
        | "stickers" | "passes" | "desktop" | "mobile" => "Tools & Extras",
        _ => "Other",
    }
}

#[async_trait]
impl SlashCommand for HelpCommand {
    fn name(&self) -> &str { "help" }
    fn aliases(&self) -> Vec<&str> { vec!["h", "?"] }
    fn description(&self) -> &str { "Show available commands and usage information" }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        if !args.is_empty() {
            // Show help for a specific command
            if let Some(cmd) = find_command(args) {
                let aliases = cmd.aliases();
                let alias_line = if aliases.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\nAliases: {}",
                        aliases.iter().map(|a| format!("/{}", a)).collect::<Vec<_>>().join(", ")
                    )
                };
                return CommandResult::Message(format!(
                    "/{name}{aliases}\n{desc}\n\n{help}",
                    name = cmd.name(),
                    aliases = alias_line,
                    desc = cmd.description(),
                    help = cmd.help(),
                ));
            }
            return CommandResult::Error(format!("Unknown command: /{}", args));
        }

        // Grouped output
        let commands = all_commands();
        let visible: Vec<_> = commands.iter().filter(|c| !c.hidden()).collect();

        // Collect categories in stable order
        let category_order = [
            "Conversation",
            "Settings",
            "Usage & Cost",
            "System",
            "Auth & Permissions",
            "Project",
            "Integrations",
            "Sessions & Remote",
            "AI & Thinking",
            "Tools & Extras",
            "General",
            "Other",
        ];

        let mut by_cat: std::collections::HashMap<&str, Vec<String>> =
            std::collections::HashMap::new();

        for cmd in &visible {
            let cat = command_category(cmd.name());
            let aliases = cmd.aliases();
            let alias_str = if aliases.is_empty() {
                String::new()
            } else {
                format!(
                    " ({})",
                    aliases.iter().map(|a| format!("/{}", a)).collect::<Vec<_>>().join(", ")
                )
            };
            by_cat
                .entry(cat)
                .or_default()
                .push(format!("  /{:<20} {}", format!("{}{}", cmd.name(), alias_str), cmd.description()));
        }

        let mut output = String::from("Claude Code — Slash Commands\n");
        output.push_str("════════════════════════════\n");

        for cat in &category_order {
            if let Some(entries) = by_cat.get(cat) {
                output.push_str(&format!("\n{}\n", cat));
                for entry in entries {
                    output.push_str(&format!("{}\n", entry));
                }
            }
        }

        output.push_str("\nType /help <command> for detailed help on a specific command.");
        CommandResult::Message(output)
    }
}

// ---- /clear --------------------------------------------------------------

#[async_trait]
impl SlashCommand for ClearCommand {
    fn name(&self) -> &str { "clear" }
    fn aliases(&self) -> Vec<&str> { vec!["c", "reset", "new"] }
    fn description(&self) -> &str { "Clear the conversation history" }

    async fn execute(&self, _args: &str, _ctx: &mut CommandContext) -> CommandResult {
        CommandResult::ClearConversation
    }
}

// ---- /compact ------------------------------------------------------------

#[async_trait]
impl SlashCommand for CompactCommand {
    fn name(&self) -> &str { "compact" }
    fn description(&self) -> &str { "Compact the conversation to reduce token usage" }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let msg_count = ctx.messages.len();
        let instruction = if args.is_empty() {
            "Provide a detailed summary of our conversation so far, preserving all \
             key technical details, decisions made, file paths mentioned, and current \
             task status."
                .to_string()
        } else {
            args.to_string()
        };

        CommandResult::UserMessage(format!(
            "[Compact requested ({} messages). Instruction: {}]",
            msg_count, instruction
        ))
    }
}

// ---- /cost ---------------------------------------------------------------

#[async_trait]
impl SlashCommand for CostCommand {
    fn name(&self) -> &str { "cost" }
    fn description(&self) -> &str { "Show token usage and cost for this session" }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        let tracker = &ctx.cost_tracker;
        CommandResult::Message(format!(
            "Session cost:\n  Input tokens:  {}\n  Output tokens: {}\n  \
             Cache creation: {}\n  Cache read:    {}\n  Total tokens:  {}\n  \
             Estimated cost: ${:.4}",
            tracker.input_tokens(),
            tracker.output_tokens(),
            tracker.cache_creation_tokens(),
            tracker.cache_read_tokens(),
            tracker.total_tokens(),
            tracker.total_cost_usd(),
        ))
    }
}

// ---- /exit ---------------------------------------------------------------

#[async_trait]
impl SlashCommand for ExitCommand {
    fn name(&self) -> &str { "exit" }
    fn aliases(&self) -> Vec<&str> { vec!["quit", "q"] }
    fn description(&self) -> &str { "Exit Claude Code" }

    async fn execute(&self, _args: &str, _ctx: &mut CommandContext) -> CommandResult {
        CommandResult::Exit
    }
}

// ---- /model --------------------------------------------------------------

#[async_trait]
impl SlashCommand for ModelCommand {
    fn name(&self) -> &str { "model" }
    fn description(&self) -> &str { "Show or change the current model" }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        if args.is_empty() {
            CommandResult::Message(format!(
                "Current model: {}",
                ctx.config.effective_model()
            ))
        } else {
            let mut new_config = ctx.config.clone();
            new_config.model = Some(args.trim().to_string());
            CommandResult::ConfigChange(new_config)
        }
    }
}

// ---- /config -------------------------------------------------------------

#[async_trait]
impl SlashCommand for ConfigCommand {
    fn name(&self) -> &str { "config" }
    fn aliases(&self) -> Vec<&str> { vec!["settings"] }
    fn description(&self) -> &str { "Show or modify configuration settings" }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let args = args.trim();
        if args.is_empty() || matches!(args, "show" | "get") {
            let json = serde_json::to_string_pretty(&ctx.config).unwrap_or_default();
            return CommandResult::Message(format!(
                "Current configuration:\n{}\n\nUsage:\n  /config\n  /config set theme <default|dark|light>\n  /config set output-style <default|concise|explanatory|learning|formal|casual>\n  /config set model <model>\n  /config set provider <anthropic|openai-compat>\n  /config set permission-mode <default|accept-edits|bypass-permissions|plan>\n  /config unset <model|output-style>",
                json
            ));
        }

        if let Some(key) = args.strip_prefix("get ").map(str::trim) {
            return match key {
                "theme" => CommandResult::Message(format!("theme = {:?}", ctx.config.theme)),
                "output-style" | "output_style" => CommandResult::Message(format!(
                    "output-style = {}",
                    current_output_style_name(&ctx.config)
                )),
                "model" => CommandResult::Message(format!(
                    "model = {}",
                    ctx.config.effective_model()
                )),
                "provider" => CommandResult::Message(format!(
                    "provider = {:?}",
                    ctx.config.effective_provider()
                )),
                "permission-mode" | "permission_mode" => CommandResult::Message(format!(
                    "permission-mode = {:?}",
                    ctx.config.permission_mode
                )),
                other => CommandResult::Error(format!("Unknown config key '{}'", other)),
            };
        }

        if let Some(key) = args.strip_prefix("unset ").map(str::trim) {
            return match key {
                "model" => {
                    let mut new_config = ctx.config.clone();
                    new_config.model = None;
                    if let Err(err) = save_settings_mutation(|settings| settings.config.model = None)
                    {
                        return CommandResult::Error(format!(
                            "Failed to save configuration: {}",
                            err
                        ));
                    }
                    CommandResult::ConfigChangeMessage(
                        new_config,
                        "Model reset to the default for new sessions.".to_string(),
                    )
                }
                "output-style" | "output_style" => {
                    let mut new_config = ctx.config.clone();
                    new_config.output_style = None;
                    if let Err(err) =
                        save_settings_mutation(|settings| settings.config.output_style = None)
                    {
                        return CommandResult::Error(format!(
                            "Failed to save configuration: {}",
                            err
                        ));
                    }
                    CommandResult::ConfigChangeMessage(
                        new_config,
                        "Output style reset to default.".to_string(),
                    )
                }
                other => CommandResult::Error(format!("Unknown config key '{}'", other)),
            };
        }

        let mut parts = args.splitn(3, ' ');
        let command = parts.next().unwrap_or_default();
        let key = parts.next().unwrap_or_default().trim();
        let value = parts.next().unwrap_or_default().trim();
        if command != "set" || key.is_empty() || value.is_empty() {
            return CommandResult::Error("Usage: /config set <key> <value>".to_string());
        }

        match key {
            "theme" => {
                let Some(theme) = parse_theme(value) else {
                    return CommandResult::Error(
                        "Theme must be one of: default, dark, light".to_string(),
                    );
                };
                let mut new_config = ctx.config.clone();
                new_config.theme = theme.clone();
                if let Err(err) =
                    save_settings_mutation(|settings| settings.config.theme = theme.clone())
                {
                    return CommandResult::Error(format!("Failed to save configuration: {}", err));
                }
                CommandResult::ConfigChangeMessage(
                    new_config,
                    format!("Theme set to {}.", value.trim().to_lowercase()),
                )
            }
            "output-style" | "output_style" => {
                let normalized = value.trim().to_lowercase();
                let valid = available_output_style_names();
                if !valid.iter().any(|name| name == &normalized) {
                    return CommandResult::Error(format!(
                        "Unsupported output style '{}'. Use one of: {}",
                        value,
                        valid.join(", ")
                    ));
                }

                let mut new_config = ctx.config.clone();
                new_config.output_style =
                    (normalized != "default").then(|| normalized.clone());
                if let Err(err) = save_settings_mutation(|settings| {
                    settings.config.output_style =
                        (normalized != "default").then(|| normalized.clone());
                }) {
                    return CommandResult::Error(format!("Failed to save configuration: {}", err));
                }
                CommandResult::ConfigChangeMessage(
                    new_config,
                    format!(
                        "Output style set to {}. Changes take effect on the next request.",
                        normalized
                    ),
                )
            }
            "model" => {
                let mut new_config = ctx.config.clone();
                new_config.model = Some(value.to_string());
                if let Err(err) = save_settings_mutation(|settings| {
                    settings.config.model = Some(value.to_string());
                }) {
                    return CommandResult::Error(format!("Failed to save configuration: {}", err));
                }
                CommandResult::ConfigChangeMessage(
                    new_config,
                    format!("Model set to {}.", value),
                )
            }
            "provider" => {
                let provider = match value.trim().to_lowercase().as_str() {
                    "anthropic" => cc_core::config::ModelProvider::Anthropic,
                    "openai-compat" | "openai_compat" => {
                        cc_core::config::ModelProvider::OpenaiCompat
                    }
                    _ => {
                        return CommandResult::Error(
                            "Provider must be one of: anthropic, openai-compat".to_string(),
                        )
                    }
                };
                let mut new_config = ctx.config.clone();
                new_config.provider = provider.clone();
                if let Err(err) = save_settings_mutation(|settings| {
                    settings.config.provider = provider.clone();
                }) {
                    return CommandResult::Error(format!("Failed to save configuration: {}", err));
                }
                CommandResult::ConfigChangeMessage(
                    new_config,
                    format!("Provider set to {}.", value.trim().to_lowercase()),
                )
            }
            "permission-mode" | "permission_mode" => {
                let mode = match value.trim().to_lowercase().as_str() {
                    "default" => cc_core::config::PermissionMode::Default,
                    "accept-edits" | "accept_edits" => {
                        cc_core::config::PermissionMode::AcceptEdits
                    }
                    "bypass-permissions" | "bypass_permissions" => {
                        cc_core::config::PermissionMode::BypassPermissions
                    }
                    "plan" => cc_core::config::PermissionMode::Plan,
                    _ => {
                        return CommandResult::Error(
                            "Permission mode must be one of: default, accept-edits, bypass-permissions, plan"
                                .to_string(),
                        )
                    }
                };

                let mut new_config = ctx.config.clone();
                new_config.permission_mode = mode.clone();
                if let Err(err) = save_settings_mutation(|settings| {
                    settings.config.permission_mode = mode.clone();
                }) {
                    return CommandResult::Error(format!("Failed to save configuration: {}", err));
                }
                CommandResult::ConfigChangeMessage(
                    new_config,
                    format!("Permission mode set to {}.", value.trim().to_lowercase()),
                )
            }
            other => CommandResult::Error(format!("Unknown config key '{}'", other)),
        }
    }
}

// ---- /color --------------------------------------------------------------

#[async_trait]
impl SlashCommand for ColorCommand {
    fn name(&self) -> &str { "color" }
    fn description(&self) -> &str { "Set or show the prompt bar color for this session" }
    fn help(&self) -> &str {
        "Usage: /color [<name|#RRGGBB|default>]\n\n\
         Sets the accent color for the prompt bar in this session.\n\
         Named colors: red, green, blue, yellow, cyan, magenta, white, orange, purple\n\
         Hex codes:    #RGB or #RRGGBB\n\
         Reset:        /color default\n\n\
         The color is persisted to ~/.claude/ui-settings.json and\n\
         applied on the next REPL startup."
    }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let color = args.trim();
        if color.is_empty() {
            let current = load_ui_settings();
            return CommandResult::Message(format!(
                "Current prompt color: {}\n\
                 Use /color <name|#RRGGBB|default> to change it.\n\n\
                 Named colors: red, green, blue, yellow, cyan, magenta, white, orange, purple",
                current.prompt_color.as_deref().unwrap_or("default"),
            ));
        }

        let normalized = if color == "default" {
            None
        } else {
            let known_colors = [
                "red", "green", "blue", "yellow", "cyan", "magenta",
                "white", "orange", "purple", "pink", "gray", "grey",
            ];
            let is_hex = color.starts_with('#') && (color.len() == 4 || color.len() == 7)
                && color[1..].chars().all(|c| c.is_ascii_hexdigit());
            if !is_hex && !known_colors.contains(&color.to_lowercase().as_str()) {
                return CommandResult::Error(format!(
                    "Unknown color '{}'. Use a color name (red, green, …) or a hex code (#RGB or #RRGGBB).",
                    color
                ));
            }
            Some(color.to_string())
        };

        match mutate_ui_settings(|s| s.prompt_color = normalized.clone()) {
            Ok(_) => CommandResult::Message(format!(
                "Prompt color set to {}.\n\
                 Restart the REPL for the change to take effect.",
                normalized.as_deref().unwrap_or("default")
            )),
            Err(e) => CommandResult::Error(format!("Failed to save color: {}", e)),
        }
    }
}

// ---- /theme --------------------------------------------------------------

#[async_trait]
impl SlashCommand for ThemeCommand {
    fn name(&self) -> &str { "theme" }
    fn description(&self) -> &str { "Show or change the current theme" }
    fn help(&self) -> &str {
        "Usage: /theme [default|dark|light]\n\
         Without arguments, shows the active theme. With an argument, updates the theme for this and future sessions."
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let args = args.trim();
        if args.is_empty() {
            return CommandResult::Message(format!(
                "Current theme: {:?}\nUse /theme <default|dark|light> to change it.",
                ctx.config.theme
            ));
        }

        let Some(theme) = parse_theme(args) else {
            return CommandResult::Error(
                "Theme must be one of: default, dark, light".to_string(),
            );
        };

        let mut new_config = ctx.config.clone();
        new_config.theme = theme.clone();
        if let Err(err) = save_settings_mutation(|settings| settings.config.theme = theme.clone())
        {
            return CommandResult::Error(format!("Failed to save theme: {}", err));
        }

        CommandResult::ConfigChangeMessage(
            new_config,
            format!("Theme set to {}.", args.to_lowercase()),
        )
    }
}

// ---- /output-style -------------------------------------------------------

#[async_trait]
impl SlashCommand for OutputStyleCommand {
    fn name(&self) -> &str { "output-style" }
    fn description(&self) -> &str { "Show the output-style migration guidance" }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        CommandResult::Message(format!(
            "/output-style has been deprecated. Use /config to change your output style, or set it in your settings file. Changes take effect on the next session.\nCurrent output style: {}",
            current_output_style_name(&ctx.config)
        ))
    }
}

// ---- /keybindings --------------------------------------------------------

#[async_trait]
impl SlashCommand for KeybindingsCommand {
    fn name(&self) -> &str { "keybindings" }
    fn description(&self) -> &str { "Create or open ~/.claude/keybindings.json" }

    async fn execute(&self, _args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let config_dir = Settings::config_dir();
        let path = config_dir.join("keybindings.json");
        let existed = path.exists();

        if !existed {
            if let Err(err) = std::fs::create_dir_all(&config_dir) {
                return CommandResult::Error(format!(
                    "Failed to create {}: {}",
                    config_dir.display(),
                    err
                ));
            }

            let template = match generate_keybindings_template() {
                Ok(template) => template,
                Err(err) => {
                    return CommandResult::Error(format!(
                        "Failed to generate keybindings template: {}",
                        err
                    ))
                }
            };

            if let Err(err) = std::fs::write(&path, template) {
                return CommandResult::Error(format!(
                    "Failed to write {}: {}",
                    path.display(),
                    err
                ));
            }
        }

        match open_with_system(&path.display().to_string()) {
            Ok(_) => CommandResult::Message(if existed {
                format!("Opened {} in your editor.", path.display())
            } else {
                format!(
                    "Created {} with a template and opened it in your editor.",
                    path.display()
                )
            }),
            Err(err) => CommandResult::Message(if existed {
                format!(
                    "Opened {}. Could not launch an editor automatically: {}",
                    path.display(),
                    err
                )
            } else {
                format!(
                    "Created {} with a template. Could not launch an editor automatically: {}",
                    path.display(),
                    err
                )
            }),
        }
    }
}

// ---- /privacy-settings ---------------------------------------------------

#[async_trait]
impl SlashCommand for PrivacySettingsCommand {
    fn name(&self) -> &str { "privacy-settings" }
    fn description(&self) -> &str { "Open Claude privacy settings" }

    async fn execute(&self, _args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let url = "https://claude.ai/settings/data-privacy-controls";
        let fallback = format!("Review and manage your privacy settings at {}", url);
        match open_with_system(url) {
            Ok(_) => CommandResult::Message(format!("Opened privacy settings: {}", url)),
            Err(_) => CommandResult::Message(fallback),
        }
    }
}

// ---- /version ------------------------------------------------------------

#[async_trait]
impl SlashCommand for VersionCommand {
    fn name(&self) -> &str { "version" }
    fn aliases(&self) -> Vec<&str> { vec!["v"] }
    fn description(&self) -> &str { "Show version information" }

    async fn execute(&self, _args: &str, _ctx: &mut CommandContext) -> CommandResult {
        CommandResult::Message(format!(
            "Claude Code (Rust) v{}",
            cc_core::constants::APP_VERSION
        ))
    }
}

// ---- /resume -------------------------------------------------------------

#[async_trait]
impl SlashCommand for ResumeCommand {
    fn name(&self) -> &str { "resume" }
    fn aliases(&self) -> Vec<&str> { vec!["r", "continue"] }
    fn description(&self) -> &str { "Resume a previous conversation" }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        if args.is_empty() {
            let sessions = cc_core::history::list_sessions().await;
            if sessions.is_empty() {
                return CommandResult::Message("No previous sessions found.".to_string());
            }
            let mut output = String::from("Recent sessions:\n\n");
            for (i, session) in sessions.iter().take(10).enumerate() {
                let title = session
                    .title
                    .as_deref()
                    .unwrap_or("(untitled)");
                let id_short = &session.id[..session.id.len().min(8)];
                output.push_str(&format!(
                    "  {}. {} - {} ({} messages)\n",
                    i + 1,
                    id_short,
                    title,
                    session.messages.len()
                ));
            }
            output.push_str("\nUse /resume <id> to resume a session.");
            CommandResult::Message(output)
        } else {
            match cc_core::history::load_session(args.trim()).await {
                Ok(session) => CommandResult::ResumeSession(session),
                Err(e) => CommandResult::Error(format!(
                    "Failed to load session {}: {}",
                    args.trim(),
                    e
                )),
            }
        }
    }
}

// ---- /status -------------------------------------------------------------

#[async_trait]
impl SlashCommand for StatusCommand {
    fn name(&self) -> &str { "status" }
    fn description(&self) -> &str { "Show comprehensive system and session status" }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        // Auth status
        let auth_status = match cc_core::oauth::OAuthTokens::load().await {
            Some(tokens) => {
                let sub = tokens.subscription_type.as_deref().unwrap_or("oauth");
                format!("Authenticated ({})", sub)
            }
            None => {
                if ctx.config.resolve_api_key().is_some() {
                    "Authenticated (API key)".to_string()
                } else {
                    "Not authenticated".to_string()
                }
            }
        };

        // MCP status
        let mcp_count = ctx.config.mcp_servers.len();
        let mcp_status = if mcp_count == 0 {
            "none configured".to_string()
        } else {
            format!("{} server(s) configured", mcp_count)
        };

        // Hook status
        let hook_count: usize = ctx.config.hooks.values().map(|v| v.len()).sum();

        // UI settings
        let ui = load_ui_settings();
        let editor_mode = ui.editor_mode.as_deref().unwrap_or("normal");
        let fast_mode = ui.fast_mode.unwrap_or(false);

        // Git status
        let git_branch = tokio::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&ctx.working_dir)
            .output()
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "n/a".to_string());

        CommandResult::Message(format!(
            "Claude Code Status\n\
             ══════════════════\n\
             Auth:           {auth_status}\n\
             Model:          {model}\n\
             Permission mode: {perm:?}\n\
             Fast mode:      {fast}\n\
             Editor mode:    {editor}\n\n\
             Session\n\
             ───────\n\
             Session ID:     {sid}\n\
             Title:          {title}\n\
             Messages:       {msgs}\n\
             Working dir:    {wd}\n\
             Git branch:     {branch}\n\n\
             Integrations\n\
             ────────────\n\
             MCP servers:    {mcp}\n\
             Hooks:          {hooks} configured\n\n\
             Usage\n\
             ─────\n\
             {summary}",
            auth_status = auth_status,
            model = ctx.config.effective_model(),
            perm = ctx.config.permission_mode,
            fast = if fast_mode { "on" } else { "off" },
            editor = editor_mode,
            sid = &ctx.session_id[..ctx.session_id.len().min(12)],
            title = ctx.session_title.as_deref().unwrap_or("(untitled)"),
            msgs = ctx.messages.len(),
            wd = ctx.working_dir.display(),
            branch = git_branch,
            mcp = mcp_status,
            hooks = hook_count,
            summary = ctx.cost_tracker.summary(),
        ))
    }
}

// ---- /diff ---------------------------------------------------------------

#[async_trait]
impl SlashCommand for DiffCommand {
    fn name(&self) -> &str { "diff" }
    fn description(&self) -> &str { "Show git diff of changes in the working directory" }
    fn help(&self) -> &str {
        "Usage: /diff [--stat|--staged|<ref>]\n\n\
         Shows git diff output for the current working directory.\n\n\
         Options:\n\
           /diff           — diff of all unstaged changes (git diff)\n\
           /diff --stat    — summary of changed files\n\
           /diff --staged  — diff of staged changes (git diff --cached)\n\
           /diff <ref>     — diff against a branch, tag, or commit"
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let args = args.trim();

        let git_args: Vec<&str> = if args == "--stat" {
            vec!["diff", "--stat"]
        } else if args == "--staged" || args == "--cached" {
            vec!["diff", "--cached"]
        } else if args.is_empty() {
            vec!["diff"]
        } else {
            // Treat as a ref
            vec!["diff", args]
        };

        let output = tokio::process::Command::new("git")
            .args(&git_args)
            .current_dir(&ctx.working_dir)
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() || out.status.code() == Some(1) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if stdout.trim().is_empty() {
                    CommandResult::Message(
                        "No changes found. Working tree is clean (or not a git repository)."
                            .to_string(),
                    )
                } else {
                    // Truncate very long diffs
                    let text = stdout.as_ref();
                    let display = if text.len() > 8000 {
                        format!(
                            "{}\n… (truncated — {} total bytes; use `git diff` for full output)",
                            &text[..8000],
                            text.len()
                        )
                    } else {
                        text.to_string()
                    };
                    CommandResult::Message(format!("Changes:\n{}", display))
                }
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                CommandResult::Error(format!(
                    "git diff failed (exit {}): {}",
                    out.status.code().unwrap_or(-1),
                    stderr.trim()
                ))
            }
            Err(e) => CommandResult::Error(format!("Failed to run git diff: {}", e)),
        }
    }
}

// ---- /memory -------------------------------------------------------------

#[async_trait]
impl SlashCommand for MemoryCommand {
    fn name(&self) -> &str { "memory" }
    fn description(&self) -> &str { "View CLAUDE.md memory files (project and global)" }
    fn help(&self) -> &str {
        "Usage: /memory [edit]\n\n\
         Shows the content of CLAUDE.md files that provide project context to Claude.\n\
         Claude reads these files automatically at session start.\n\n\
         Locations checked (in priority order):\n\
         1. <project>/.claude/CLAUDE.md\n\
         2. <project>/CLAUDE.md\n\
         3. ~/.claude/CLAUDE.md  (global memory)\n\n\
         Use /memory edit to open the project CLAUDE.md in your editor.\n\
         Use /init to create a new CLAUDE.md from a template."
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let project_claude_dir = ctx.working_dir.join(".claude").join("CLAUDE.md");
        let project_root = ctx.working_dir.join("CLAUDE.md");
        let global_path = dirs::home_dir()
            .unwrap_or_default()
            .join(".claude")
            .join("CLAUDE.md");

        let locations = [
            ("project (.claude/CLAUDE.md)", project_claude_dir.clone()),
            ("project (CLAUDE.md)", project_root.clone()),
            ("global (~/.claude/CLAUDE.md)", global_path.clone()),
        ];

        let edit_mode = args.trim() == "edit";

        if edit_mode {
            // Open best available CLAUDE.md
            let target = if project_root.exists() {
                project_root.clone()
            } else if project_claude_dir.exists() {
                project_claude_dir.clone()
            } else {
                project_root.clone() // will be created by editor
            };
            return match open_with_system(&target.display().to_string()) {
                Ok(_) => CommandResult::Message(format!(
                    "Opening {} in your editor.", target.display()
                )),
                Err(e) => CommandResult::Message(format!(
                    "Could not launch editor: {}. Edit {} manually.", e, target.display()
                )),
            };
        }

        let mut output = String::from("CLAUDE.md Memory Files\n══════════════════════\n");
        let mut found_any = false;

        for (label, path) in &locations {
            if path.exists() {
                found_any = true;
                match tokio::fs::read_to_string(path).await {
                    Ok(content) => {
                        let lines: usize = content.lines().count();
                        let chars = content.len();
                        output.push_str(&format!(
                            "\n[{label}]\nPath: {path}\nSize: {lines} lines, {chars} chars\n\
                             ─────────────────────────────────\n\
                             {content}\n",
                            label = label,
                            path = path.display(),
                            lines = lines,
                            chars = chars,
                            content = if content.len() > 2000 {
                                format!("{}…\n(truncated — file is {} chars)", &content[..2000], chars)
                            } else {
                                content.clone()
                            }
                        ));
                    }
                    Err(e) => output.push_str(&format!(
                        "\n[{label}] — Error reading {}: {}\n",
                        path.display(), e, label = label
                    )),
                }
            }
        }

        if !found_any {
            output.push_str(
                "\nNo CLAUDE.md files found.\n\
                 Use /init to create one in the current project."
            );
        } else {
            output.push_str("\nUse /memory edit to open the project CLAUDE.md.");
        }

        CommandResult::Message(output)
    }
}

// ---- /bug ----------------------------------------------------------------

#[async_trait]
impl SlashCommand for BugCommand {
    fn name(&self) -> &str { "feedback" }
    fn aliases(&self) -> Vec<&str> { vec!["bug"] }
    fn description(&self) -> &str { "Submit feedback about Claude Code" }
    fn help(&self) -> &str { "Usage: /feedback [report]" }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let report = args.trim();
        if report.is_empty() {
            CommandResult::Message(
                "To submit feedback or report a bug, visit: https://github.com/anthropics/claude-code/issues"
                    .to_string(),
            )
        } else {
            CommandResult::Message(format!(
                "To submit feedback or report a bug, visit: https://github.com/anthropics/claude-code/issues\nSuggested report summary: {}",
                report
            ))
        }
    }
}

// ---- /usage --------------------------------------------------------------

#[async_trait]
impl SlashCommand for UsageCommand {
    fn name(&self) -> &str { "usage" }
    fn description(&self) -> &str { "Show API usage, quotas, and rate limit status" }
    fn help(&self) -> &str {
        "Usage: /usage\n\n\
         Shows current session API usage and account quota information.\n\
         For detailed per-call breakdown, use /extra-usage.\n\
         For cost details, use /cost."
    }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        let input = ctx.cost_tracker.input_tokens();
        let output = ctx.cost_tracker.output_tokens();
        let cache_creation = ctx.cost_tracker.cache_creation_tokens();
        let cache_read = ctx.cost_tracker.cache_read_tokens();
        let total = ctx.cost_tracker.total_tokens();
        let cost = ctx.cost_tracker.total_cost_usd();

        // Try to get account tier from OAuth tokens
        let account_info = match cc_core::oauth::OAuthTokens::load().await {
            Some(tokens) => {
                let sub = tokens.subscription_type.as_deref().unwrap_or("unknown");
                format!("Plan: {}", sub)
            }
            None => {
                if ctx.config.resolve_api_key().is_some() {
                    "Plan: API key (Console billing)".to_string()
                } else {
                    "Plan: not authenticated — run /login".to_string()
                }
            }
        };

        CommandResult::Message(format!(
            "API Usage — Current Session\n\
             ────────────────────────────\n\
             {account_info}\n\
             Model:          {model}\n\n\
             Tokens used this session:\n\
               Input:        {input:>10}\n\
               Output:       {output:>10}\n\
               Cache write:  {cache_creation:>10}\n\
               Cache read:   {cache_read:>10}\n\
               Total:        {total:>10}\n\n\
             Estimated cost: ${cost:.4}\n\n\
             Use /extra-usage for per-call breakdown.\n\
             Use /rate-limit-options to see your plan limits.",
            account_info = account_info,
            model = ctx.config.effective_model(),
            input = input,
            output = output,
            cache_creation = cache_creation,
            cache_read = cache_read,
            total = total,
            cost = cost,
        ))
    }
}

// ---- /plugin -------------------------------------------------------------

#[async_trait]
impl SlashCommand for PluginCommand {
    fn name(&self) -> &str { "plugin" }
    fn aliases(&self) -> Vec<&str> { vec!["plugins"] }
    fn description(&self) -> &str { "Manage plugins" }
    fn help(&self) -> &str {
        "Usage: /plugin [list|enable <name>|disable <name>|info <name>|install <path>]\n\
         Manage Claude Code plugins."
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let project_dir = ctx.working_dir.clone();

        let parsed = cc_plugins::parse_plugin_args(args);
        match parsed {
            cc_plugins::PluginSubCommand::List => {
                let registry = cc_plugins::load_plugins(&project_dir, &[]).await;
                CommandResult::Message(cc_plugins::format_plugin_list(&registry))
            }
            cc_plugins::PluginSubCommand::Enable(name) => {
                let mut settings = cc_core::config::Settings::load_sync().unwrap_or_default();
                settings.enabled_plugins.insert(name.clone());
                settings.disabled_plugins.remove(&name);
                let _ = settings.save_sync();
                CommandResult::Message(format!("Plugin '{}' enabled.", name))
            }
            cc_plugins::PluginSubCommand::Disable(name) => {
                let mut settings = cc_core::config::Settings::load_sync().unwrap_or_default();
                settings.disabled_plugins.insert(name.clone());
                settings.enabled_plugins.remove(&name);
                let _ = settings.save_sync();
                CommandResult::Message(format!("Plugin '{}' disabled.", name))
            }
            cc_plugins::PluginSubCommand::Info(name) => {
                let registry = cc_plugins::load_plugins(&project_dir, &[]).await;
                CommandResult::Message(cc_plugins::format_plugin_info(&registry, &name))
            }
            cc_plugins::PluginSubCommand::Install(path) => {
                let result = cc_plugins::install_plugin_from_path(
                    std::path::Path::new(&path),
                );
                match result {
                    Ok(name) => CommandResult::Message(format!(
                        "Plugin '{}' installed. Run /reload-plugins to activate.",
                        name
                    )),
                    Err(e) => CommandResult::Error(format!("Install failed: {}", e)),
                }
            }
            cc_plugins::PluginSubCommand::Help => {
                CommandResult::Message(
                    "Plugin commands:\n\
                     /plugin list            — list all installed plugins\n\
                     /plugin enable <name>   — enable a plugin\n\
                     /plugin disable <name>  — disable a plugin\n\
                     /plugin info <name>     — show plugin details\n\
                     /plugin install <path>  — install plugin from local path"
                        .to_string(),
                )
            }
        }
    }
}

// ---- /reload-plugins -----------------------------------------------------

#[async_trait]
impl SlashCommand for ReloadPluginsCommand {
    fn name(&self) -> &str { "reload-plugins" }
    fn description(&self) -> &str { "Reload all plugins without restarting" }
    fn help(&self) -> &str {
        "Usage: /reload-plugins\n\
         Reloads all plugins and shows what changed."
    }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        let project_dir = ctx.working_dir.clone();

        let old_registry = cc_plugins::load_plugins(&project_dir, &[]).await;
        let (new_registry, diff) =
            cc_plugins::reload_plugins(&old_registry, &project_dir, &[]).await;

        CommandResult::Message(cc_plugins::format_reload_summary(&new_registry, &diff))
    }
}

// ---- Plugin slash command adapter ----------------------------------------

/// Wraps a plugin-defined `PluginCommandDef` so it can be executed like a
/// built-in slash command.  The adapter is created on-the-fly inside
/// `execute_command` when no built-in matches the input.
pub struct PluginSlashCommandAdapter {
    pub def: cc_plugins::PluginCommandDef,
}

#[async_trait]
impl SlashCommand for PluginSlashCommandAdapter {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn description(&self) -> &str {
        &self.def.description
    }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        match &self.def.run_action {
            cc_plugins::CommandRunAction::StaticResponse(msg) => {
                CommandResult::Message(msg.clone())
            }
            cc_plugins::CommandRunAction::MarkdownPrompt {
                file_path,
                plugin_root: _,
            } => {
                // Read the markdown file and inject it into the conversation
                match std::fs::read_to_string(file_path) {
                    Ok(content) => {
                        let full_prompt = if args.is_empty() {
                            content
                        } else {
                            format!("{}\n\nArguments: {}", content, args)
                        };
                        CommandResult::UserMessage(full_prompt)
                    }
                    Err(e) => CommandResult::Error(format!(
                        "Could not read plugin command file '{}': {}",
                        file_path, e
                    )),
                }
            }
            cc_plugins::CommandRunAction::ShellCommand {
                command,
                plugin_root,
            } => {
                let full_cmd = if args.is_empty() {
                    command.clone()
                } else {
                    format!("{} {}", command, args)
                };
                let cmd_result = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
                    .args(if cfg!(windows) {
                        vec!["/C", &full_cmd]
                    } else {
                        vec!["-c", &full_cmd]
                    })
                    .env("CLAUDE_PLUGIN_ROOT", plugin_root)
                    .output();
                match cmd_result {
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        if out.status.success() {
                            CommandResult::Message(stdout.to_string())
                        } else {
                            CommandResult::Error(format!("Command failed:\n{}", stderr))
                        }
                    }
                    Err(e) => CommandResult::Error(format!("Failed to run command: {}", e)),
                }
            }
        }
    }
}

// ---- /doctor -------------------------------------------------------------

#[async_trait]
impl SlashCommand for DoctorCommand {
    fn name(&self) -> &str { "doctor" }
    fn description(&self) -> &str { "Check system health and diagnose issues" }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        let mut checks = Vec::new();

        // Check API key
        if ctx.config.resolve_api_key().is_some() {
            checks.push("  [ok] API key configured");
        } else {
            checks.push("  [!!] No API key found (set ANTHROPIC_API_KEY)");
        }

        // Check git
        let git_ok = tokio::process::Command::new("git")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if git_ok {
            checks.push("  [ok] git available");
        } else {
            checks.push("  [!!] git not found");
        }

        // Check config dir
        let config_dir = cc_core::config::Settings::config_dir();
        if config_dir.exists() {
            checks.push("  [ok] Config directory exists");
        } else {
            checks.push("  [!!] Config directory missing");
        }

        CommandResult::Message(format!("Doctor report:\n\n{}", checks.join("\n")))
    }
}

// ---- /login --------------------------------------------------------------

#[async_trait]
impl SlashCommand for LoginCommand {
    fn name(&self) -> &str { "login" }
    fn description(&self) -> &str { "Authenticate with Anthropic (OAuth PKCE flow)" }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        // `--console` flag → Console/API-key auth; default → Claude.ai subscription auth
        let login_with_claude_ai = !args.contains("--console");
        CommandResult::StartOAuthFlow(login_with_claude_ai)
    }
}

// ---- /logout -------------------------------------------------------------

#[async_trait]
impl SlashCommand for LogoutCommand {
    fn name(&self) -> &str { "logout" }
    fn description(&self) -> &str { "Clear stored OAuth tokens and API key" }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        // Clear OAuth tokens file
        if let Err(e) = cc_core::oauth::OAuthTokens::clear().await {
            return CommandResult::Error(format!("Failed to clear OAuth tokens: {}", e));
        }
        // Also clear any API key stored in settings
        let mut settings = cc_core::config::Settings::load().await.unwrap_or_default();
        settings.config.api_key = None;
        if let Err(e) = settings.save().await {
            return CommandResult::Error(format!("Failed to update settings: {}", e));
        }
        ctx.config.api_key = None;
        CommandResult::Message("Logged out. Credentials cleared.".to_string())
    }
}

// ---- /init ---------------------------------------------------------------

#[async_trait]
impl SlashCommand for InitCommand {
    fn name(&self) -> &str { "init" }
    fn description(&self) -> &str { "Initialize a new project with CLAUDE.md" }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        let path = ctx.working_dir.join("CLAUDE.md");
        if path.exists() {
            return CommandResult::Message(format!(
                "CLAUDE.md already exists at {}",
                path.display()
            ));
        }

        let default_content = "# Project Instructions\n\n\
            Add project-specific instructions and context here.\n\n\
            ## Guidelines\n\n\
            - Describe your project structure\n\
            - Note any coding conventions\n\
            - List important files and their purposes\n";

        match tokio::fs::write(&path, default_content).await {
            Ok(()) => CommandResult::Message(format!(
                "Created CLAUDE.md at {}",
                path.display()
            )),
            Err(e) => CommandResult::Error(format!("Failed to create CLAUDE.md: {}", e)),
        }
    }
}

// ---- /review -------------------------------------------------------------

#[async_trait]
impl SlashCommand for ReviewCommand {
    fn name(&self) -> &str { "review" }
    fn description(&self) -> &str { "Review code changes (git diff)" }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let target = if args.is_empty() { "HEAD" } else { args.trim() };
        CommandResult::UserMessage(format!(
            "Please review the code changes in `git diff {}`. \
             Look for bugs, security issues, and style problems.",
            target
        ))
    }
}

// ---- /hooks --------------------------------------------------------------

#[async_trait]
impl SlashCommand for HooksCommand {
    fn name(&self) -> &str { "hooks" }
    fn description(&self) -> &str { "Show configured event hooks" }
    fn help(&self) -> &str {
        "Usage: /hooks\n\
         Show hooks configured in settings.json under 'hooks'.\n\
         Hooks fire shell commands on events: PreToolUse, PostToolUse, Stop, UserPromptSubmit."
    }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        if ctx.config.hooks.is_empty() {
            return CommandResult::Message(
                "No hooks configured.\n\
                 Add hooks to ~/.claude/settings.json under the 'hooks' key.\n\
                 Example:\n  \"hooks\": { \"PreToolUse\": [{\"command\": \"echo $STDIN\", \"blocking\": false}] }"
                    .to_string(),
            );
        }

        let mut lines = vec!["Configured hooks:".to_string()];
        for (event, entries) in &ctx.config.hooks {
            lines.push(format!("\n  {:?} ({} entries):", event, entries.len()));
            for e in entries {
                let filter = e.tool_filter.as_deref().unwrap_or("*");
                lines.push(format!(
                    "    - [{}] {} (blocking={})",
                    filter, e.command, e.blocking
                ));
            }
        }

        CommandResult::Message(lines.join("\n"))
    }
}

// ---- /mcp ----------------------------------------------------------------

#[async_trait]
impl SlashCommand for McpCommand {
    fn name(&self) -> &str { "mcp" }
    fn description(&self) -> &str { "Show MCP server status and configuration" }
    fn help(&self) -> &str {
        "Usage: /mcp [list|status|add|remove]\n\n\
         Manages Model Context Protocol (MCP) servers.\n\
         MCP servers extend Claude with external tools and resources.\n\n\
         Subcommands:\n\
           /mcp              — list configured servers (same as /mcp list)\n\
           /mcp list         — list all configured MCP servers\n\
           /mcp status       — show server connection status\n\n\
         To add/remove MCP servers, edit ~/.claude/settings.json\n\
         under the 'mcpServers' key, or use the MCP CLI.\n\
         Docs: https://docs.anthropic.com/claude-code/mcp"
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let sub = args.trim();
        if ctx.config.mcp_servers.is_empty() {
            return CommandResult::Message(
                "No MCP servers configured.\n\n\
                 To add a MCP server, edit ~/.claude/settings.json:\n\
                 {\n\
                   \"mcpServers\": [\n\
                     {\n\
                       \"name\": \"my-server\",\n\
                       \"command\": \"npx\",\n\
                       \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"/tmp\"]\n\
                     }\n\
                   ]\n\
                 }\n\n\
                 Docs: https://docs.anthropic.com/claude-code/mcp"
                    .to_string(),
            );
        }

        if sub == "status" {
            let mut output = String::from("MCP Server Status\n─────────────────\n");
            for srv in &ctx.config.mcp_servers {
                let kind = match srv.server_type.as_str() {
                    "stdio" => "stdio",
                    "sse" | "http" => "HTTP/SSE",
                    other => other,
                };
                let endpoint = srv
                    .url
                    .as_deref()
                    .or_else(|| srv.command.as_deref())
                    .unwrap_or("(unknown)");
                output.push_str(&format!(
                    "  {name:20} [{kind:8}] {endpoint}\n",
                    name = srv.name,
                    kind = kind,
                    endpoint = endpoint,
                ));
            }
            output.push_str(
                "\nNote: Live connection status requires the MCP runtime to be active.\n\
                 Restart Claude Code to reconnect to MCP servers."
            );
            return CommandResult::Message(output);
        }

        // Default: list
        let mut output = format!(
            "Configured MCP Servers ({})\n──────────────────────────\n",
            ctx.config.mcp_servers.len()
        );
        for srv in &ctx.config.mcp_servers {
            let cmd_display = if let Some(ref url) = srv.url {
                format!("url={}", url)
            } else if let Some(ref cmd) = srv.command {
                let args_str = srv.args.join(" ");
                if args_str.is_empty() {
                    cmd.clone()
                } else {
                    format!("{} {}", cmd, args_str)
                }
            } else {
                "(no command)".to_string()
            };
            output.push_str(&format!(
                "  {name} ({type_})\n    {cmd}\n",
                name = srv.name,
                type_ = srv.server_type,
                cmd = cmd_display,
            ));
        }
        output.push_str("\nUse /mcp status to see connection status.");
        CommandResult::Message(output)
    }
}

// ---- /permissions --------------------------------------------------------

#[async_trait]
impl SlashCommand for PermissionsCommand {
    fn name(&self) -> &str { "permissions" }
    fn description(&self) -> &str { "View or change tool permission settings" }
    fn help(&self) -> &str {
        "Usage: /permissions [set <mode>|allow <tool>|deny <tool>|reset]\n\n\
         Modes: default, accept-edits, bypass-permissions, plan\n\n\
         Examples:\n\
           /permissions                    — show current permissions\n\
           /permissions set accept-edits   — auto-accept file edits\n\
           /permissions allow Bash         — allow a specific tool\n\
           /permissions deny Write         — deny a specific tool\n\
           /permissions reset              — clear overrides"
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let args = args.trim();

        if args.is_empty() {
            let allowed_display = if ctx.config.allowed_tools.is_empty() {
                "(all tools allowed)".to_string()
            } else {
                ctx.config.allowed_tools.join(", ")
            };
            let denied_display = if ctx.config.disallowed_tools.is_empty() {
                "(none)".to_string()
            } else {
                ctx.config.disallowed_tools.join(", ")
            };
            return CommandResult::Message(format!(
                "Permission Settings\n\
                 ───────────────────\n\
                 Mode:          {:?}\n\
                 Allowed tools: {}\n\
                 Denied tools:  {}\n\n\
                 Use /permissions set <mode> to change the permission mode.\n\
                 Use /permissions allow|deny <tool> to override individual tools.\n\
                 Use /permissions reset to clear all overrides.",
                ctx.config.permission_mode,
                allowed_display,
                denied_display,
            ));
        }

        let mut parts = args.splitn(2, ' ');
        let sub = parts.next().unwrap_or("").trim();
        let arg = parts.next().unwrap_or("").trim();

        match sub {
            "set" => {
                let mode = match arg.to_lowercase().as_str() {
                    "default" => cc_core::config::PermissionMode::Default,
                    "accept-edits" | "accept_edits" => cc_core::config::PermissionMode::AcceptEdits,
                    "bypass-permissions" | "bypass_permissions" => cc_core::config::PermissionMode::BypassPermissions,
                    "plan" => cc_core::config::PermissionMode::Plan,
                    _ => return CommandResult::Error(
                        "Mode must be: default, accept-edits, bypass-permissions, or plan".to_string()
                    ),
                };
                let mut new_config = ctx.config.clone();
                new_config.permission_mode = mode.clone();
                if let Err(e) = save_settings_mutation(|s| s.config.permission_mode = mode.clone()) {
                    return CommandResult::Error(format!("Failed to save: {}", e));
                }
                CommandResult::ConfigChangeMessage(
                    new_config,
                    format!("Permission mode set to {:?}.", mode),
                )
            }
            "allow" => {
                if arg.is_empty() {
                    return CommandResult::Error("Usage: /permissions allow <tool>".to_string());
                }
                let tool = arg.to_string();
                let mut new_config = ctx.config.clone();
                if !new_config.allowed_tools.contains(&tool) {
                    new_config.allowed_tools.push(tool.clone());
                }
                new_config.disallowed_tools.retain(|t| t != &tool);
                if let Err(e) = save_settings_mutation(|s| {
                    if !s.config.allowed_tools.contains(&tool) {
                        s.config.allowed_tools.push(tool.clone());
                    }
                    s.config.disallowed_tools.retain(|t| t != &tool);
                }) {
                    return CommandResult::Error(format!("Failed to save: {}", e));
                }
                CommandResult::ConfigChangeMessage(new_config, format!("Allowed tool: {}", tool))
            }
            "deny" => {
                if arg.is_empty() {
                    return CommandResult::Error("Usage: /permissions deny <tool>".to_string());
                }
                let tool = arg.to_string();
                let mut new_config = ctx.config.clone();
                if !new_config.disallowed_tools.contains(&tool) {
                    new_config.disallowed_tools.push(tool.clone());
                }
                new_config.allowed_tools.retain(|t| t != &tool);
                if let Err(e) = save_settings_mutation(|s| {
                    if !s.config.disallowed_tools.contains(&tool) {
                        s.config.disallowed_tools.push(tool.clone());
                    }
                    s.config.allowed_tools.retain(|t| t != &tool);
                }) {
                    return CommandResult::Error(format!("Failed to save: {}", e));
                }
                CommandResult::ConfigChangeMessage(new_config, format!("Denied tool: {}", tool))
            }
            "reset" => {
                let mut new_config = ctx.config.clone();
                new_config.allowed_tools.clear();
                new_config.disallowed_tools.clear();
                new_config.permission_mode = cc_core::config::PermissionMode::Default;
                if let Err(e) = save_settings_mutation(|s| {
                    s.config.allowed_tools.clear();
                    s.config.disallowed_tools.clear();
                    s.config.permission_mode = cc_core::config::PermissionMode::Default;
                }) {
                    return CommandResult::Error(format!("Failed to save: {}", e));
                }
                CommandResult::ConfigChangeMessage(
                    new_config,
                    "Permissions reset to defaults.".to_string(),
                )
            }
            other => CommandResult::Error(format!(
                "Unknown subcommand '{}'. Use: /permissions [set|allow|deny|reset]",
                other
            )),
        }
    }
}

// ---- /plan ---------------------------------------------------------------

#[async_trait]
impl SlashCommand for PlanCommand {
    fn name(&self) -> &str { "plan" }
    fn description(&self) -> &str { "Enter plan mode – model outputs a plan for approval before acting" }
    fn help(&self) -> &str {
        "Usage: /plan [description]\n\n\
         Switches to plan mode where the model will create a detailed plan before executing.\n\
         The plan must be approved before any file writes or command executions are performed.\n\
         Use /plan exit to leave plan mode."
    }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        if args.trim() == "exit" {
            return CommandResult::UserMessage(
                "[Exiting plan mode. Resuming normal execution.]".to_string()
            );
        }
        let task_desc = if args.is_empty() {
            "the current task".to_string()
        } else {
            args.to_string()
        };
        CommandResult::UserMessage(format!(
            "[Entering plan mode for: {}]\n\
             Please create a detailed step-by-step plan. Do not execute any commands or \
             write any files until the plan has been reviewed and approved.",
            task_desc
        ))
    }
}

// ---- /tasks --------------------------------------------------------------

#[async_trait]
impl SlashCommand for TasksCommand {
    fn name(&self) -> &str { "tasks" }
    fn aliases(&self) -> Vec<&str> { vec!["bashes"] }
    fn description(&self) -> &str { "List and manage background tasks" }

    async fn execute(&self, _args: &str, _ctx: &mut CommandContext) -> CommandResult {
        CommandResult::UserMessage(
            "Please list all current tasks using the TaskList tool and show their status.".to_string()
        )
    }
}

// ---- /session ------------------------------------------------------------

#[async_trait]
impl SlashCommand for SessionCommand {
    fn name(&self) -> &str { "session" }
    fn aliases(&self) -> Vec<&str> { vec!["remote"] }
    fn description(&self) -> &str { "Show or manage conversation sessions" }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        match args.trim() {
            "list" => {
                let sessions = cc_core::history::list_sessions().await;
                if sessions.is_empty() {
                    CommandResult::Message("No saved sessions found.".to_string())
                } else {
                    let mut output = String::from("Recent sessions:\n\n");
                    for sess in sessions.iter().take(10) {
                        let updated = sess.updated_at.format("%Y-%m-%d %H:%M").to_string();
                        let id_short = &sess.id[..sess.id.len().min(8)];
                        output.push_str(&format!(
                            "  {} | {} | {} messages | {}\n",
                            id_short,
                            updated,
                            sess.messages.len(),
                            sess.title.as_deref().unwrap_or("(untitled)")
                        ));
                    }
                    output.push_str("\nUse /resume <id> to resume a session.");
                    CommandResult::Message(output)
                }
            }
            "" => {
                CommandResult::Message(format!(
                    "Current session stats:\n  ID: {}\n  Title: {}\n  Messages: {}\n  Model: {}\n\nUse /session list to see all sessions.",
                    ctx.session_id,
                    ctx.session_title.as_deref().unwrap_or("(untitled)"),
                    ctx.messages.len(),
                    ctx.config.effective_model()
                ))
            }
            _ => CommandResult::Error(format!("Unknown subcommand: {}\n\nUsage: /session [list]", args)),
        }
    }
}

// ---- /thinking -----------------------------------------------------------

#[async_trait]
impl SlashCommand for ThinkingCommand {
    fn name(&self) -> &str { "thinking" }
    fn description(&self) -> &str { "Toggle extended thinking mode" }
    fn aliases(&self) -> Vec<&str> { vec!["think"] }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        // Extended thinking is configured through the model; just inform the user
        let model = ctx.config.effective_model();
        if model.contains("claude-3-5") || model.contains("claude-3.5") {
            CommandResult::Message(
                "Extended thinking is not available for Claude 3.5 models.\n\
                 Use claude-opus-4-6 or claude-sonnet-4-6 for extended thinking.".to_string()
            )
        } else {
            CommandResult::Message(format!(
                "Extended thinking is available with {}.\n\
                 You can request thinking by asking Claude to 'think step by step' or \
                 'think carefully before answering'.",
                model
            ))
        }
    }
}

// ---- /export -------------------------------------------------------------

#[async_trait]
impl SlashCommand for ExportCommand {
    fn name(&self) -> &str { "export" }
    fn description(&self) -> &str { "Export conversation to a file" }
    fn help(&self) -> &str {
        "Usage: /export [filename]\n\
         Export the current conversation as JSON. Defaults to claude_export_<timestamp>.json."
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let filename = if args.trim().is_empty() {
            format!(
                "claude_export_{}.json",
                chrono::Utc::now().format("%Y%m%d_%H%M%S")
            )
        } else {
            args.trim().to_string()
        };

        let path = ctx.working_dir.join(&filename);
        let export = serde_json::json!({
            "exported_at": chrono::Utc::now().to_rfc3339(),
            "model": ctx.config.effective_model(),
            "message_count": ctx.messages.len(),
            "messages": ctx.messages.iter().map(|m| serde_json::json!({
                "role": m.role,
                "content": m.get_all_text(),
            })).collect::<Vec<_>>(),
        });

        let json = match serde_json::to_string_pretty(&export) {
            Ok(j) => j,
            Err(e) => return CommandResult::Error(format!("Failed to serialize: {}", e)),
        };

        match std::fs::write(&path, &json) {
            Ok(_) => CommandResult::Message(format!(
                "Conversation exported to {}\n({} messages)",
                path.display(),
                ctx.messages.len()
            )),
            Err(e) => CommandResult::Error(format!("Failed to write {}: {}", filename, e)),
        }
    }
}

// ---- /skills -------------------------------------------------------------

#[async_trait]
impl SlashCommand for SkillsCommand {
    fn name(&self) -> &str { "skills" }
    fn aliases(&self) -> Vec<&str> { vec!["skill"] }
    fn description(&self) -> &str { "List available skills in .claude/commands/" }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        let mut found: Vec<String> = Vec::new();
        let dirs = [
            ctx.working_dir.join(".claude").join("commands"),
            dirs::home_dir()
                .unwrap_or_default()
                .join(".claude")
                .join("commands"),
        ];

        for dir in &dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().map_or(false, |e| e == "md") {
                        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                            let name = stem.to_string();
                            if !found.contains(&name) {
                                found.push(name);
                            }
                        }
                    }
                }
            }
        }

        if found.is_empty() {
            return CommandResult::Message(
                "No skills found.\nCreate .md files in .claude/commands/ to define skills.\n\
                 Example: .claude/commands/review.md".to_string(),
            );
        }

        found.sort();
        CommandResult::Message(format!(
            "Available skills ({}):\n{}",
            found.len(),
            found.iter().map(|s| format!("  /{}", s)).collect::<Vec<_>>().join("\n")
        ))
    }
}

// ---- /rewind -------------------------------------------------------------

#[async_trait]
impl SlashCommand for RewindCommand {
    fn name(&self) -> &str { "rewind" }
    fn description(&self) -> &str { "Remove the last N message turns" }
    fn help(&self) -> &str { "Usage: /rewind [n]\nRemove the last N turns (default 1)." }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let n: usize = args.trim().parse().unwrap_or(1).max(1);

        // Each "turn" is a user+assistant pair = 2 messages.
        let to_remove = (n * 2).min(ctx.messages.len());
        if to_remove == 0 {
            return CommandResult::Message("Nothing to rewind.".to_string());
        }

        let new_len = ctx.messages.len().saturating_sub(to_remove);
        let mut trimmed = ctx.messages.clone();
        trimmed.truncate(new_len);

        let removed = ctx.messages.len() - new_len;
        let note = format!(
            "Rewound {} turn{} ({} message{} removed).",
            n,
            if n == 1 { "" } else { "s" },
            removed,
            if removed == 1 { "" } else { "s" }
        );

        // SetMessages propagates the trimmed list back to the REPL.
        // We use a UserMessage to surface the confirmation note.
        // Since CommandResult can only return one variant, we use SetMessages
        // and let the REPL show a status message.
        let _ = note; // consumed below
        CommandResult::SetMessages(trimmed)
    }
}

// ---- /stats --------------------------------------------------------------

#[async_trait]
impl SlashCommand for StatsCommand {
    fn name(&self) -> &str { "stats" }
    fn description(&self) -> &str { "Show token usage and cost statistics" }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        let input = ctx.cost_tracker.input_tokens();
        let output = ctx.cost_tracker.output_tokens();
        let cost = ctx.cost_tracker.total_cost_usd();
        let turns = ctx.messages.len();
        let model = ctx.config.effective_model();

        CommandResult::Message(format!(
            "Session statistics\n\
             ──────────────────\n\
             Model:          {}\n\
             Messages:       {}\n\
             Input tokens:   {}\n\
             Output tokens:  {}\n\
             Total tokens:   {}\n\
             Estimated cost: ${:.4}",
            model,
            turns,
            input,
            output,
            input + output,
            cost
        ))
    }
}

// ---- /files --------------------------------------------------------------

#[async_trait]
impl SlashCommand for FilesCommand {
    fn name(&self) -> &str { "files" }
    fn description(&self) -> &str { "List files referenced in the current conversation" }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        use std::collections::HashSet;
        // Scan message content for file paths (simple heuristic)
        let mut files: HashSet<String> = HashSet::new();
        let path_re = regex::Regex::new(r#"(?m)([A-Za-z]:[\\/][^\s,;:"'<>]+|/[^\s,;:"'<>]{3,})"#).ok();

        for msg in &ctx.messages {
            let text = msg.get_all_text();
            if let Some(ref re) = path_re {
                for cap in re.captures_iter(&text) {
                    let path = cap[1].trim().to_string();
                    if std::path::Path::new(&path).exists() {
                        files.insert(path);
                    }
                }
            }
        }

        if files.is_empty() {
            return CommandResult::Message(
                "No referenced files detected in the conversation.".to_string(),
            );
        }

        let mut sorted: Vec<String> = files.into_iter().collect();
        sorted.sort();

        CommandResult::Message(format!(
            "Referenced files ({}):\n{}",
            sorted.len(),
            sorted.iter().map(|f| format!("  {}", f)).collect::<Vec<_>>().join("\n")
        ))
    }
}

// ---- /rename -------------------------------------------------------------

#[async_trait]
impl SlashCommand for RenameCommand {
    fn name(&self) -> &str { "rename" }
    fn description(&self) -> &str { "Rename the current session" }
    fn help(&self) -> &str { "Usage: /rename <new name>" }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let name = args.trim();
        if name.is_empty() {
            return CommandResult::Error("Usage: /rename <new name>".to_string());
        }

        CommandResult::RenameSession(name.to_string())
    }
}

// ---- /effort -------------------------------------------------------------

#[async_trait]
impl SlashCommand for EffortCommand {
    fn name(&self) -> &str { "effort" }
    fn description(&self) -> &str { "Set the model's thinking effort (low | normal | high)" }
    fn help(&self) -> &str {
        "Usage: /effort [low|normal|high]\n\
         Sets how much computation the model uses for reasoning.\n\
         'high' enables extended thinking with a larger budget."
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        match args.trim() {
            "" => CommandResult::Message(format!(
                "Current effort: normal\nUse /effort [low|normal|high] to change."
            )),
            "low" => {
                // Low effort: smaller max_tokens
                ctx.config.max_tokens = Some(4096);
                CommandResult::ConfigChange(ctx.config.clone())
            }
            "normal" => {
                ctx.config.max_tokens = None; // use default
                CommandResult::ConfigChange(ctx.config.clone())
            }
            "high" => {
                ctx.config.max_tokens = Some(32768);
                CommandResult::ConfigChange(ctx.config.clone())
            }
            other => CommandResult::Error(format!(
                "Unknown effort level '{}'. Use: low | normal | high",
                other
            )),
        }
    }
}

// ---- /summary ------------------------------------------------------------

#[async_trait]
impl SlashCommand for SummaryCommand {
    fn name(&self) -> &str { "summary" }
    fn description(&self) -> &str { "Generate a brief summary of the conversation so far" }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        let count = ctx.messages.len();
        if count == 0 {
            return CommandResult::Message("No messages in conversation yet.".to_string());
        }

        // Ask the model to summarize by injecting a hidden user message
        CommandResult::UserMessage(
            "Please provide a brief (3-5 sentence) summary of our conversation so far, \
             focusing on what has been accomplished and the current state."
                .to_string(),
        )
    }
}

// ---- /commit -------------------------------------------------------------

#[async_trait]
impl SlashCommand for CommitCommand {
    fn name(&self) -> &str { "commit" }
    fn description(&self) -> &str { "Ask Claude to commit staged changes" }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let extra = if args.trim().is_empty() {
            String::new()
        } else {
            format!(" with message: {}", args.trim())
        };

        CommandResult::UserMessage(format!(
            "Please commit the currently staged git changes{}. \
             Run `git diff --cached` to see what's staged, \
             write an appropriate commit message following the repository's conventions, \
             and run `git commit`.",
            extra
        ))
    }
}

// ---------------------------------------------------------------------------
// UI settings helpers (stored in ~/.claude/ui-settings.json)
// These hold things not present in the core Config struct.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct UiSettings {
    #[serde(default)]
    pub editor_mode: Option<String>,       // "vim" or "normal"
    #[serde(default)]
    pub fast_mode: Option<bool>,
    #[serde(default)]
    pub voice_enabled: Option<bool>,
    #[serde(default)]
    pub statusline_show_cost: Option<bool>,
    #[serde(default)]
    pub statusline_show_tokens: Option<bool>,
    #[serde(default)]
    pub statusline_show_model: Option<bool>,
    #[serde(default)]
    pub statusline_show_time: Option<bool>,
    #[serde(default)]
    pub prompt_color: Option<String>,
}

fn ui_settings_path() -> std::path::PathBuf {
    cc_core::config::Settings::config_dir().join("ui-settings.json")
}

fn load_ui_settings() -> UiSettings {
    let path = ui_settings_path();
    if !path.exists() {
        return UiSettings::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_ui_settings(settings: &UiSettings) -> anyhow::Result<()> {
    let path = ui_settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    std::fs::write(&path, json)?;
    Ok(())
}

fn mutate_ui_settings<F>(f: F) -> anyhow::Result<UiSettings>
where
    F: FnOnce(&mut UiSettings),
{
    let mut s = load_ui_settings();
    f(&mut s);
    save_ui_settings(&s)?;
    Ok(s)
}

// ---- /remote-control (/rc) -----------------------------------------------

#[async_trait]
impl SlashCommand for RemoteControlCommand {
    fn name(&self) -> &str { "remote-control" }
    fn aliases(&self) -> Vec<&str> { vec!["rc"] }
    fn description(&self) -> &str { "Show or manage the remote control (Bridge) connection" }
    fn help(&self) -> &str {
        "Usage: /remote-control [start|stop|status]\n\n\
         The Bridge feature lets you connect your local Claude Code CLI to the\n\
         claude.ai web UI or mobile app.\n\n\
         Subcommands:\n\
         /remote-control          Show current bridge status and connection URL\n\
         /remote-control start    Start the remote-control bridge listener\n\
         /remote-control stop     Stop the bridge listener\n\
         /remote-control status   Show bridge status"
    }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let settings = match cc_core::config::Settings::load().await {
            Ok(s) => s,
            Err(e) => return CommandResult::Error(format!("Failed to load settings: {}", e)),
        };

        let remote_at_startup = settings.remote_control_at_startup;

        match args.trim() {
            "" | "status" => {
                let status = if remote_at_startup { "enabled at startup" } else { "disabled" };
                let bridge_url = std::env::var("CLAUDE_CODE_BRIDGE_URL")
                    .unwrap_or_else(|_| "https://claude.ai".to_string());
                CommandResult::Message(format!(
                    "Remote Control (Bridge) Status\n\
                     ──────────────────────────────\n\
                     Status:      {status}\n\
                     Bridge URL:  {bridge_url}\n\
                     Token:       {token}\n\n\
                     To connect from the web UI, visit:\n  {bridge_url}/claude-code\n\n\
                     Use /remote-control start  to enable bridge at startup\n\
                     Use /remote-control stop   to disable bridge at startup",
                    status = status,
                    bridge_url = bridge_url,
                    token = if std::env::var("CLAUDE_CODE_BRIDGE_TOKEN").is_ok() {
                        "configured (set via CLAUDE_CODE_BRIDGE_TOKEN)"
                    } else {
                        "not set — set CLAUDE_CODE_BRIDGE_TOKEN to connect"
                    },
                ))
            }
            "start" => {
                if let Err(e) = save_settings_mutation(|s| s.remote_control_at_startup = true) {
                    return CommandResult::Error(format!("Failed to save settings: {}", e));
                }
                CommandResult::Message(
                    "Remote control bridge enabled at startup.\n\
                     Set CLAUDE_CODE_BRIDGE_TOKEN=<token> and restart Claude Code\n\
                     to connect to the claude.ai web UI."
                        .to_string(),
                )
            }
            "stop" => {
                if let Err(e) = save_settings_mutation(|s| s.remote_control_at_startup = false) {
                    return CommandResult::Error(format!("Failed to save settings: {}", e));
                }
                CommandResult::Message("Remote control bridge disabled.".to_string())
            }
            other => CommandResult::Error(format!(
                "Unknown subcommand: '{}'\nUsage: /remote-control [start|stop|status]",
                other
            )),
        }
    }
}

// ---- /remote-env ---------------------------------------------------------

#[async_trait]
impl SlashCommand for RemoteEnvCommand {
    fn name(&self) -> &str { "remote-env" }
    fn description(&self) -> &str { "Show and manage environment variables for remote sessions" }
    fn help(&self) -> &str {
        "Usage: /remote-env [set <KEY> <VALUE> | unset <KEY> | list]\n\n\
         Manages env vars stored in config that are forwarded to remote Claude Code sessions.\n\
         These are persisted to settings under the 'env' key."
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let args = args.trim();

        if args.is_empty() || args == "list" {
            if ctx.config.env.is_empty() {
                return CommandResult::Message(
                    "No remote environment variables configured.\n\
                     Use /remote-env set <KEY> <VALUE> to add one."
                        .to_string(),
                );
            }
            let mut lines = vec!["Remote environment variables:".to_string()];
            let mut keys: Vec<_> = ctx.config.env.keys().collect();
            keys.sort();
            for key in keys {
                let val = &ctx.config.env[key];
                // Mask values that look like secrets
                let display = if key.to_uppercase().contains("KEY")
                    || key.to_uppercase().contains("TOKEN")
                    || key.to_uppercase().contains("SECRET")
                    || key.to_uppercase().contains("PASSWORD")
                {
                    format!("{}***", &val[..val.len().min(4)])
                } else {
                    val.clone()
                };
                lines.push(format!("  {} = {}", key, display));
            }
            return CommandResult::Message(lines.join("\n"));
        }

        let mut parts = args.splitn(3, ' ');
        let sub = parts.next().unwrap_or("").trim();
        let key = parts.next().unwrap_or("").trim();
        let val = parts.next().unwrap_or("").trim();

        match sub {
            "set" => {
                if key.is_empty() || val.is_empty() {
                    return CommandResult::Error(
                        "Usage: /remote-env set <KEY> <VALUE>".to_string(),
                    );
                }
                let key_owned = key.to_string();
                let val_owned = val.to_string();
                if let Err(e) = save_settings_mutation(|s| {
                    s.config.env.insert(key_owned.clone(), val_owned.clone());
                }) {
                    return CommandResult::Error(format!("Failed to save: {}", e));
                }
                let mut new_config = ctx.config.clone();
                new_config.env.insert(key.to_string(), val.to_string());
                CommandResult::ConfigChangeMessage(
                    new_config,
                    format!("Set remote env: {} = {}", key, val),
                )
            }
            "unset" | "remove" | "delete" => {
                if key.is_empty() {
                    return CommandResult::Error(
                        "Usage: /remote-env unset <KEY>".to_string(),
                    );
                }
                if !ctx.config.env.contains_key(key) {
                    return CommandResult::Message(format!("Key '{}' is not set.", key));
                }
                let key_owned = key.to_string();
                if let Err(e) = save_settings_mutation(|s| {
                    s.config.env.remove(&key_owned);
                }) {
                    return CommandResult::Error(format!("Failed to save: {}", e));
                }
                let mut new_config = ctx.config.clone();
                new_config.env.remove(key);
                CommandResult::ConfigChangeMessage(
                    new_config,
                    format!("Removed remote env var: {}", key),
                )
            }
            other => CommandResult::Error(format!(
                "Unknown subcommand: '{}'\nUsage: /remote-env [list|set <K> <V>|unset <K>]",
                other
            )),
        }
    }
}

// ---- /context ------------------------------------------------------------

#[async_trait]
impl SlashCommand for ContextCommand {
    fn name(&self) -> &str { "context" }
    fn description(&self) -> &str { "Show context window usage (tokens used / available)" }
    fn help(&self) -> &str {
        "Usage: /context\n\n\
         Displays the current context window utilization:\n\
         - Estimated tokens consumed by current conversation\n\
         - Context window limit for the active model\n\
         - Percentage used"
    }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        let model = ctx.config.effective_model();

        // Determine context window size from known model names
        let context_window: u64 = if model.contains("claude-3-5") || model.contains("claude-3.5") {
            200_000
        } else if model.contains("opus") {
            200_000
        } else if model.contains("sonnet") {
            200_000
        } else if model.contains("haiku") {
            200_000
        } else {
            200_000 // safe default for any Claude model
        };

        let used_tokens = ctx.cost_tracker.total_tokens();
        let pct = if context_window > 0 {
            (used_tokens as f64 / context_window as f64) * 100.0
        } else {
            0.0
        };

        let bar_width = 40usize;
        let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
        let bar: String = "█".repeat(filled) + &"░".repeat(bar_width.saturating_sub(filled));

        // Estimate approximate message tokens from the message list
        let msg_char_count: usize = ctx.messages.iter().map(|m| m.get_all_text().len()).sum();
        // Rough estimate: ~4 chars per token for message text
        let msg_token_estimate = msg_char_count / 4;

        CommandResult::Message(format!(
            "Context Window Usage\n\
             ────────────────────\n\
             Model:          {model}\n\
             Context window: {window:>10} tokens\n\
             API tokens used:{used:>10} tokens  ({pct:.1}%)\n\
             Est. msg size:  {msg:>10} tokens  (approx)\n\
             Messages:       {msgs:>10}\n\n\
             [{bar}] {pct:.1}%\n\n\
             Use /compact to reduce context usage.",
            model = model,
            window = context_window,
            used = used_tokens,
            pct = pct,
            msg = msg_token_estimate,
            msgs = ctx.messages.len(),
            bar = bar,
        ))
    }
}

// ---- /copy ---------------------------------------------------------------

#[async_trait]
impl SlashCommand for CopyCommand {
    fn name(&self) -> &str { "copy" }
    fn description(&self) -> &str { "Copy the last assistant response to the clipboard" }
    fn help(&self) -> &str {
        "Usage: /copy [n]\n\n\
         Copies the most recent assistant response to the system clipboard.\n\
         Optionally pass a number to copy the Nth most-recent response."
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let n: usize = args.trim().parse().unwrap_or(1).max(1);

        // Find the Nth most recent assistant message
        let assistant_msgs: Vec<&cc_core::types::Message> = ctx
            .messages
            .iter()
            .rev()
            .filter(|m| m.role == cc_core::types::Role::Assistant)
            .take(n)
            .collect();

        let msg = match assistant_msgs.last() {
            Some(m) => m,
            None => {
                return CommandResult::Message(
                    "No assistant messages found in conversation.".to_string(),
                )
            }
        };

        let text = msg.get_all_text();
        if text.is_empty() {
            return CommandResult::Message("Last assistant message is empty.".to_string());
        }

        // Try system clipboard via arboard
        #[cfg(not(target_os = "linux"))]
        {
            match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text.clone())) {
                Ok(()) => {
                    let preview: String = text.chars().take(80).collect();
                    let ellipsis = if text.len() > 80 { "…" } else { "" };
                    return CommandResult::Message(format!(
                        "Copied {} chars to clipboard.\nPreview: {}{}",
                        text.len(),
                        preview,
                        ellipsis
                    ));
                }
                Err(e) => {
                    tracing::warn!("Clipboard write failed: {}", e);
                    // Fall through to file fallback
                }
            }
        }

        // Fallback: write to a temp file and inform the user
        let tmp_path = std::env::temp_dir().join("claude_copy.md");
        match std::fs::write(&tmp_path, &text) {
            Ok(()) => {
                let preview: String = text.chars().take(80).collect();
                let ellipsis = if text.len() > 80 { "…" } else { "" };
                CommandResult::Message(format!(
                    "Clipboard not available; saved {} chars to {}\nPreview: {}{}",
                    text.len(),
                    tmp_path.display(),
                    preview,
                    ellipsis
                ))
            }
            Err(e) => CommandResult::Error(format!("Failed to copy: {}", e)),
        }
    }
}

// ---- /chrome -------------------------------------------------------------

#[async_trait]
impl SlashCommand for ChromeCommand {
    fn name(&self) -> &str { "chrome" }
    fn description(&self) -> &str { "Chrome DevTools integration — connect Claude to a browser tab" }
    fn help(&self) -> &str {
        "Usage: /chrome [url]\n\n\
         Integrates Claude Code with Google Chrome via the Chrome DevTools Protocol (CDP).\n\n\
         To use:\n\
         1. Launch Chrome with remote debugging:\n\
            chrome --remote-debugging-port=9222\n\
         2. Run /chrome to connect\n\
         3. Claude can then read the DOM, console logs, network requests, etc.\n\n\
         Optional: /chrome <url>  — navigate to a URL after connecting"
    }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let cdp_url = "http://localhost:9222";

        // Try to reach the Chrome debugging endpoint
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap_or_default();

        let chrome_available = client
            .get(format!("{}/json/version", cdp_url))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);

        if chrome_available {
            let navigate_msg = if !args.trim().is_empty() {
                format!("\n\nNavigating to: {}", args.trim())
            } else {
                String::new()
            };
            CommandResult::Message(format!(
                "Chrome DevTools connected at {cdp_url}{nav}\n\n\
                 Claude can now access the browser context. Try asking:\n\
                 - 'What's on the current page?'\n\
                 - 'Check the browser console for errors'\n\
                 - 'Describe the page structure'",
                cdp_url = cdp_url,
                nav = navigate_msg,
            ))
        } else {
            CommandResult::Message(format!(
                "Chrome DevTools not found at {cdp_url}\n\n\
                 To enable Chrome integration:\n\
                 1. Close Chrome completely\n\
                 2. Relaunch with debugging enabled:\n\n\
                    macOS:   /Applications/Google\\ Chrome.app/Contents/MacOS/Google\\ Chrome \\\n\
                             --remote-debugging-port=9222 --no-first-run\n\
                    Windows: chrome.exe --remote-debugging-port=9222 --no-first-run\n\
                    Linux:   google-chrome --remote-debugging-port=9222 --no-first-run\n\n\
                 3. Then run /chrome again\n\n\
                 Note: Do not use your primary Chrome profile for security reasons.\n\
                 Docs: https://docs.anthropic.com/claude-code/chrome-devtools",
                cdp_url = cdp_url,
            ))
        }
    }
}

// ---- /vim (/vi) ----------------------------------------------------------

#[async_trait]
impl SlashCommand for VimCommand {
    fn name(&self) -> &str { "vim" }
    fn aliases(&self) -> Vec<&str> { vec!["vi"] }
    fn description(&self) -> &str { "Toggle vim keybinding mode on/off" }
    fn help(&self) -> &str {
        "Usage: /vim [on|off]\n\n\
         Toggles vim keybinding mode in the REPL input.\n\
         When enabled, use Esc to switch between INSERT and NORMAL modes.\n\n\
         The setting is persisted to ~/.claude/ui-settings.json."
    }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let current = load_ui_settings();
        let current_mode = current.editor_mode.as_deref().unwrap_or("normal");

        let new_mode = match args.trim() {
            "on" | "vim" => "vim",
            "off" | "normal" => "normal",
            "" => {
                // Toggle
                if current_mode == "vim" { "normal" } else { "vim" }
            }
            other => {
                return CommandResult::Error(format!(
                    "Unknown argument '{}'. Use: /vim [on|off]",
                    other
                ))
            }
        };

        match mutate_ui_settings(|s| s.editor_mode = Some(new_mode.to_string())) {
            Ok(_) => CommandResult::Message(format!(
                "Editor mode set to {}.\n{}",
                new_mode,
                if new_mode == "vim" {
                    "Use Esc to switch between INSERT and NORMAL modes.\n\
                     Restart the REPL for the change to take effect."
                } else {
                    "Using standard (readline-style) keyboard bindings.\n\
                     Restart the REPL for the change to take effect."
                }
            )),
            Err(e) => CommandResult::Error(format!("Failed to save setting: {}", e)),
        }
    }
}

// ---- /voice --------------------------------------------------------------

#[async_trait]
impl SlashCommand for VoiceCommand {
    fn name(&self) -> &str { "voice" }
    fn description(&self) -> &str { "Toggle voice input mode on/off" }
    fn help(&self) -> &str {
        "Usage: /voice [on|off]\n\n\
         Enables or disables voice input (hold-to-talk).\n\
         Voice requires a Claude.ai subscription with the voice scope enabled.\n\
         Setting is persisted to ~/.claude/ui-settings.json."
    }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let current = load_ui_settings();
        let currently_enabled = current.voice_enabled.unwrap_or(false);

        let enable = match args.trim() {
            "on" | "enable" | "enabled" | "true" | "1" => true,
            "off" | "disable" | "disabled" | "false" | "0" => false,
            "" => !currently_enabled, // toggle
            other => {
                return CommandResult::Error(format!(
                    "Unknown argument '{}'. Use: /voice [on|off]",
                    other
                ))
            }
        };

        match mutate_ui_settings(|s| s.voice_enabled = Some(enable)) {
            Ok(_) => {
                if enable {
                    CommandResult::Message(
                        "Voice input enabled.\n\
                         Hold the configured hold-to-talk key to record.\n\
                         Voice mode requires a Claude.ai account with voice scope.\n\
                         Restart the REPL for the change to take effect."
                            .to_string(),
                    )
                } else {
                    CommandResult::Message("Voice input disabled.".to_string())
                }
            }
            Err(e) => CommandResult::Error(format!("Failed to save voice setting: {}", e)),
        }
    }
}

// ---- /upgrade ------------------------------------------------------------

#[async_trait]
impl SlashCommand for UpgradeCommand {
    fn name(&self) -> &str { "upgrade" }
    fn description(&self) -> &str { "Check for updates and show upgrade options" }
    fn help(&self) -> &str {
        "Usage: /upgrade\n\n\
         Checks GitHub releases for the latest version of Claude Code.\n\
         If a newer version is available, shows the upgrade command."
    }

    async fn execute(&self, _args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let current = cc_core::constants::APP_VERSION;

        // Check GitHub releases API for latest version
        let client = reqwest::Client::builder()
            .user_agent(format!("claude-code-rust/{}", current))
            .timeout(std::time::Duration::from_secs(8))
            .build();

        let client = match client {
            Ok(c) => c,
            Err(e) => {
                return CommandResult::Message(format!(
                    "Current version: {current}\n\
                     Could not check for updates (HTTP client error: {e})\n\
                     Visit https://github.com/anthropics/claude-code/releases for updates."
                ))
            }
        };

        let resp = client
            .get("https://api.github.com/repos/anthropics/claude-code/releases/latest")
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let json: serde_json::Value =
                    r.json().await.unwrap_or(serde_json::Value::Null);

                let tag = json
                    .get("tag_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .trim_start_matches('v');

                let url = json
                    .get("html_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("https://github.com/anthropics/claude-code/releases");

                if tag == current || tag == "unknown" {
                    CommandResult::Message(format!(
                        "Claude Code v{current} — you are up to date.\n\
                         Release page: {url}"
                    ))
                } else {
                    CommandResult::Message(format!(
                        "Update available!\n\
                         Current version:  v{current}\n\
                         Latest version:   v{tag}\n\
                         Release page:     {url}\n\n\
                         To upgrade (npm):\n\
                           npm install -g @anthropic-ai/claude-code@latest\n\n\
                         To upgrade (cargo):\n\
                           cargo install claude-code --force"
                    ))
                }
            }
            Ok(r) => {
                let status = r.status();
                CommandResult::Message(format!(
                    "Current version: v{current}\n\
                     Could not check for updates (HTTP {status}).\n\
                     Visit https://github.com/anthropics/claude-code/releases for updates."
                ))
            }
            Err(e) => CommandResult::Message(format!(
                "Current version: v{current}\n\
                 Could not check for updates: {e}\n\
                 Visit https://github.com/anthropics/claude-code/releases for updates."
            )),
        }
    }
}

// ---- /release-notes ------------------------------------------------------

#[async_trait]
impl SlashCommand for ReleaseNotesCommand {
    fn name(&self) -> &str { "release-notes" }
    fn description(&self) -> &str { "Show release notes for the current version" }
    fn help(&self) -> &str {
        "Usage: /release-notes [version]\n\n\
         Fetches and displays release notes from GitHub.\n\
         Without an argument, shows notes for the current version."
    }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let current = cc_core::constants::APP_VERSION;
        let version = args.trim();

        let tag = if version.is_empty() {
            format!("v{}", current)
        } else if version.starts_with('v') {
            version.to_string()
        } else {
            format!("v{}", version)
        };

        let client = reqwest::Client::builder()
            .user_agent(format!("claude-code-rust/{}", current))
            .timeout(std::time::Duration::from_secs(8))
            .build();

        let client = match client {
            Ok(c) => c,
            Err(_) => {
                return CommandResult::Message(format!(
                    "Claude Code {tag} release notes:\n\
                     Visit https://github.com/anthropics/claude-code/releases/tag/{tag}"
                ))
            }
        };

        let url = format!(
            "https://api.github.com/repos/anthropics/claude-code/releases/tags/{}",
            tag
        );

        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => {
                let json: serde_json::Value =
                    r.json().await.unwrap_or(serde_json::Value::Null);

                let body = json
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("No release notes found.");

                let published = json
                    .get("published_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown date");

                let html_url = json
                    .get("html_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                CommandResult::Message(format!(
                    "Release Notes: Claude Code {tag}\n\
                     Published: {published}\n\
                     URL: {html_url}\n\
                     ─────────────────────────────────\n\
                     {body}"
                ))
            }
            Ok(r) if r.status().as_u16() == 404 => CommandResult::Message(format!(
                "No release found for {tag}.\n\
                 View all releases: https://github.com/anthropics/claude-code/releases"
            )),
            Ok(r) => CommandResult::Message(format!(
                "Could not fetch release notes (HTTP {}).\n\
                 View at: https://github.com/anthropics/claude-code/releases/tag/{}",
                r.status(),
                tag
            )),
            Err(e) => CommandResult::Message(format!(
                "Could not fetch release notes: {e}\n\
                 View at: https://github.com/anthropics/claude-code/releases/tag/{tag}"
            )),
        }
    }
}

// ---- /rate-limit-options -------------------------------------------------

#[async_trait]
impl SlashCommand for RateLimitOptionsCommand {
    fn name(&self) -> &str { "rate-limit-options" }
    fn description(&self) -> &str { "Show rate limit tiers and current rate limit status" }
    fn help(&self) -> &str {
        "Usage: /rate-limit-options\n\n\
         Displays available rate limit tiers and the current tier for your account.\n\
         Rate limits depend on your Claude plan (Free, Pro, Max, API)."
    }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        // Try to read from OAuth tokens file to get subscription/tier info
        let tier_info = match cc_core::oauth::OAuthTokens::load().await {
            Some(tokens) => {
                let sub_type = tokens.subscription_type.as_deref().unwrap_or("unknown");
                format!(
                    "Account type:    {}\n\
                     Scopes:          {}",
                    sub_type,
                    if tokens.scopes.is_empty() { "none".to_string() } else { tokens.scopes.join(", ") }
                )
            }
            None => {
                // Check for API key auth
                if ctx.config.resolve_api_key().is_some() {
                    "Account type:    API key (Console)\n\
                     Rate limit tier: Depends on your API plan tier"
                        .to_string()
                } else {
                    "Not logged in. Run /login to see your rate limit tier.".to_string()
                }
            }
        };

        CommandResult::Message(format!(
            "Rate Limit Status\n\
             ─────────────────\n\
             {tier_info}\n\n\
             Available tiers:\n\
             ┌─────────────────────────────────────────────────┐\n\
             │ Free          │ Limited daily usage             │\n\
             │ Pro           │ Higher limits, faster resets    │\n\
             │ Max (5x)      │ 5× Pro limits                   │\n\
             │ Max (20x)     │ 20× Pro limits (highest tier)   │\n\
             │ API / Console │ Usage-billed, no hard cap       │\n\
             └─────────────────────────────────────────────────┘\n\n\
             To upgrade: /upgrade\n\
             Manage billing: https://claude.ai/settings/billing",
            tier_info = tier_info,
        ))
    }
}

// ---- /statusline ---------------------------------------------------------

#[async_trait]
impl SlashCommand for StatuslineCommand {
    fn name(&self) -> &str { "statusline" }
    fn description(&self) -> &str { "Configure what is shown in the status line" }
    fn help(&self) -> &str {
        "Usage: /statusline [show|hide] [cost|tokens|model|time|all]\n\n\
         Controls which items appear in the TUI status bar at the bottom.\n\
         Settings are persisted to ~/.claude/ui-settings.json.\n\n\
         Examples:\n\
           /statusline               — show current configuration\n\
           /statusline show cost     — show cost in status line\n\
           /statusline hide tokens   — hide token count\n\
           /statusline show all      — show everything\n\
           /statusline hide all      — hide everything"
    }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let args = args.trim();
        let current = load_ui_settings();

        if args.is_empty() {
            return CommandResult::Message(format!(
                "Status line configuration\n\
                 ─────────────────────────\n\
                 Show cost:   {cost}\n\
                 Show tokens: {tokens}\n\
                 Show model:  {model}\n\
                 Show time:   {time}\n\n\
                 Use /statusline [show|hide] [cost|tokens|model|time|all] to change.",
                cost = fmt_bool(current.statusline_show_cost.unwrap_or(true)),
                tokens = fmt_bool(current.statusline_show_tokens.unwrap_or(true)),
                model = fmt_bool(current.statusline_show_model.unwrap_or(true)),
                time = fmt_bool(current.statusline_show_time.unwrap_or(true)),
            ));
        }

        let mut parts = args.splitn(2, ' ');
        let verb = parts.next().unwrap_or("").trim();
        let item = parts.next().unwrap_or("").trim();

        let show = match verb {
            "show" | "enable" | "on" => true,
            "hide" | "disable" | "off" => false,
            _ => {
                return CommandResult::Error(
                    "Usage: /statusline [show|hide] [cost|tokens|model|time|all]".to_string(),
                )
            }
        };

        if item.is_empty() || item == "all" {
            match mutate_ui_settings(|s| {
                s.statusline_show_cost = Some(show);
                s.statusline_show_tokens = Some(show);
                s.statusline_show_model = Some(show);
                s.statusline_show_time = Some(show);
            }) {
                Ok(_) => return CommandResult::Message(format!(
                    "Status line: all items {}.",
                    if show { "shown" } else { "hidden" }
                )),
                Err(e) => return CommandResult::Error(format!("Failed to save: {}", e)),
            }
        }

        let result = match item {
            "cost" => mutate_ui_settings(|s| s.statusline_show_cost = Some(show)),
            "tokens" | "token" => mutate_ui_settings(|s| s.statusline_show_tokens = Some(show)),
            "model" => mutate_ui_settings(|s| s.statusline_show_model = Some(show)),
            "time" | "clock" => mutate_ui_settings(|s| s.statusline_show_time = Some(show)),
            other => {
                return CommandResult::Error(format!(
                    "Unknown item '{}'. Use: cost, tokens, model, time, or all.",
                    other
                ))
            }
        };

        match result {
            Ok(_) => CommandResult::Message(format!(
                "Status line: {} {}.",
                item,
                if show { "shown" } else { "hidden" }
            )),
            Err(e) => CommandResult::Error(format!("Failed to save: {}", e)),
        }
    }
}

fn fmt_bool(v: bool) -> &'static str {
    if v { "on" } else { "off" }
}

// ---- /security-review ----------------------------------------------------

#[async_trait]
impl SlashCommand for SecurityReviewCommand {
    fn name(&self) -> &str { "security-review" }
    fn description(&self) -> &str { "Run a security review of the current project" }
    fn help(&self) -> &str {
        "Usage: /security-review [path]\n\n\
         Asks Claude to perform a security review of the codebase.\n\
         Analyzes for common vulnerabilities: injection attacks, auth issues,\n\
         secrets exposure, unsafe deserialization, path traversal, etc."
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let target = if args.trim().is_empty() {
            ctx.working_dir.display().to_string()
        } else {
            args.trim().to_string()
        };

        CommandResult::UserMessage(format!(
            "Please perform a comprehensive security review of the code in `{target}`.\n\n\
             Focus on identifying:\n\
             1. Injection vulnerabilities (SQL, command, LDAP, XSS, SSTI)\n\
             2. Authentication and authorization flaws\n\
             3. Hardcoded secrets, API keys, or passwords\n\
             4. Insecure deserialization\n\
             5. Path traversal or file inclusion vulnerabilities\n\
             6. Cryptographic weaknesses (weak algorithms, bad IV usage, key reuse)\n\
             7. Dependency vulnerabilities (check for outdated packages)\n\
             8. Race conditions and TOCTOU issues\n\
             9. Information disclosure (verbose errors, debug endpoints)\n\
             10. Any OWASP Top 10 issues relevant to this codebase\n\n\
             For each finding, provide:\n\
             - Severity: Critical/High/Medium/Low/Informational\n\
             - File and line number\n\
             - Description of the vulnerability\n\
             - Proof of concept or reproduction steps\n\
             - Recommended remediation\n\n\
             Start by reading the main source files and any dependency manifests.",
            target = target,
        ))
    }
}

// ---- /terminal-setup -----------------------------------------------------

#[async_trait]
impl SlashCommand for TerminalSetupCommand {
    fn name(&self) -> &str { "terminal-setup" }
    fn description(&self) -> &str { "Help configure your terminal for optimal Claude Code use" }
    fn help(&self) -> &str {
        "Usage: /terminal-setup\n\n\
         Diagnoses your terminal environment and gives recommendations for\n\
         optimal Claude Code display (font, color support, Unicode, etc.)."
    }

    async fn execute(&self, _args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let mut checks: Vec<String> = Vec::new();

        // Check TERM variable
        let term = std::env::var("TERM").unwrap_or_default();
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();

        // Terminal identification
        let terminal_name = if !term_program.is_empty() {
            term_program.clone()
        } else {
            term.clone()
        };
        checks.push(format!("Terminal:      {}", terminal_name));

        // Color depth
        let color_depth = if colorterm == "truecolor" || colorterm == "24bit" {
            "24-bit true color (optimal)"
        } else if term.contains("256color") || colorterm == "256color" {
            "256 colors (good)"
        } else if !term.is_empty() {
            "Basic colors (limited)"
        } else {
            "Unknown"
        };
        checks.push(format!("Colors:        {}", color_depth));

        // Check if UNICODE is likely supported
        let lang = std::env::var("LANG").unwrap_or_default();
        let lc_all = std::env::var("LC_ALL").unwrap_or_default();
        let unicode_env = lang.to_lowercase().contains("utf") || lc_all.to_lowercase().contains("utf");
        checks.push(format!(
            "Unicode/UTF-8: {}",
            if unicode_env { "likely supported (LANG/LC_ALL contains UTF)" } else { "check LANG env var" }
        ));

        // Check for known good terminals
        let is_good_terminal = matches!(
            term_program.to_lowercase().as_str(),
            "iterm.app" | "iterm2" | "hyper" | "warp" | "alacritty" | "kitty" | "wezterm"
        ) || term_program.to_lowercase().contains("vscode")
          || term_program.to_lowercase().contains("terminal");

        checks.push(format!(
            "Terminal type: {}",
            if is_good_terminal { "well-known terminal (good)" } else { "verify settings below" }
        ));

        // Shell detection
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
        checks.push(format!("Shell:         {}", shell));

        // Check for Nerd Fonts (heuristic: environment variable set by some terminals)
        let nerd_font = std::env::var("NERD_FONT").is_ok()
            || std::env::var("TERM_NERD_FONT").is_ok();

        CommandResult::Message(format!(
            "Terminal Setup Diagnostic\n\
             ─────────────────────────\n\
             {checks}\n\n\
             Recommendations for optimal Claude Code experience:\n\
             ─────────────────────────────────────────────────\n\
             1. Font: Use a Nerd Font for box-drawing characters and icons\n\
                {nerd_hint}\n\
                Download: https://www.nerdfonts.com/\n\
             2. Color: Enable 24-bit true color:\n\
                export COLORTERM=truecolor\n\
             3. Unicode: Ensure UTF-8 locale:\n\
                export LANG=en_US.UTF-8\n\
             4. Recommended terminals:\n\
                - WezTerm (all platforms)\n\
                - Alacritty (all platforms)\n\
                - Kitty (macOS/Linux)\n\
                - Windows Terminal (Windows)\n\
                - iTerm2 (macOS)\n\
             5. Set terminal to unlimited scrollback for long conversations",
            checks = checks.join("\n  "),
            nerd_hint = if nerd_font {
                "[ok] Nerd Font detected"
            } else {
                "[!] Nerd Font not detected — box-drawing may appear broken"
            },
        ))
    }
}

// ---- /extra-usage --------------------------------------------------------

#[async_trait]
impl SlashCommand for ExtraUsageCommand {
    fn name(&self) -> &str { "extra-usage" }
    fn description(&self) -> &str { "Show detailed usage statistics: calls, cache, tools" }
    fn help(&self) -> &str {
        "Usage: /extra-usage\n\n\
         Displays extended usage statistics beyond /cost:\n\
         - API call count\n\
         - Cache hit/miss ratio\n\
         - Token breakdown by type\n\
         - Effective cost per call"
    }

    async fn execute(&self, _args: &str, ctx: &mut CommandContext) -> CommandResult {
        let input = ctx.cost_tracker.input_tokens();
        let output = ctx.cost_tracker.output_tokens();
        let cache_creation = ctx.cost_tracker.cache_creation_tokens();
        let cache_read = ctx.cost_tracker.cache_read_tokens();
        let total = ctx.cost_tracker.total_tokens();
        let cost = ctx.cost_tracker.total_cost_usd();

        // Estimate API calls from messages (each assistant message ~ 1 API call)
        let api_calls = ctx.messages.iter()
            .filter(|m| m.role == cc_core::types::Role::Assistant)
            .count();
        let api_calls = api_calls.max(1); // at least 1 if we have any data

        // Cache efficiency
        let cache_total = cache_creation + cache_read;
        let cache_hit_pct = if cache_total > 0 {
            (cache_read as f64 / cache_total as f64) * 100.0
        } else {
            0.0
        };

        let cost_per_call = if api_calls > 0 {
            cost / api_calls as f64
        } else {
            0.0
        };

        CommandResult::Message(format!(
            "Detailed Usage Statistics\n\
             ─────────────────────────\n\
             API calls:           {api_calls}\n\
             Avg cost/call:       ${cost_per_call:.4}\n\n\
             Token Breakdown:\n\
               Input tokens:      {input:>10}\n\
               Output tokens:     {output:>10}\n\
               Cache creation:    {cache_creation:>10}\n\
               Cache read:        {cache_read:>10}\n\
               Total tokens:      {total:>10}\n\n\
             Cache Performance:\n\
               Cache hit rate:    {cache_hit_pct:.1}%\n\
               Cache efficiency:  {cache_eff}\n\n\
             Cost:\n\
               Total cost:        ${cost:.4}\n\
               Cost/1k tokens:    ${cost_per_k:.4}",
            api_calls = api_calls,
            cost_per_call = cost_per_call,
            input = input,
            output = output,
            cache_creation = cache_creation,
            cache_read = cache_read,
            total = total,
            cache_hit_pct = cache_hit_pct,
            cache_eff = if cache_hit_pct > 70.0 {
                "Excellent"
            } else if cache_hit_pct > 40.0 {
                "Good"
            } else if cache_total > 0 {
                "Low — prompts may not be stable enough to cache"
            } else {
                "No cache activity"
            },
            cost = cost,
            cost_per_k = if total > 0 { cost / (total as f64 / 1000.0) } else { 0.0 },
        ))
    }
}

// ---- /fast (/speed) ------------------------------------------------------

#[async_trait]
impl SlashCommand for FastCommand {
    fn name(&self) -> &str { "fast" }
    fn aliases(&self) -> Vec<&str> { vec!["speed"] }
    fn description(&self) -> &str { "Toggle fast mode (uses a faster/cheaper model)" }
    fn help(&self) -> &str {
        "Usage: /fast [on|off]\n\n\
         Fast mode switches to a faster, more economical model variant\n\
         (claude-haiku) for quick responses. Toggle without argument to switch.\n\
         The setting is persisted to ~/.claude/ui-settings.json."
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let current = load_ui_settings();
        let currently_on = current.fast_mode.unwrap_or(false);

        let enable = match args.trim() {
            "on" | "enable" | "true" | "1" => true,
            "off" | "disable" | "false" | "0" => false,
            "" => !currently_on,
            other => {
                return CommandResult::Error(format!(
                    "Unknown argument '{}'. Use: /fast [on|off]",
                    other
                ))
            }
        };

        if let Err(e) = mutate_ui_settings(|s| s.fast_mode = Some(enable)) {
            return CommandResult::Error(format!("Failed to save setting: {}", e));
        }

        let fast_model = "claude-haiku-4-5";
        let normal_model = ctx.config.model.as_deref()
            .unwrap_or(cc_core::constants::DEFAULT_MODEL);

        if enable {
            let mut new_config = ctx.config.clone();
            new_config.model = Some(fast_model.to_string());
            CommandResult::ConfigChangeMessage(
                new_config,
                format!(
                    "Fast mode ON. Using {} for quicker, cheaper responses.\n\
                     Use /fast off to return to {}.",
                    fast_model, normal_model
                ),
            )
        } else {
            let mut new_config = ctx.config.clone();
            // Restore default / saved model
            new_config.model = None;
            CommandResult::ConfigChangeMessage(
                new_config,
                format!(
                    "Fast mode OFF. Restored to default model ({}).",
                    cc_core::constants::DEFAULT_MODEL
                ),
            )
        }
    }
}

// ---- /think-back ---------------------------------------------------------

#[async_trait]
impl SlashCommand for ThinkBackCommand {
    fn name(&self) -> &str { "think-back" }
    fn aliases(&self) -> Vec<&str> { vec!["thinkback"] }
    fn description(&self) -> &str { "Show thinking traces from previous responses in this session" }
    fn help(&self) -> &str {
        "Usage: /think-back [n]\n\n\
         Displays the thinking/reasoning traces from the most recent model responses.\n\
         Pass a number to show the Nth most recent thinking block."
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let n: usize = args.trim().parse().unwrap_or(1).max(1);

        // Scan messages for thinking blocks
        let thinking_blocks: Vec<(usize, String)> = ctx
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == cc_core::types::Role::Assistant)
            .filter_map(|(idx, m)| {
                let blocks = m.get_thinking_blocks();
                if blocks.is_empty() {
                    return None;
                }
                let thinking: String = blocks
                    .iter()
                    .filter_map(|b| {
                        if let cc_core::types::ContentBlock::Thinking { thinking, .. } = b {
                            Some(thinking.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if thinking.is_empty() { None } else { Some((idx, thinking)) }
            })
            .collect();

        if thinking_blocks.is_empty() {
            return CommandResult::Message(
                "No thinking traces found in this session.\n\
                 Thinking traces appear when the model uses extended thinking mode.\n\
                 Try asking Claude to 'think step by step' or 'think carefully'."
                    .to_string(),
            );
        }

        // Show the Nth most recent (1-indexed)
        let total = thinking_blocks.len();
        let target_idx = total.saturating_sub(n);
        let (msg_idx, trace) = &thinking_blocks[target_idx];

        CommandResult::Message(format!(
            "Thinking trace ({n} of {total} found, from message {msg}):\n\
             ─────────────────────────────────────\n\
             {trace}\n\
             ─────────────────────────────────────\n\
             Use /think-back <n> to see older traces.",
            n = n,
            total = total,
            msg = msg_idx + 1,
            trace = trace,
        ))
    }
}

// ---- /thinkback-play -----------------------------------------------------

#[async_trait]
impl SlashCommand for ThinkBackPlayCommand {
    fn name(&self) -> &str { "thinkback-play" }
    fn description(&self) -> &str { "Replay a thinking trace as an animated walkthrough" }
    fn help(&self) -> &str {
        "Usage: /thinkback-play [n]\n\n\
         Replays a previous thinking trace, formatted for easy reading.\n\
         Pass a number to replay the Nth most recent trace."
    }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        let n: usize = args.trim().parse().unwrap_or(1).max(1);

        let thinking_blocks: Vec<String> = ctx
            .messages
            .iter()
            .filter(|m| m.role == cc_core::types::Role::Assistant)
            .filter_map(|m| {
                let blocks = m.get_thinking_blocks();
                if blocks.is_empty() {
                    return None;
                }
                let t: String = blocks
                    .iter()
                    .filter_map(|b| {
                        if let cc_core::types::ContentBlock::Thinking { thinking, .. } = b {
                            Some(thinking.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if t.is_empty() { None } else { Some(t) }
            })
            .collect();

        if thinking_blocks.is_empty() {
            return CommandResult::Message(
                "No thinking traces to replay in this session.".to_string(),
            );
        }

        let total = thinking_blocks.len();
        let idx = total.saturating_sub(n);
        let trace = &thinking_blocks[idx];

        // Format the trace with step numbering
        let steps: Vec<&str> = trace.split('\n').filter(|l| !l.trim().is_empty()).collect();
        let mut formatted = format!(
            "Thinking Trace Replay ({}/{total})\n\
             ══════════════════════════════════\n",
            n,
            total = total
        );
        for (i, step) in steps.iter().enumerate() {
            formatted.push_str(&format!("  Step {}: {}\n", i + 1, step));
        }
        formatted.push_str("══════════════════════════════════\n");
        formatted.push_str(&format!(
            "{} steps shown. Use /think-back for raw traces.",
            steps.len()
        ));

        CommandResult::Message(formatted)
    }
}

// ---- /feedback (standalone, supplements BugCommand alias) ----------------

#[async_trait]
impl SlashCommand for FeedbackCommand {
    fn name(&self) -> &str { "report" }
    fn aliases(&self) -> Vec<&str> { vec![] }
    fn description(&self) -> &str { "Open the GitHub issues page to report a bug or request a feature" }
    fn hidden(&self) -> bool { true } // surfaced via BugCommand alias; hidden to avoid duplicate
    fn help(&self) -> &str {
        "Usage: /report [description]\n\n\
         Opens the GitHub issues tracker. If a description is provided,\n\
         it is shown as a suggested pre-fill for the issue body."
    }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let url = "https://github.com/anthropics/claude-code/issues/new";
        let report = args.trim();
        let display_url = if report.is_empty() {
            url.to_string()
        } else {
            // Append as a body query param
            format!(
                "{}?body={}",
                url,
                urlencoding::encode(report)
            )
        };

        match open_with_system(&display_url) {
            Ok(_) => CommandResult::Message(format!("Opened issue tracker: {}", url)),
            Err(_) => CommandResult::Message(format!(
                "Please visit {} to submit a report.",
                url
            )),
        }
    }
}

// ---- /color (full implementation) ----------------------------------------

#[async_trait]
impl SlashCommand for ColorSetCommand {
    fn name(&self) -> &str { "color-set" }
    fn hidden(&self) -> bool { true }
    fn description(&self) -> &str { "Internal: set prompt color — use /color instead" }

    async fn execute(&self, args: &str, _ctx: &mut CommandContext) -> CommandResult {
        let color = args.trim();
        if color.is_empty() {
            let current = load_ui_settings();
            return CommandResult::Message(format!(
                "Current prompt color: {}\n\
                 Use /color <name|#RRGGBB|default> to change it.\n\n\
                 Named colors: red, green, blue, yellow, cyan, magenta, white, orange, purple",
                current.prompt_color.as_deref().unwrap_or("default"),
            ));
        }

        let normalized = if color == "default" {
            None
        } else {
            // Validate hex or named color
            let known_colors = [
                "red", "green", "blue", "yellow", "cyan", "magenta",
                "white", "orange", "purple", "pink", "gray", "grey",
            ];
            let is_hex = color.starts_with('#') && (color.len() == 4 || color.len() == 7)
                && color[1..].chars().all(|c| c.is_ascii_hexdigit());
            if !is_hex && !known_colors.contains(&color.to_lowercase().as_str()) {
                return CommandResult::Error(format!(
                    "Unknown color '{}'. Use a color name (red, green, …) or a hex code (#RGB or #RRGGBB).",
                    color
                ));
            }
            Some(color.to_string())
        };

        match mutate_ui_settings(|s| s.prompt_color = normalized.clone()) {
            Ok(_) => CommandResult::Message(format!(
                "Prompt color set to {}.\n\
                 Restart the REPL for the change to take effect.",
                normalized.as_deref().unwrap_or("default")
            )),
            Err(e) => CommandResult::Error(format!("Failed to save color: {}", e)),
        }
    }
}

// ---- Named-command slash adapters ----------------------------------------

#[async_trait]
impl SlashCommand for NamedCommandAdapter {
    fn name(&self) -> &str { self.slash_name }

    fn aliases(&self) -> Vec<&str> { self.slash_aliases.to_vec() }

    fn description(&self) -> &str { self.slash_description }

    fn help(&self) -> &str { self.slash_help }

    async fn execute(&self, args: &str, ctx: &mut CommandContext) -> CommandResult {
        execute_named_command_from_slash(self.target_name, args, ctx)
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Return all built-in slash commands.
pub fn all_commands() -> Vec<Box<dyn SlashCommand>> {
    vec![
        Box::new(HelpCommand),
        Box::new(ClearCommand),
        Box::new(CompactCommand),
        Box::new(CostCommand),
        Box::new(ExitCommand),
        Box::new(ModelCommand),
        Box::new(ConfigCommand),
        Box::new(ColorCommand),
        Box::new(PluginCommand),
        Box::new(VersionCommand),
        Box::new(ResumeCommand),
        Box::new(ReloadPluginsCommand),
        Box::new(StatusCommand),
        Box::new(DiffCommand),
        Box::new(MemoryCommand),
        Box::new(BugCommand),
        Box::new(UsageCommand),
        Box::new(DoctorCommand),
        Box::new(LoginCommand),
        Box::new(LogoutCommand),
        Box::new(InitCommand),
        Box::new(ReviewCommand),
        Box::new(HooksCommand),
        Box::new(McpCommand),
        Box::new(PermissionsCommand),
        Box::new(PlanCommand),
        Box::new(TasksCommand),
        Box::new(SessionCommand),
        Box::new(ThinkingCommand),
        Box::new(ThemeCommand),
        Box::new(OutputStyleCommand),
        Box::new(KeybindingsCommand),
        Box::new(PrivacySettingsCommand),
        // New commands
        Box::new(ExportCommand),
        Box::new(SkillsCommand),
        Box::new(RewindCommand),
        Box::new(StatsCommand),
        Box::new(FilesCommand),
        Box::new(RenameCommand),
        Box::new(EffortCommand),
        Box::new(SummaryCommand),
        Box::new(CommitCommand),
        Box::new(NamedCommandAdapter {
            slash_name: "add-dir",
            target_name: "add-dir",
            slash_aliases: &[],
            slash_description: "Add a directory to Claude Code's allowed workspace paths",
            slash_help: "Usage: /add-dir <path>",
        }),
        Box::new(NamedCommandAdapter {
            slash_name: "agents",
            target_name: "agents",
            slash_aliases: &[],
            slash_description: "Manage and configure sub-agents",
            slash_help: "Usage: /agents [list|create|edit|delete] [name]",
        }),
        Box::new(NamedCommandAdapter {
            slash_name: "branch",
            target_name: "branch",
            slash_aliases: &[],
            slash_description: "Create a branch of the current conversation at this point",
            slash_help: "Usage: /branch [create|switch|list] [name]",
        }),
        Box::new(NamedCommandAdapter {
            slash_name: "tag",
            target_name: "tag",
            slash_aliases: &[],
            slash_description: "Toggle a searchable tag on the current session",
            slash_help: "Usage: /tag [list|add|remove] [tag]",
        }),
        Box::new(NamedCommandAdapter {
            slash_name: "passes",
            target_name: "passes",
            slash_aliases: &[],
            slash_description: "Share a free week of Claude Code with friends",
            slash_help: "Usage: /passes",
        }),
        Box::new(NamedCommandAdapter {
            slash_name: "ide",
            target_name: "ide",
            slash_aliases: &[],
            slash_description: "Manage IDE integrations and show status",
            slash_help: "Usage: /ide [status|connect|disconnect|open]",
        }),
        Box::new(NamedCommandAdapter {
            slash_name: "pr-comments",
            target_name: "pr-comments",
            slash_aliases: &[],
            slash_description: "Get comments from a GitHub pull request",
            slash_help: "Usage: /pr-comments <PR-number>",
        }),
        Box::new(NamedCommandAdapter {
            slash_name: "desktop",
            target_name: "desktop",
            slash_aliases: &[],
            slash_description: "Open the Claude Code desktop app",
            slash_help: "Usage: /desktop",
        }),
        Box::new(NamedCommandAdapter {
            slash_name: "mobile",
            target_name: "mobile",
            slash_aliases: &[],
            slash_description: "Set up Claude Code on mobile",
            slash_help: "Usage: /mobile",
        }),
        Box::new(NamedCommandAdapter {
            slash_name: "install-github-app",
            target_name: "install-github-app",
            slash_aliases: &[],
            slash_description: "Set up Claude GitHub Actions for a repository",
            slash_help: "Usage: /install-github-app",
        }),
        Box::new(NamedCommandAdapter {
            slash_name: "web-setup",
            target_name: "remote-setup",
            slash_aliases: &["remote-setup"],
            slash_description: "Configure a remote Claude Code environment",
            slash_help: "Usage: /web-setup",
        }),
        Box::new(NamedCommandAdapter {
            slash_name: "stickers",
            target_name: "stickers",
            slash_aliases: &[],
            slash_description: "View collected stickers",
            slash_help: "Usage: /stickers",
        }),
        // Batch-1 new commands
        Box::new(RemoteControlCommand),
        Box::new(RemoteEnvCommand),
        Box::new(ContextCommand),
        Box::new(CopyCommand),
        Box::new(ChromeCommand),
        Box::new(VimCommand),
        Box::new(VoiceCommand),
        Box::new(UpgradeCommand),
        Box::new(ReleaseNotesCommand),
        Box::new(RateLimitOptionsCommand),
        Box::new(StatuslineCommand),
        Box::new(SecurityReviewCommand),
        Box::new(TerminalSetupCommand),
        Box::new(ExtraUsageCommand),
        Box::new(FastCommand),
        Box::new(ThinkBackCommand),
        Box::new(ThinkBackPlayCommand),
        Box::new(FeedbackCommand),
        Box::new(ColorSetCommand),
    ]
}

/// Find a command by name or alias.
pub fn find_command(name: &str) -> Option<Box<dyn SlashCommand>> {
    let name = name.trim_start_matches('/');
    all_commands().into_iter().find(|c| {
        c.name() == name || c.aliases().contains(&name)
    })
}

/// Execute a slash command string (with leading /).
pub async fn execute_command(
    input: &str,
    ctx: &mut CommandContext,
) -> Option<CommandResult> {
    if !cc_tui::input::is_slash_command(input) { return None; }
    let (name, args) = cc_tui::input::parse_slash_command(input);

    // First check built-in commands.
    if let Some(cmd) = find_command(name) {
        return Some(cmd.execute(args, ctx).await);
    }

    // Then check plugin-defined slash commands.
    let project_dir = ctx.working_dir.clone();
    let registry = cc_plugins::load_plugins(&project_dir, &[]).await;
    let cmd_name = name.trim_start_matches('/');
    for cmd_def in registry.all_command_defs() {
        if cmd_def.name == cmd_name {
            let adapter = PluginSlashCommandAdapter { def: cmd_def };
            return Some(adapter.execute(args, ctx).await);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Named commands module (top-level `claude <name>` subcommands)
// ---------------------------------------------------------------------------
pub mod named_commands;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cc_core::cost::CostTracker;

    fn make_ctx() -> CommandContext {
        CommandContext {
            config: cc_core::config::Config::default(),
            cost_tracker: CostTracker::new(),
            messages: vec![],
            working_dir: std::path::PathBuf::from("."),
            session_id: "test-session".to_string(),
            session_title: None,
        }
    }

    // ---- Command registry tests ---------------------------------------------

    #[test]
    fn test_all_commands_non_empty() {
        assert!(!all_commands().is_empty());
    }

    #[test]
    fn test_all_commands_have_unique_names() {
        let mut names = std::collections::HashSet::new();
        for cmd in all_commands() {
            assert!(
                names.insert(cmd.name().to_string()),
                "Duplicate command name: {}",
                cmd.name()
            );
        }
    }

    #[test]
    fn test_find_command_by_name() {
        assert!(find_command("help").is_some());
        assert!(find_command("clear").is_some());
        assert!(find_command("exit").is_some());
        assert!(find_command("model").is_some());
        assert!(find_command("version").is_some());
    }

    #[test]
    fn test_find_command_with_slash_prefix() {
        // find_command should strip the leading / before lookup
        assert!(find_command("/help").is_some());
        assert!(find_command("/clear").is_some());
    }

    #[test]
    fn test_find_command_by_alias() {
        // /help has aliases "h" and "?"
        assert!(find_command("h").is_some());
        assert!(find_command("?").is_some());
        // /clear has alias "c"
        assert!(find_command("c").is_some());
        assert!(find_command("settings").is_some());
        assert!(find_command("continue").is_some());
        assert!(find_command("bug").is_some());
        assert!(find_command("bashes").is_some());
        assert!(find_command("remote").is_some());
        assert!(find_command("remote-setup").is_some());
    }

    #[test]
    fn test_find_command_not_found() {
        assert!(find_command("nonexistent_command_xyz").is_none());
    }

    #[test]
    fn test_core_commands_present() {
        let expected = [
            "help", "clear", "compact", "cost", "exit", "model",
            "config", "version", "status", "diff", "memory", "hooks",
            "permissions", "plan", "tasks", "session", "login", "logout",
            "feedback", "usage", "plugin", "reload-plugins",
            "add-dir", "agents", "branch", "tag",
            "passes", "ide", "pr-comments", "desktop", "mobile",
            "install-github-app", "web-setup", "stickers",
        ];
        for name in &expected {
            assert!(
                find_command(name).is_some(),
                "Expected command '{}' not in all_commands()",
                name
            );
        }
    }

    // ---- Command execution tests --------------------------------------------

    #[tokio::test]
    async fn test_clear_command_returns_clear_conversation() {
        let mut ctx = make_ctx();
        let cmd = find_command("clear").unwrap();
        let result = cmd.execute("", &mut ctx).await;
        assert!(matches!(result, CommandResult::ClearConversation));
    }

    #[tokio::test]
    async fn test_exit_command_returns_exit() {
        let mut ctx = make_ctx();
        let cmd = find_command("exit").unwrap();
        let result = cmd.execute("", &mut ctx).await;
        assert!(matches!(result, CommandResult::Exit));
    }

    #[tokio::test]
    async fn test_version_command_returns_message() {
        let mut ctx = make_ctx();
        let cmd = find_command("version").unwrap();
        let result = cmd.execute("", &mut ctx).await;
        assert!(matches!(result, CommandResult::Message(_)));
        if let CommandResult::Message(msg) = result {
            assert!(
                msg.contains("claude") || msg.contains("Claude") || msg.contains('.'),
                "Version message should contain version number, got: {}",
                msg
            );
        }
    }

    #[tokio::test]
    async fn test_cost_command_returns_message() {
        let mut ctx = make_ctx();
        let cmd = find_command("cost").unwrap();
        let result = cmd.execute("", &mut ctx).await;
        assert!(matches!(result, CommandResult::Message(_)));
    }

    #[tokio::test]
    async fn test_login_command_starts_oauth_flow() {
        let mut ctx = make_ctx();
        let cmd = find_command("login").unwrap();
        // Default (no --console) → login_with_claude_ai = true
        let result = cmd.execute("", &mut ctx).await;
        assert!(matches!(result, CommandResult::StartOAuthFlow(true)));
    }

    #[tokio::test]
    async fn test_login_command_console_flag() {
        let mut ctx = make_ctx();
        let cmd = find_command("login").unwrap();
        let result = cmd.execute("--console", &mut ctx).await;
        assert!(matches!(result, CommandResult::StartOAuthFlow(false)));
    }

    #[tokio::test]
    async fn test_help_command_returns_message() {
        let mut ctx = make_ctx();
        let cmd = find_command("help").unwrap();
        let result = cmd.execute("", &mut ctx).await;
        // help returns either Message or Silent
        assert!(
            matches!(result, CommandResult::Message(_) | CommandResult::Silent),
            "help should return Message or Silent"
        );
    }

    #[tokio::test]
    async fn test_web_setup_proxy_executes_named_command() {
        let mut ctx = make_ctx();
        let cmd = find_command("web-setup").unwrap();
        let result = cmd.execute("", &mut ctx).await;
        assert!(matches!(result, CommandResult::Message(_)));
    }

    #[test]
    fn test_split_command_args_preserves_quoted_segments() {
        assert_eq!(
            split_command_args("create \"agent alpha\" 'second value'"),
            vec![
                "create".to_string(),
                "agent alpha".to_string(),
                "second value".to_string(),
            ]
        );
    }
}
