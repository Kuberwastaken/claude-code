// claurst CLI entry point
//
// This is the main binary for Claurst. It:
// 1. Parses CLI arguments with clap (mirrors cli.tsx + main.tsx flags)
// 2. Loads configuration from settings.json + env vars
// 3. Builds system/user context (git status, AGENTS.md)
// 4. Runs in either:
//    - Headless (--print / -p) mode: single query, output to stdout
//    - Interactive REPL mode: full TUI with ratatui

mod oauth_flow;
mod codex_oauth_flow;

// ---------------------------------------------------------------------------
// Build-time metadata (embedded via build.rs)
// ---------------------------------------------------------------------------

/// Build timestamp in RFC 3339 format
pub const BUILD_TIME: &str = env!("BUILD_TIME");

/// Short git commit hash (or "unknown" if not a git repo)
pub const GIT_COMMIT: &str = env!("GIT_COMMIT");

/// Package/distribution identifier
pub const PACKAGE_URL: &str = env!("PACKAGE_URL");

/// Feedback/issue reporting channel
pub const FEEDBACK_CHANNEL: &str = env!("FEEDBACK_CHANNEL");

/// Explanation of issue routing in this build
pub const ISSUES_EXPLAINER: &str = env!("ISSUES_EXPLAINER");

use anyhow::Context;
use claurst_core::{
    config::{Config, PermissionMode, Settings},
    constants::APP_VERSION,
    context::ContextBuilder,
    cost::CostTracker,
    permissions::{AutoPermissionHandler, InteractivePermissionHandler, PermissionManager},
};
use async_trait::async_trait;
use claurst_core::types::ToolDefinition;
use claurst_tools::{PermissionLevel, Tool, ToolContext, ToolResult};
use clap::{ArgAction, Parser, ValueEnum};
use parking_lot::Mutex as ParkingMutex;
use std::{path::PathBuf, sync::Arc};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

// ---------------------------------------------------------------------------
// MCP tool wrapper: makes MCP server tools look like native cc-tools.
// ---------------------------------------------------------------------------

struct McpToolWrapper {
    tool_def: ToolDefinition,
    server_name: String,
    manager: Arc<claurst_mcp::McpManager>,
}

#[async_trait]
impl Tool for McpToolWrapper {
    fn name(&self) -> &str {
        &self.tool_def.name
    }

    fn description(&self) -> &str {
        &self.tool_def.description
    }

    fn permission_level(&self) -> PermissionLevel {
        // MCP tools run external processes – treat as Execute.
        PermissionLevel::Execute
    }

    fn input_schema(&self) -> serde_json::Value {
        self.tool_def.input_schema.clone()
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let desc = format!("Run MCP tool {}", self.tool_def.name);
        if let Err(e) = ctx.check_permission(self.name(), &desc, false) {
            return ToolResult::error(e.to_string());
        }

        // Strip the server-name prefix to get the bare tool name.
        let prefix = format!("{}_", self.server_name);
        let bare_name = self
            .tool_def
            .name
            .strip_prefix(&prefix)
            .unwrap_or(&self.tool_def.name);

        let args = if input.is_null() { None } else { Some(input) };

        match self.manager.call_tool(&self.tool_def.name, args).await {
            Ok(result) => {
                let text = claurst_mcp::mcp_result_to_string(&result);
                if result.is_error {
                    ToolResult::error(text)
                } else {
                    ToolResult::success(text)
                }
            }
            Err(e) => ToolResult::error(format!("MCP tool '{}' failed: {}", bare_name, e)),
        }
    }
}

// ---------------------------------------------------------------------------
// CLI argument definition (matches TypeScript main.tsx flags)
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "claude",
    version = APP_VERSION,
    about = "Claurst - AI-powered coding assistant",
    long_about = None,
)]
struct Cli {
    /// Initial prompt to send (enables headless/print mode)
    prompt: Option<String>,

    /// Print mode: send prompt and exit (non-interactive)
    #[arg(short = 'p', long = "print", action = ArgAction::SetTrue)]
    print: bool,

    /// Model to use
    #[arg(short = 'm', long = "model")]
    model: Option<String>,

    /// Permission mode
    #[arg(long = "permission-mode", value_enum, default_value_t = CliPermissionMode::Default)]
    permission_mode: CliPermissionMode,

    /// Resume a previous session by ID
    #[arg(long = "resume")]
    resume: Option<String>,

    /// Maximum number of agentic turns
    #[arg(long = "max-turns", default_value_t = 10)]
    max_turns: u32,

    /// Custom system prompt
    #[arg(long = "system-prompt", short = 's')]
    system_prompt: Option<String>,

    /// Append to system prompt
    #[arg(long = "append-system-prompt")]
    append_system_prompt: Option<String>,

    /// Disable AGENTS.md memory files
    #[arg(long = "no-claude-md", action = ArgAction::SetTrue)]
    no_claude_md: bool,

    /// Output format
    #[arg(long = "output-format", value_enum, default_value_t = CliOutputFormat::Text)]
    output_format: CliOutputFormat,

    /// Enable verbose logging
    #[arg(long = "verbose", short = 'v', action = ArgAction::SetTrue)]
    verbose: bool,

    /// API key for the active provider (overrides provider-specific env vars)
    #[arg(long = "api-key")]
    api_key: Option<String>,

    /// Maximum tokens per response
    #[arg(long = "max-tokens")]
    max_tokens: Option<u32>,

    /// Working directory
    #[arg(long = "cwd")]
    cwd: Option<PathBuf>,

    /// Bypass all permission checks (danger!)
    #[arg(long = "dangerously-skip-permissions", action = ArgAction::SetTrue)]
    dangerously_skip_permissions: bool,

    /// Dump the system prompt to stdout and exit
    #[arg(long = "dump-system-prompt", action = ArgAction::SetTrue, hide = true)]
    dump_system_prompt: bool,

    /// MCP config JSON string (inline server definitions)
    #[arg(long = "mcp-config")]
    mcp_config: Option<String>,

    /// Disable auto-compaction
    #[arg(long = "no-auto-compact", action = ArgAction::SetTrue)]
    no_auto_compact: bool,

    /// Grant Claurst access to an additional directory (can be repeated)
    #[arg(long = "add-dir", value_name = "DIR", action = ArgAction::Append)]
    add_dir: Vec<PathBuf>,

    /// Input format for --print mode (text or stream-json)
    #[arg(long = "input-format", value_enum, default_value_t = CliInputFormat::Text)]
    input_format: CliInputFormat,

    /// Session ID to tag this headless run (for tracking in logs/hooks)
    #[arg(long = "session-id")]
    session_id_flag: Option<String>,

    /// Prefill the first assistant turn with this text
    #[arg(long = "prefill")]
    prefill: Option<String>,

    /// Effort level for extended thinking (low, medium, high, max)
    #[arg(long = "effort", value_name = "LEVEL")]
    effort: Option<String>,

    /// Extended thinking budget in tokens (enables extended thinking)
    #[arg(long = "thinking", value_name = "TOKENS")]
    thinking: Option<u32>,

    /// Continue the most recent conversation
    #[arg(short = 'c', long = "continue", action = ArgAction::SetTrue)]
    continue_session: bool,

    /// Override system prompt from a file
    #[arg(long = "system-prompt-file")]
    system_prompt_file: Option<PathBuf>,

    /// Tools to allow (comma-separated, default: all)
    #[arg(long = "allowed-tools", value_name = "TOOLS")]
    allowed_tools: Option<String>,

    /// Tools to disallow (comma-separated)
    #[arg(long = "disallowed-tools", value_name = "TOOLS")]
    disallowed_tools: Option<String>,

    /// Extra beta feature headers to send (comma-separated)
    #[arg(long = "betas", value_name = "HEADERS")]
    betas: Option<String>,

    /// Disable all slash commands
    #[arg(long = "disable-slash-commands", action = ArgAction::SetTrue)]
    disable_slash_commands: bool,

    /// Run in bare mode (no hooks, no plugins, no AGENTS.md)
    #[arg(long = "bare", action = ArgAction::SetTrue)]
    bare: bool,

    /// Billing workload tag
    #[arg(long = "workload", value_name = "TAG")]
    workload: Option<String>,

    /// Maximum spend in USD before aborting the query loop
    #[arg(long = "max-budget-usd", value_name = "USD")]
    max_budget_usd: Option<f64>,

    /// Fallback model to use if the primary model is overloaded or unavailable
    #[arg(long = "fallback-model")]
    fallback_model: Option<String>,

    /// LLM provider to use (default: anthropic). Examples: openai, google, ollama
    #[arg(long, env = "CLAURST_PROVIDER")]
    provider: Option<String>,

    /// Override the API base URL for the selected provider
    #[arg(long, env = "CLAURST_API_BASE")]
    api_base: Option<String>,

    /// Named agent to use (e.g., build, plan, explore)
    #[arg(long, short = 'A')]
    agent: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum CliPermissionMode {
    Default,
    AcceptEdits,
    BypassPermissions,
    Plan,
}

impl From<CliPermissionMode> for PermissionMode {
    fn from(m: CliPermissionMode) -> Self {
        match m {
            CliPermissionMode::Default => PermissionMode::Default,
            CliPermissionMode::AcceptEdits => PermissionMode::AcceptEdits,
            CliPermissionMode::BypassPermissions => PermissionMode::BypassPermissions,
            CliPermissionMode::Plan => PermissionMode::Plan,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum CliOutputFormat {
    Text,
    Json,
    #[value(name = "stream-json")]
    StreamJson,
}

impl From<CliOutputFormat> for claurst_core::config::OutputFormat {
    fn from(f: CliOutputFormat) -> Self {
        match f {
            CliOutputFormat::Text => claurst_core::config::OutputFormat::Text,
            CliOutputFormat::Json => claurst_core::config::OutputFormat::Json,
            CliOutputFormat::StreamJson => claurst_core::config::OutputFormat::StreamJson,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum CliInputFormat {
    /// Plain text prompt (default)
    Text,
    /// Newline-delimited JSON messages — each line is {"role":"user"|"assistant","content":"..."}
    #[value(name = "stream-json")]
    StreamJson,
}

fn resolve_bridge_config(
    settings: &Settings,
    auth_credential: &str,
    use_bearer_auth: bool,
    is_headless: bool,
) -> Option<claurst_bridge::BridgeConfig> {
    if is_headless {
        return None;
    }

    let mut bridge_config = claurst_bridge::BridgeConfig::from_env();

    if settings.remote_control_at_startup {
        bridge_config.enabled = true;
    }

    if bridge_config.session_token.is_none() && use_bearer_auth && !auth_credential.is_empty() {
        bridge_config.session_token = Some(auth_credential.to_string());
    }

    bridge_config.is_active().then_some(bridge_config)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Fast-path: handle --version before parsing everything
    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.iter().any(|a| a == "--version" || a == "-V") {
        println!("claude {}", APP_VERSION);
        return Ok(());
    }

    // Fast-path: `claude auth <login|logout|status>` — mirrors TypeScript cli.tsx pattern
    if raw_args.get(1).map(|s| s.as_str()) == Some("auth") {
        return handle_auth_command(&raw_args[2..]).await;
    }

    // Fast-path: `claude acp` — start the Agent Client Protocol stdio server.
    if raw_args.get(1).map(|s| s.as_str()) == Some("acp") {
        return claurst_acp::run_acp_server().await;
    }

    // Fast-path: `claude models` — list all available providers and models.
    if raw_args.get(1).map(|s| s.as_str()) == Some("models") {
        let mut registry = claurst_api::ModelRegistry::new();
        // Load cached models.dev data if available so the list is comprehensive.
        registry.load_cache(&models_cache_path());
        let mut entries = registry.list_all();
        // Sort by provider then model id for stable output.
        entries.sort_by(|a, b| {
            (&*a.info.provider_id).cmp(&*b.info.provider_id)
                .then_with(|| (&*a.info.id).cmp(&*b.info.id))
        });
        for entry in entries {
            println!(
                "{}/{} — {} (ctx: {}K, in: ${:.2}/M, out: ${:.2}/M)",
                entry.info.provider_id,
                entry.info.id,
                entry.info.name,
                entry.info.context_window / 1000,
                entry.cost_input.unwrap_or(0.0),
                entry.cost_output.unwrap_or(0.0),
            );
        }
        return Ok(());
    }

    // Fast-path: named commands (`claude agents`, `claude ide`, `claude branch`, …)
    // Check before Cli::parse() so these names don't conflict with positional prompt arg.
    if let Some(cmd_name) = raw_args.get(1).map(|s| s.as_str()) {
        // Only intercept if it looks like a subcommand (no leading `-` or `/`)
        if !cmd_name.starts_with('-') && !cmd_name.starts_with('/') {
            if let Some(named_cmd) = claurst_commands::named_commands::find_named_command(cmd_name) {
                // Build a minimal CommandContext (named commands are pre-session)
                let settings = Settings::load().await.unwrap_or_default();
                let config = settings.effective_config();
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let cmd_ctx = claurst_commands::CommandContext {
                    config,
                    cost_tracker: CostTracker::new(),
                    messages: vec![],
                    working_dir: cwd,
                    session_id: "pre-session".to_string(),
                    session_title: None,
                    remote_session_url: None,
                    mcp_manager: None,
                    mcp_auth_runner: None,
                };
                // Collect remaining args after the command name
                let rest: Vec<&str> = raw_args[2..].iter().map(|s| s.as_str()).collect();
                let result = named_cmd.execute_named(&rest, &cmd_ctx);
                match result {
                    claurst_commands::CommandResult::Message(msg)
                    | claurst_commands::CommandResult::UserMessage(msg) => {
                        println!("{}", msg);
                        std::process::exit(0);
                    }
                    claurst_commands::CommandResult::Error(e) => {
                        eprintln!("Error: {}", e);
                        eprintln!("Usage: {}", named_cmd.usage());
                        std::process::exit(1);
                    }
                    _ => {
                        // For any other result variant, fall through to normal startup
                    }
                }
                return Ok(());
            }
        }
    }

    let cli = Cli::parse();

    // Setup logging
    let log_level = if cli.verbose { "debug" } else { "warn" };
    let base_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(log_level));
    let log_filter = base_filter
        .add_directive("rmcp::service::client=error".parse().expect("valid rmcp directive"));
    tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .with_target(false)
        .without_time()
        .init();

    // Determine working directory
    let cwd = cli
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    debug!(cwd = %cwd.display(), "Starting Claurst");

    // Load settings from disk (hierarchical: global < project)
    let settings = Settings::load_hierarchical(&cwd).await;

    // Build effective config (CLI args override settings)
    let mut config = settings.effective_config();
    if let Some(ref key) = cli.api_key {
        config.api_key = Some(key.clone());
    }
    if let Some(ref m) = cli.model {
        config.model = Some(m.clone());
    }
    if let Some(mt) = cli.max_tokens {
        config.max_tokens = Some(mt);
    }
    config.verbose = cli.verbose;
    config.output_format = cli.output_format.into();
    config.disable_claude_mds = cli.no_claude_md;
    if let Some(sp) = cli.system_prompt.clone() {
        config.custom_system_prompt = Some(sp);
    }
    if let Some(asp) = cli.append_system_prompt.clone() {
        config.append_system_prompt = Some(asp);
    }
    if cli.dangerously_skip_permissions {
        // Mirror TS setup.ts: block bypass mode when running as root/sudo.
        #[cfg(unix)]
        if nix::unistd::Uid::effective().is_root() {
            anyhow::bail!(
                "--dangerously-skip-permissions cannot be used with root/sudo privileges for security reasons"
            );
        }
        config.permission_mode = PermissionMode::BypassPermissions;
    } else {
        config.permission_mode = cli.permission_mode.into();
    }
    config.additional_dirs = cli.add_dir.clone();
    if cli.no_auto_compact {
        config.auto_compact = false;
    }
    config.project_dir = Some(cwd.clone());
    if let Some(p) = &cli.provider {
        config.provider = Some(p.clone());
    }
    if let Some(base) = &cli.api_base {
        // Store in the provider's config entry
        let provider_id = config.provider.clone().unwrap_or_else(|| "anthropic".to_string());
        config
            .provider_configs
            .entry(provider_id)
            .or_default()
            .api_base = Some(base.clone());
    }

    // --dump-system-prompt fast path
    if cli.dump_system_prompt {
        let ctx = ContextBuilder::new(cwd.clone())
            .disable_claude_mds(config.disable_claude_mds);
        let sys = ctx.build_system_context().await;
        let user = ctx.build_user_context().await;
        println!("{}\n\n{}", sys, user);
        return Ok(());
    }

    // Build context
    let ctx_builder = ContextBuilder::new(cwd.clone())
        .disable_claude_mds(config.disable_claude_mds);
    let system_ctx = ctx_builder.build_system_context().await;
    let user_ctx = ctx_builder.build_user_context().await;

    // Build system prompt
    let mut system_parts = vec![
        include_str!("system_prompt.txt").to_string(),
        system_ctx,
        user_ctx,
    ];
    if let Some(ref custom) = config.custom_system_prompt {
        // replace base system prompt
        system_parts[0] = custom.clone();
    }
    if let Some(ref append) = config.append_system_prompt {
        system_parts.push(append.clone());
    }
    let system_prompt = system_parts.join("\n\n");

    // Determine mode early (needed for auth error handling and permission handler selection).
    let is_headless = cli.print || cli.prompt.is_some();

    // Initialize API client.
    // Try config/env first; fall back to saved OAuth tokens.
    // If no Anthropic credentials are found, check whether any other provider is
    // configured (OpenAI, Google, Ollama, Groq, etc.) — if so, proceed without
    // requiring Anthropic auth. Only launch the OAuth flow when Anthropic is
    // explicitly the intended provider and no key exists at all.
    let active_provider = config.selected_provider_id();
    let (api_key, use_bearer_auth) = if active_provider == "anthropic" {
        match config.resolve_anthropic_auth_async().await {
            Some(auth) => auth,
            None => {
                if is_headless {
                    anyhow::bail!(
                        "No API key found. Options:\n\
                         - Set ANTHROPIC_API_KEY for Anthropic\n\
                         - Set OPENAI_API_KEY for OpenAI\n\
                         - Set GOOGLE_API_KEY for Google Gemini\n\
                         - Set GROQ_API_KEY for Groq (fast, free tier available)\n\
                         - Run `claurst --provider ollama` for local models (no key needed)\n\
                         - Run `claurst auth login` for Anthropic OAuth"
                    );
                } else {
                    (String::new(), false)
                }
            }
        }
    } else {
        (String::new(), false)
    };

    let client_config = claurst_api::client::ClientConfig {
        api_key: api_key.clone(),
        api_base: config.resolve_anthropic_api_base(),
        use_bearer_auth,
        ..Default::default()
    };
    let client = Arc::new(
        claurst_api::AnthropicClient::new(client_config.clone())
            .context("Failed to create API client")?,
    );

    // Build provider registry: auto-registers all env-configured providers
    // AND providers with keys stored in ~/.claurst/auth.json (from /connect).
    // Anthropic is always the default; additional providers (OpenAI, Google,
    // Bedrock, Azure, Copilot, Cohere, local providers) are registered when
    // their respective environment variables or auth store entries are found.
    let provider_registry = claurst_api::ProviderRegistry::from_config(&config, client_config);

    let bridge_config = resolve_bridge_config(&settings, &api_key, use_bearer_auth, is_headless);
    if let Some(cfg) = bridge_config.as_ref() {
        info!(
            server_url = %cfg.server_url,
            startup_enabled = settings.remote_control_at_startup,
            "Remote control bridge configured for interactive startup"
        );
    }

    let permission_manager = Arc::new(std::sync::Mutex::new(PermissionManager::new(
        config.permission_mode.clone(),
        &settings,
    )));

    let permission_handler: Arc<dyn claurst_core::PermissionHandler> = if is_headless {
        Arc::new(AutoPermissionHandler::with_manager(permission_manager.clone()))
    } else {
        Arc::new(InteractivePermissionHandler::with_manager(permission_manager.clone()))
    };
    let cost_tracker = CostTracker::new();
    // Use --session-id if provided, otherwise generate a fresh UUID.
    let session_id = cli
        .session_id_flag
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let file_history = Arc::new(ParkingMutex::new(
        claurst_core::file_history::FileHistory::new(),
    ));
    let current_turn = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    // Initialize MCP servers first (needed for ToolContext.mcp_manager).
    let mcp_manager_arc = connect_mcp_manager_arc(&config).await;

    let pending_permissions = Arc::new(ParkingMutex::new(claurst_tools::PendingPermissionStore::default()));

    let tool_ctx = ToolContext {
        working_dir: cwd.clone(),
        permission_mode: config.permission_mode.clone(),
        permission_handler: permission_handler.clone(),
        cost_tracker: cost_tracker.clone(),
        session_id: session_id.clone(),
        file_history: file_history.clone(),
        current_turn: current_turn.clone(),
        non_interactive: cli.print || cli.prompt.is_some(),
        mcp_manager: mcp_manager_arc.clone(),
        config: config.clone(),
        managed_agent_config: config.managed_agents.clone(),
        completion_notifier: None,
        pending_permissions: Some(pending_permissions.clone()),
        permission_manager: Some(permission_manager.clone()),
    };

    // Register the cc-query-backed agent runner so TeamCreateTool can spawn real
    // sub-agents.  Must be called before any tool execution begins.
    // The function is idempotent if already registered (panics only on double-call,
    // but we guard with a std::sync::OnceLock internally).
    {
        static SWARM_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        SWARM_INIT.get_or_init(|| claurst_query::init_team_swarm_runner());
    }

    // Build the full tool list: built-ins from cc-tools plus AgentTool from cc-query
    // (AgentTool lives in cc-query to avoid a circular cc-tools ↔ cc-query dependency).
    // Wrap in Arc so the list can be shared by the main loop AND the cron scheduler.
    let tools = build_tools_with_mcp(mcp_manager_arc.clone());

    // Load plugins and register any plugin-provided MCP servers into the
    // in-memory config (does not modify the settings file on disk).
    let plugin_registry = claurst_plugins::load_plugins(&cwd, &[]).await;
    {
        let plugin_cmd_count = plugin_registry.all_command_defs().len();
        let plugin_hook_count = plugin_registry
            .build_hook_registry()
            .values()
            .map(|v| v.len())
            .sum::<usize>();
        info!(
            plugins = plugin_registry.enabled_count(),
            commands = plugin_cmd_count,
            hooks = plugin_hook_count,
            "Plugins loaded"
        );

        // Register plugin MCP servers into the in-memory config so they are
        // picked up by any subsequent MCP manager construction.
        let existing_names: std::collections::HashSet<String> = config
            .mcp_servers
            .iter()
            .map(|s| s.name.clone())
            .collect();
        for mcp_server in plugin_registry.all_mcp_servers() {
            if !existing_names.contains(&mcp_server.name) {
                config.mcp_servers.push(mcp_server);
            }
        }
    }

    // Build model registry for dynamic model/provider resolution.
    // The registry is pre-populated with a hardcoded snapshot and enriched
    // from the models.dev cache if available.
    let model_registry = load_cached_model_registry();

    // Build query config
    let mut query_config = claurst_query::QueryConfig::from_config_with_registry(&config, &model_registry);
    query_config.model_registry = Some(model_registry.clone());
    query_config.max_turns = cli.max_turns;
    query_config.system_prompt = Some(system_prompt);
    query_config.append_system_prompt = None;
    query_config.working_directory = Some(cwd.display().to_string());
    if let Some(tokens) = cli.thinking {
        query_config.thinking_budget = Some(tokens);
    }
    if let Some(ref level_str) = cli.effort {
        if let Some(level) = claurst_core::effort::EffortLevel::from_str(level_str) {
            query_config.effort_level = Some(level);
        } else {
            eprintln!("Warning: unknown effort level '{}' — expected low/medium/high/max", level_str);
        }
    }
    if let Some(usd) = cli.max_budget_usd {
        query_config.max_budget_usd = Some(usd);
    }
    if let Some(ref fb) = cli.fallback_model {
        query_config.fallback_model = Some(fb.clone());
    }
    // Wire in the provider registry so non-Anthropic providers can be dispatched.
    let provider_registry = std::sync::Arc::new(provider_registry);
    query_config.provider_registry = Some(provider_registry.clone());

    // Wire in the named agent (--agent flag).
    // Merge built-in default agents with user-defined agents (user wins on collision).
    let tools = if let Some(ref agent_name) = cli.agent {
        query_config.agent_name = Some(agent_name.clone());
        let mut all_agents = claurst_core::default_agents();
        all_agents.extend(config.agents.clone());
        if let Some(def) = all_agents.get(agent_name) {
            let access = def.access.clone();
            query_config.agent_definition = Some(def.clone());
            // Override max_turns from agent definition when specified.
            if let Some(turns) = def.max_turns {
                query_config.max_turns = turns;
            }
            filter_tools_for_agent(tools, &access)
        } else {
            eprintln!("Warning: unknown agent '{}'. Run /agent to see available agents.", agent_name);
            tools
        }
    } else {
        tools
    };

    // Spawn the background cron scheduler (fires cron tasks at scheduled times).
    // Cancelled automatically when the process exits since we use a shared token.
    let cron_cancel = tokio_util::sync::CancellationToken::new();
    claurst_query::start_cron_scheduler(
        client.clone(),
        tools.clone(),
        tool_ctx.clone(),
        query_config.clone(),
        cron_cancel.clone(),
    );

    // --print mode (headless)
    let result = if is_headless {
        run_headless(
            &cli,
            client,
            tools,
            tool_ctx,
            query_config,
            cost_tracker,
        )
        .await
    } else {
        let auth_store = claurst_core::AuthStore::load();
        let has_saved_credentials = !auth_store.credentials.is_empty()
            || claurst_core::oauth_config::get_codex_tokens().is_some();
        let has_credentials = !api_key.is_empty()
            || has_saved_credentials
            || config.provider.as_deref().is_some_and(|p| p != "anthropic");
        run_interactive(
            config,
            settings,
            client,
            tools,
            tool_ctx,
            query_config,
            cost_tracker,
            cli.resume,
            bridge_config,
            has_credentials,
            model_registry,
        )
        .await
    };

    cron_cancel.cancel();
    result
}

async fn connect_mcp_manager_arc(
    config: &Config,
) -> Option<Arc<claurst_mcp::McpManager>> {
    if config.mcp_servers.is_empty() {
        return None;
    }

    info!(count = config.mcp_servers.len(), "Connecting to MCP servers");
    let mcp_manager = Arc::new(claurst_mcp::McpManager::connect_all(&config.mcp_servers).await);
    mcp_manager.clone().spawn_notification_poll_loop();
    Some(mcp_manager)
}

fn build_tools_with_mcp(
    mcp_manager: Option<Arc<claurst_mcp::McpManager>>,
) -> Arc<Vec<Box<dyn claurst_tools::Tool>>> {
    let mut v: Vec<Box<dyn claurst_tools::Tool>> = claurst_tools::all_tools();
    v.push(Box::new(claurst_query::AgentTool));

    if let Some(ref manager_arc) = mcp_manager {
        for (server_name, tool_def) in manager_arc.all_tool_definitions() {
            let wrapper = McpToolWrapper {
                tool_def,
                server_name,
                manager: manager_arc.clone(),
            };
            v.push(Box::new(wrapper));
        }
        debug!(total_tools = v.len(), "MCP tools registered");
    }

    Arc::new(v)
}

fn model_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claurst")
}

fn models_cache_path() -> PathBuf {
    model_cache_dir().join("models.json")
}

fn models_dev_cache_path() -> PathBuf {
    model_cache_dir().join("models_dev.json")
}

fn load_cached_model_registry() -> Arc<claurst_api::ModelRegistry> {
    let mut reg = claurst_api::ModelRegistry::new();
    reg.load_cache(&models_cache_path());
    Arc::new(reg)
}

fn spawn_models_cache_refresh() {
    let cache_paths = vec![models_cache_path(), models_dev_cache_path()];
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        let url = std::env::var("MODELS_DEV_URL")
            .unwrap_or_else(|_| "https://models.dev/api.json".to_string());
        if let Ok(resp) = client
            .get(&url)
            .header("User-Agent", "Claurst/0.0.9")
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(text) = resp.text().await {
                    if let Some(parent) = cache_paths[0].parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    for path in &cache_paths {
                        let _ = std::fs::write(path, &text);
                    }
                    tracing::info!("Models cache refreshed from models.dev");
                }
            }
        }
    });
}

async fn remove_file_if_exists(path: &std::path::Path) -> anyhow::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

struct RefreshedProviderRuntime {
    config: Config,
    client: Arc<claurst_api::AnthropicClient>,
    provider_registry: Arc<claurst_api::ProviderRegistry>,
    model_registry: Arc<claurst_api::ModelRegistry>,
    auth_store: claurst_core::AuthStore,
}

async fn refresh_provider_runtime_state(
    current_config: &Config,
) -> anyhow::Result<RefreshedProviderRuntime> {
    remove_file_if_exists(&claurst_core::AuthStore::path())
        .await
        .context("Failed to clear auth store")?;
    remove_file_if_exists(&claurst_core::oauth::OAuthTokens::token_file_path())
        .await
        .context("Failed to clear OAuth token cache")?;
    remove_file_if_exists(&models_cache_path())
        .await
        .context("Failed to clear model cache")?;
    remove_file_if_exists(&models_dev_cache_path())
        .await
        .context("Failed to clear legacy model cache")?;

    let mut settings = Settings::load()
        .await
        .context("Failed to load settings for /refresh")?;
    settings.provider = None;
    settings.config.provider = None;
    settings.config.model = None;
    settings.config.api_key = None;
    settings
        .save()
        .await
        .context("Failed to save refreshed settings")?;

    let mut config = current_config.clone();
    config.api_key = None;
    config.provider = None;
    config.model = None;

    let (api_key, use_bearer_auth) = config
        .resolve_anthropic_auth_async()
        .await
        .unwrap_or((String::new(), false));
    let client_config = claurst_api::client::ClientConfig {
        api_key,
        api_base: config.resolve_anthropic_api_base(),
        use_bearer_auth,
        ..Default::default()
    };
    let client = Arc::new(
        claurst_api::AnthropicClient::new(client_config.clone())
            .context("Failed to rebuild Anthropic client")?,
    );
    let provider_registry =
        Arc::new(claurst_api::ProviderRegistry::from_config(&config, client_config));
    let model_registry = load_cached_model_registry();

    spawn_models_cache_refresh();

    Ok(RefreshedProviderRuntime {
        config,
        client,
        provider_registry,
        model_registry,
        auth_store: claurst_core::AuthStore::default(),
    })
}

fn normalize_provider_from_model(config: &mut Config) {
    if let Some(model) = config.model.as_deref() {
        if let Some((provider, _)) = model.split_once('/') {
            config.provider = Some(provider.to_string());
        }
    }
}

/// Filter the tool list based on the agent's access level.
/// - "full"        → all tools allowed (no filtering)
/// - "read-only"   → only ReadOnly/None permission tools and AskUserQuestion
/// - "search-only" → only Grep, Glob, Read, WebSearch, WebFetch tools
fn filter_tools_for_agent(
    tools: Arc<Vec<Box<dyn claurst_tools::Tool>>>,
    access: &str,
) -> Arc<Vec<Box<dyn claurst_tools::Tool>>> {
    use claurst_tools::PermissionLevel as PL;
    match access {
        "read-only" => {
            // Collect names of tools that are read-only, then rebuild from all_tools
            // (Box<dyn Tool> is not Clone so we can't directly filter-and-keep).
            let allowed_names: Vec<String> = tools
                .iter()
                .filter(|t| {
                    matches!(t.permission_level(), PL::ReadOnly | PL::None)
                        || t.name() == "AskUserQuestion"
                })
                .map(|t| t.name().to_string())
                .collect();
            let filtered: Vec<Box<dyn claurst_tools::Tool>> = claurst_tools::all_tools()
                .into_iter()
                .filter(|t| allowed_names.iter().any(|n| n == t.name()))
                .collect();
            Arc::new(filtered)
        }
        "search-only" => {
            const SEARCH_TOOLS: &[&str] = &["Grep", "Glob", "Read", "WebSearch", "WebFetch"];
            let filtered: Vec<Box<dyn claurst_tools::Tool>> = claurst_tools::all_tools()
                .into_iter()
                .filter(|t| SEARCH_TOOLS.contains(&t.name()))
                .collect();
            Arc::new(filtered)
        }
        _ => tools, // "full" — allow all tools unchanged
    }
}

// ---------------------------------------------------------------------------
// Headless mode: read prompt from arg/stdin, run, print response
// ---------------------------------------------------------------------------

async fn run_headless(
    cli: &Cli,
    client: Arc<claurst_api::AnthropicClient>,
    tools: Arc<Vec<Box<dyn claurst_tools::Tool>>>,
    tool_ctx: ToolContext,
    query_config: claurst_query::QueryConfig,
    cost_tracker: Arc<CostTracker>,
) -> anyhow::Result<()> {
    use claurst_query::{QueryEvent, QueryOutcome};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    // Build initial messages list from input.
    // --input-format stream-json: stdin is newline-delimited JSON, each line is
    //   {"role":"user"|"assistant","content":"..."} (mirrors TS --input-format stream-json).
    // --input-format text (default): read prompt from positional arg or entire stdin as text.
    let mut messages: Vec<claurst_core::types::Message> = if cli.input_format == CliInputFormat::StreamJson {
        use tokio::io::{self, AsyncBufReadExt, BufReader};
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        let mut parsed: Vec<claurst_core::types::Message> = Vec::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(v) => {
                    let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("user");
                    let content = v
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    if role == "assistant" {
                        parsed.push(claurst_core::types::Message::assistant(content));
                    } else {
                        parsed.push(claurst_core::types::Message::user(content));
                    }
                }
                Err(e) => {
                    eprintln!("Warning: skipping malformed JSON line: {} ({:?})", trimmed, e);
                }
            }
        }
        if parsed.is_empty() {
            // Also check positional arg as fallback
            if let Some(ref p) = cli.prompt {
                parsed.push(claurst_core::types::Message::user(p.clone()));
            }
        }
        parsed
    } else {
        // Plain text mode
        let prompt = if let Some(ref p) = cli.prompt {
            p.clone()
        } else {
            use tokio::io::{self, AsyncReadExt};
            let mut stdin = io::stdin();
            let mut buf = String::new();
            stdin.read_to_string(&mut buf).await?;
            buf.trim().to_string()
        };

        if prompt.is_empty() {
            eprintln!("Error: No prompt provided. Use --print <prompt> or pipe text to stdin.");
            std::process::exit(1);
        }

        vec![claurst_core::types::Message::user(prompt)]
    };

    // --prefill: inject a partial assistant turn before the query so the model
    // continues from that text (mirrors TS --prefill flag).
    if let Some(ref prefill_text) = cli.prefill {
        messages.push(claurst_core::types::Message::assistant(prefill_text.clone()));
    }

    if messages.is_empty() {
        eprintln!("Error: No messages provided.");
        std::process::exit(1);
    }

    let is_json_output = matches!(cli.output_format, CliOutputFormat::Json | CliOutputFormat::StreamJson);
    let is_stream_json = matches!(cli.output_format, CliOutputFormat::StreamJson);

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<QueryEvent>();
    let cancel = CancellationToken::new();
    let client_clone = client.clone();
    let tool_ctx_clone = tool_ctx.clone();
    let qcfg = query_config.clone();
    let tracker_clone = cost_tracker.clone();
    let event_tx_clone = event_tx.clone();
    let cancel_clone = cancel.clone();

    let query_handle = tokio::spawn(async move {
        claurst_query::run_query_loop(
            client_clone.as_ref(),
            &mut messages,
            tools.as_slice(),
            &tool_ctx_clone,
            &qcfg,
            tracker_clone,
            Some(event_tx_clone),
            cancel_clone,
            None,
        )
        .await
    });

    // Drop the original tx so the channel closes when the task drops its clone
    drop(event_tx);

    // Drain events and print streaming text
    let mut full_text = String::new();

    while let Some(event) = event_rx.recv().await {
        match &event {
            QueryEvent::Stream(claurst_api::AnthropicStreamEvent::ContentBlockDelta {
                delta: claurst_api::streaming::ContentDelta::TextDelta { text },
                ..
            }) => {
                full_text.push_str(text);
                if !is_json_output {
                    print!("{}", text);
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                } else if is_stream_json {
                    let chunk = serde_json::json!({ "type": "text_delta", "text": text });
                    println!("{}", chunk);
                }
            }
            QueryEvent::ToolStart { tool_name, .. } => {
                if !is_json_output {
                    eprintln!("\n[{}...]", tool_name);
                } else {
                    let ev = serde_json::json!({ "type": "tool_start", "tool": tool_name });
                    println!("{}", ev);
                }
            }
            QueryEvent::Error(msg) => {
                if is_json_output {
                    let ev = serde_json::json!({ "type": "error", "error": msg });
                    eprintln!("{}", ev);
                } else {
                    eprintln!("\nError: {}", msg);
                }
            }
            _ => {}
        }
    }

    // Wait for the query task to finish and get the final outcome
    let outcome = query_handle.await.unwrap_or(QueryOutcome::Error(
        claurst_core::error::ClaudeError::Other("Query task panicked".to_string()),
    ));

    // Final output
    match cli.output_format {
        CliOutputFormat::Json => {
            match outcome {
                QueryOutcome::EndTurn { message, usage } => {
                    let result_text = if full_text.is_empty() {
                        message.get_all_text()
                    } else {
                        full_text
                    };
                    let out = serde_json::json!({
                        "type": "result",
                        "result": result_text,
                        "usage": {
                            "input_tokens": usage.input_tokens,
                            "output_tokens": usage.output_tokens,
                            "cache_creation_input_tokens": usage.cache_creation_input_tokens,
                            "cache_read_input_tokens": usage.cache_read_input_tokens,
                        },
                        "cost_usd": cost_tracker.total_cost_usd(),
                    });
                    println!("{}", out);
                }
                QueryOutcome::Error(e) => {
                    let out = serde_json::json!({ "type": "error", "error": e.to_string() });
                    eprintln!("{}", out);
                    std::process::exit(1);
                }
                _ => {}
            }
        }
        CliOutputFormat::StreamJson => {
            // Already streamed above; emit final result event
            match outcome {
                QueryOutcome::EndTurn { usage, .. } => {
                    let out = serde_json::json!({
                        "type": "result",
                        "usage": {
                            "input_tokens": usage.input_tokens,
                            "output_tokens": usage.output_tokens,
                        },
                        "cost_usd": cost_tracker.total_cost_usd(),
                    });
                    println!("{}", out);
                }
                QueryOutcome::Error(e) => {
                    let out = serde_json::json!({ "type": "error", "error": e.to_string() });
                    eprintln!("{}", out);
                    std::process::exit(1);
                }
                _ => {}
            }
        }
        CliOutputFormat::Text => {
            // Streaming text was already printed; add newline
            println!();
            if cli.verbose {
                eprintln!(
                    "\nTokens: {} in / {} out | Cost: ${:.4}",
                    cost_tracker.input_tokens(),
                    cost_tracker.output_tokens(),
                    cost_tracker.total_cost_usd(),
                );
            }
            match outcome {
                QueryOutcome::Error(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                QueryOutcome::BudgetExceeded { cost_usd, limit_usd } => {
                    eprintln!(
                        "Budget limit ${:.4} reached (spent ${:.4}). Stopping.",
                        limit_usd, cost_usd
                    );
                    std::process::exit(2);
                }
                _ => {}
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Interactive REPL mode
// ---------------------------------------------------------------------------

fn permission_request_from_core(
    pending: &claurst_tools::PendingPermissionRequest,
) -> claurst_tui::dialogs::PermissionRequest {
    let reason = pending.reason.clone();
    let tool_name = pending.request.tool_name.clone();
    let tool_use_id = pending.tool_use_id.clone();

    match (tool_name.as_str(), pending.request.path.clone()) {
        ("Bash", Some(command)) => {
            let suggested_prefix = command
                .split_whitespace()
                .next()
                .filter(|prefix| !prefix.is_empty())
                .map(|prefix| format!("{} ", prefix));
            claurst_tui::dialogs::PermissionRequest::bash(
                tool_use_id,
                tool_name,
                reason,
                command,
                suggested_prefix,
            )
        },
        ("PowerShell", Some(command)) => claurst_tui::dialogs::PermissionRequest::powershell(
            tool_use_id,
            tool_name,
            reason,
            command,
        ),
        ("Read", Some(path)) => claurst_tui::dialogs::PermissionRequest::file_read(
            tool_use_id,
            tool_name,
            reason,
            path,
        ),
        (_, Some(path)) if matches!(tool_name.as_str(), "Write" | "Edit" | "NotebookEdit") => {
            claurst_tui::dialogs::PermissionRequest::file_write(tool_use_id, tool_name, reason, path)
        }
        _ => claurst_tui::dialogs::PermissionRequest::from_reason(
            tool_use_id,
            tool_name,
            reason,
            pending.request.path.clone(),
        ),
    }
}


async fn run_interactive(
    config: Config,
    settings: claurst_core::config::Settings,
    client: Arc<claurst_api::AnthropicClient>,
    tools: Arc<Vec<Box<dyn claurst_tools::Tool>>>,
    tool_ctx: ToolContext,
    query_config: claurst_query::QueryConfig,
    cost_tracker: Arc<CostTracker>,
    resume_id: Option<String>,
    bridge_config: Option<claurst_bridge::BridgeConfig>,
    has_credentials: bool,
    model_registry: Arc<claurst_api::ModelRegistry>,
) -> anyhow::Result<()> {
    use claurst_commands::{execute_command, CommandContext, CommandResult};
    use claurst_bridge::{BridgeOutbound, TuiBridgeEvent};
    use claurst_query::{QueryEvent, QueryOutcome};
    use claurst_tui::{
        bridge_state::BridgeConnectionState, notifications::NotificationKind,
        render::render_app, restore_terminal, setup_terminal, App,
        device_auth_dialog::DeviceAuthEvent,
    };
    use crossterm::event::{self, Event, KeyCode};
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    let mut client = client;
    let mut model_registry = model_registry;
    let mut tool_ctx = tool_ctx;
    let mut session = if let Some(ref id) = resume_id {
        match claurst_core::history::load_session(id).await {
            Ok(session) => {
                println!("Resumed session: {}", id);
                if let Some(saved_dir) = session.working_dir.as_ref() {
                    let saved_path = std::path::PathBuf::from(saved_dir);
                    if saved_path.exists() {
                        tool_ctx.working_dir = saved_path;
                    }
                }
                tool_ctx.session_id = session.id.clone();
                session
            }
            Err(e) => {
                eprintln!("Warning: could not load session {}: {}", id, e);
                let mut session =
                    claurst_core::history::ConversationSession::new(
                        claurst_api::effective_model_for_config(&config, &model_registry),
                    );
                session.id = tool_ctx.session_id.clone();
                session.working_dir = Some(tool_ctx.working_dir.display().to_string());
                session
            }
        }
    } else {
        let mut session =
            claurst_core::history::ConversationSession::new(
                claurst_api::effective_model_for_config(&config, &model_registry),
            );
        session.id = tool_ctx.session_id.clone();
        session.working_dir = Some(tool_ctx.working_dir.display().to_string());
        session
    };
    let initial_messages = session.messages.clone();
    let mut base_query_config = query_config;
    let mut live_config = config.clone();
    if !session.model.is_empty() {
        live_config.model = Some(session.model.clone());
    }
    let pending_permissions = tool_ctx
        .pending_permissions
        .clone()
        .unwrap_or_else(|| Arc::new(ParkingMutex::new(claurst_tools::PendingPermissionStore::default())));


    // Set up terminal
    let mut terminal = setup_terminal()?;
    let mut app = App::new(live_config.clone(), cost_tracker.clone());
    // Sync initial effort level (from --effort flag or /effort command) to TUI indicator.
    if let Some(level) = base_query_config.effort_level {
        use claurst_tui::EffortLevel as TuiEL;
        app.effort_level = match level {
            claurst_core::effort::EffortLevel::Low    => TuiEL::Low,
            claurst_core::effort::EffortLevel::Medium => TuiEL::Normal,
            claurst_core::effort::EffortLevel::High   => TuiEL::High,
            claurst_core::effort::EffortLevel::Max    => TuiEL::Max,
        };
    }
    app.provider_registry = base_query_config.provider_registry.clone();
    app.refresh_context_window_size();
    app.auto_compact_enabled = live_config.auto_compact;

    // Background: refresh the model registry from models.dev.
    // The fetched JSON is saved as a cache file; the App will reload it from
    // disk whenever the /model picker opens.
    {
        spawn_models_cache_refresh();
    }

    app.config.project_dir = Some(tool_ctx.working_dir.clone());
    app.attach_turn_diff_state(tool_ctx.file_history.clone(), tool_ctx.current_turn.clone());
    if let Some(manager) = tool_ctx.mcp_manager.clone() {
        app.attach_mcp_manager(manager);
    }
    app.replace_messages(initial_messages.clone());

    // Home directory warning: mirror TS feedConfigs.tsx warningText
    let home_dir = dirs::home_dir();
    if home_dir.as_deref() == Some(tool_ctx.working_dir.as_path()) {
        app.home_dir_warning = true;
    }

    // Bypass permissions confirmation dialog: must be accepted before any work
    // Mark whether valid credentials exist so the TUI can show a provider
    // setup dialog instead of failing silently on the first message.
    app.has_credentials = has_credentials;

    // If a non-Anthropic provider is active, prefix model_name with "provider/model"
    // so the status bar can show the provider name.
    if let Some(ref provider) = live_config.provider {
        if provider != "anthropic" && !app.model_name.contains('/') {
            app.model_name = format!("{}/{}", provider, app.model_name);
        }
    }

    // Set agent mode from the --agent flag (carried on query_config).
    if let Some(ref agent_name) = base_query_config.agent_name {
        app.agent_mode = Some(agent_name.clone());
    }

    // Show onboarding: status hint if no credentials, welcome tour if first run.
    // Skip the welcome tour entirely if the user already has credentials — they've
    // clearly set things up (via /connect or env vars) and don't need onboarding.
    if !has_credentials {
        if !settings.has_completed_onboarding {
            app.onboarding_dialog.show();
        } else {
            app.status_message = Some("No provider configured. Run /connect to set one up.".to_string());
        }
    } else if !settings.has_completed_onboarding {
        // User has credentials but hasn't formally completed onboarding — mark it done
        // silently so they never see it.
        let _ = claurst_tui::App::persist_onboarding_complete_pub();
    }

    // Mirror TS BypassPermissionsModeDialog.tsx startup gate
    use claurst_core::config::PermissionMode;
    if live_config.permission_mode == PermissionMode::BypassPermissions {
        app.bypass_permissions_dialog.show();
    }

    // Version-upgrade notice: record the current version for future comparisons.
    // (Actual upgrade notice UI is handled by the release-notes slash command.)
    {
        let current_version = claurst_core::constants::APP_VERSION.to_string();
        if settings.last_seen_version.as_deref() != Some(&current_version) {
            // Persist asynchronously to avoid blocking startup.
            let version_clone = current_version.clone();
            tokio::spawn(async move {
                if let Ok(mut s) = claurst_core::config::Settings::load().await {
                    s.last_seen_version = Some(version_clone);
                    let _ = s.save().await;
                }
            });
        }
    }

    // CLAUDE_STATUS_COMMAND: optional external command whose stdout replaces the
    // left-side status bar text. Polled every 500ms (debounced) in the main loop.
    // The command is run in a background task; results flow through a channel.
    let status_cmd_str = std::env::var("CLAUDE_STATUS_COMMAND").ok();
    let (status_cmd_tx, mut status_cmd_rx) = mpsc::channel::<String>(4);
    if let Some(ref cmd_str) = status_cmd_str {
        let cmd_str = cmd_str.clone();
        let tx = status_cmd_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                // Run via shell so pipes/redirects in the command string work.
                let output = if cfg!(target_os = "windows") {
                    tokio::process::Command::new("cmd")
                        .args(["/C", &cmd_str])
                        .output()
                        .await
                } else {
                    tokio::process::Command::new("sh")
                        .args(["-c", &cmd_str])
                        .output()
                        .await
                };
                if let Ok(out) = output {
                    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    let _ = tx.try_send(text);
                }
            }
        });
    }

    // Bridge runtime channels — Some when bridge is configured and started.
    //
    // tui_rx:       TUI-facing events from the bridge worker (connect/disconnect/prompts)
    // outbound_tx:  Forward query events to the bridge worker for upload to server
    // bridge_cancel: CancellationToken to stop the bridge worker task
    struct BridgeRuntime {
        tui_rx: mpsc::Receiver<TuiBridgeEvent>,
        outbound_tx: mpsc::Sender<BridgeOutbound>,
        cancel: CancellationToken,
    }

    // Preserve the bridge token before consuming bridge_config so we can reconstruct
    // a BridgeSessionInfo once the bridge worker reports it has connected.
    let bridge_token: Option<String> = bridge_config
        .as_ref()
        .and_then(|c| c.session_token.clone());

    let mut bridge_runtime: Option<BridgeRuntime> = if let Some(cfg) = bridge_config {
        let bridge_cancel = CancellationToken::new();
        let (tui_tx, tui_rx) = mpsc::channel::<TuiBridgeEvent>(64);
        let (outbound_tx, outbound_rx) = mpsc::channel::<BridgeOutbound>(256);

        // Update TUI state to "connecting" before the task starts.
        app.bridge_state = BridgeConnectionState::Connecting;

        let cancel_clone = bridge_cancel.clone();
        tokio::spawn(async move {
            if let Err(e) = claurst_bridge::run_bridge_loop(cfg, tui_tx, outbound_rx, cancel_clone).await {
                warn!("Bridge loop exited with error: {}", e);
            }
        });

        Some(BridgeRuntime {
            tui_rx,
            outbound_tx,
            cancel: bridge_cancel,
        })
    } else {
        None
    };

    // Relay channels for the BridgeSessionInfo-based event path.
    //
    // relay_ev_tx:    receives serialised JSON event payloads from the query-event
    //                 drain loop; a background task consumes them and calls
    //                 post_bridge_event so the web UI sees live streaming events.
    // relay_ev_rx_opt: Option wrapper so we can move the Receiver into the relay
    //                 task exactly once when the bridge session comes online.
    // remote_prompt_tx/rx: inbound user messages polled from poll_bridge_messages
    //                 are delivered here; the main loop injects them as query turns.
    let (relay_ev_tx, relay_ev_rx) = mpsc::channel::<String>(256);
    let mut relay_ev_rx_opt: Option<mpsc::Receiver<String>> = Some(relay_ev_rx);
    let (remote_prompt_tx, mut remote_prompt_rx) = mpsc::channel::<String>(32);

    // Once the bridge worker reports Connected we build this from the session
    // credentials so both relay tasks can POST/poll the /api/bridge/sessions API.
    let mut bridge_session_info: Option<std::sync::Arc<claurst_bridge::BridgeSessionInfo>> = None;

    let mut messages = initial_messages;
    let mut cmd_ctx = CommandContext {
        config: live_config,
        cost_tracker: cost_tracker.clone(),
        messages: messages.clone(),
        working_dir: tool_ctx.working_dir.clone(),
        session_id: session.id.clone(),
        session_title: session.title.clone(),
        remote_session_url: session.remote_session_url.clone(),
        mcp_manager: tool_ctx.mcp_manager.clone(),
        mcp_auth_runner: None,
    };

    // tools is already Arc<Vec<...>> — share it across spawned tasks without copying.
    // Keep the full unfiltered tool set so agent-mode switching can re-filter.
    let all_tools_arc: Arc<Vec<Box<dyn claurst_tools::Tool>>> =
        Arc::new(claurst_tools::all_tools());
    let mut tools_arc = tools;

    // Current cancel token (replaced each turn)
    let mut cancel: Option<CancellationToken> = None;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<QueryEvent>();
    type MessagesArc = Arc<tokio::sync::Mutex<Vec<claurst_core::types::Message>>>;
    let mut current_query: Option<(tokio::task::JoinHandle<QueryOutcome>, MessagesArc)> = None;
    // Active effort level (None = use model default / High).
    // Tracks the user's /effort selection; flows into qcfg each turn.
    let mut current_effort: Option<claurst_core::effort::EffortLevel> = None;
    // Timestamp of when the most recent query turn was dispatched (for goal elapsed tracking).
    let mut goal_turn_start: std::time::Instant = std::time::Instant::now();

    // Background update check: spawned once at startup; result delivered via channel.
    let (update_tx, mut update_rx) = tokio::sync::mpsc::channel::<Option<String>>(1);
    tokio::spawn(async move {
        let info = claurst_core::check_for_updates().await;
        let version = info.map(|i| i.latest_version);
        let _ = update_tx.send(version).await;
    });

    // Device code / OAuth auth channel — background tasks send events here
    // so the main loop can update the device_auth_dialog state.
    let (device_auth_tx, mut device_auth_rx) = mpsc::channel::<DeviceAuthEvent>(8);

    // MCP OAuth auth channel — background tasks send events here so the main
    // loop can update status and trigger a reconnect after browser auth finishes.
    enum McpAuthEvent {
        /// Browser auth completed and the token was persisted successfully.
        Completed(claurst_mcp::oauth::McpAuthResult),
        /// Browser auth or token exchange failed.
        Failed(String),
    }
    let (mcp_auth_tx, mut mcp_auth_rx) = mpsc::channel::<McpAuthEvent>(8);
    // Build a non-blocking runner so `/mcp auth` can return immediately while
    // the browser flow continues in the background.
    let mcp_auth_runner: Arc<dyn Fn(claurst_mcp::oauth::McpAuthSession) + Send + Sync> = {
        let tx = mcp_auth_tx.clone();
        Arc::new(move |session| {
            let tx = tx.clone();
            tokio::spawn(async move {
                let event = match claurst_mcp::oauth::run_mcp_auth_session(session).await {
                    Ok(result) => McpAuthEvent::Completed(result),
                    Err(err) => McpAuthEvent::Failed(err.to_string()),
                };
                let _ = tx.send(event).await;
            });
        })
    };
    cmd_ctx.mcp_auth_runner = Some(mcp_auth_runner.clone());
    'main: loop {
        app.frame_count = app.frame_count.wrapping_add(1);
        app.tick_rustle_pose();
        app.notifications.tick();

        // Draw the UI
        terminal.draw(|f| render_app(f, &app))?;

        // Poll for crossterm events (keyboard/mouse) with short timeout
        if crossterm::event::poll(Duration::from_millis(16))? {
            let evt = event::read()?;
            match evt {
                Event::Key(key) => {
                    // On Windows crossterm emits Press + Release for a single key.
                    // Only process Press to avoid double-registering input.
                    if key.kind != crossterm::event::KeyEventKind::Press {
                        continue;
                    }

                    // Ctrl+C: copy selected text if there's a selection, otherwise cancel/quit
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                    {
                        // Check if there's an active text selection — copy instead of cancel/quit
                        let has_selection = app.selection_anchor.is_some() && !app.selection_text.borrow().is_empty();
                        if has_selection {
                            // Let the app handle the copy via its normal key handler
                            app.handle_key_event(key);
                            continue;
                        }

                        // No selection — handle as cancel (if streaming) or quit
                        if app.is_streaming {
                            if let Some(ref ct) = cancel {
                                ct.cancel();
                            }
                            app.is_streaming = false;
                            app.status_message = Some("Cancelled.".to_string());
                            continue;
                        } else {
                            break 'main;
                        }
                    }

                    // Ctrl+D on empty input => quit
                    if key.code == KeyCode::Char('d')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        && app.prompt_input.is_empty()
                    {
                        break 'main;
                    }

                    // Enter => submit input (but NOT when ANY dialog/overlay is open —
                    // dialogs handle their own Enter in handle_key_event).
                    let any_dialog_open = app.connect_dialog.visible
                        || app.import_config_picker.visible
                        || app.import_config_dialog.visible
                        || app.key_input_dialog.visible
                        || app.custom_provider_dialog.visible
                        || app.device_auth_dialog.visible
                        || app.command_palette.visible
                        || app.model_picker.visible
                        || app.onboarding_dialog.visible
                        || app.bypass_permissions_dialog.visible
                        || app.settings_screen.visible
                        || app.export_dialog.visible
                        || app.theme_screen.visible
                        || app.privacy_screen.visible
                        || app.stats_dialog.open
                        || app.invalid_config_dialog.visible
                        || app.context_viz.visible
                        || app.mcp_approval.visible
                        || app.session_browser.visible
                        || app.session_branching.visible
                        || app.tasks_overlay.visible
                        || app.mcp_view.open
                        || app.agents_menu.open
                        || app.diff_viewer.open
                        || app.help_overlay.visible
                        || app.history_search_overlay.visible
                        || app.rewind_flow.visible
                        || app.show_help
                        || app.context_menu_state.is_some()
                        || app.permission_request.is_some()
                        || app.global_search.open;
                    if key.code == KeyCode::Enter && !app.is_streaming && !any_dialog_open {
                        // If a slash-command suggestion is active, accept and execute immediately.
                        if !app.prompt_input.suggestions.is_empty()
                            && app.prompt_input.suggestion_index.is_some()
                            && app.prompt_input.text.starts_with('/')
                        {
                            app.prompt_input.accept_suggestion();
                            // Fall through to submit — no second Enter needed
                        }

                        let input = app.take_input();
                        if input.is_empty() {
                            continue;
                        }

                        // Check for slash command
                        if input.starts_with('/') {
                            let (cmd_name, cmd_args) =
                                claurst_tui::input::parse_slash_command(&input);
                            let cmd_name = cmd_name.to_string();
                            let cmd_args = cmd_args.to_string();

                            // ── Step 1: TUI-layer intercept (overlays, toggles) ────────
                            // Run first so we know whether a UI overlay opened, which
                            // lets us suppress redundant CLI text output below.
                            //
                            // Skip TUI overlay for arg-bearing commands where the user
                            // wants to SET state, not browse a picker:
                            //   /model claude-haiku  → set model, don't open picker
                            //   /theme dark          → set theme, don't open picker
                            //   /resume <id>         → load session, don't open browser
                            // Also skip TUI for /vim, /voice, /fast with explicit
                            // on|off args so the blind-toggle doesn't misfire.
                            let skip_tui_for_args = !cmd_args.is_empty()
                                && matches!(
                                    cmd_name.as_str(),
                                    "model" | "theme" | "resume" | "session"
                                        | "vim" | "vi" | "voice" | "fast" | "speed"
                                );
                            let handled_by_tui = if skip_tui_for_args {
                                false
                            } else {
                                app.intercept_slash_command_with_args(&cmd_name, &cmd_args)
                            };

                            // Sync effort level when TUI cycled the visual indicator
                            // (no-args /effort → cycle Low→Med→High→Max→Low).
                            if handled_by_tui && cmd_name == "effort" && cmd_args.is_empty() {
                                current_effort = Some(match app.effort_level {
                                    claurst_tui::EffortLevel::Low =>
                                        claurst_core::effort::EffortLevel::Low,
                                    claurst_tui::EffortLevel::Normal =>
                                        claurst_core::effort::EffortLevel::Medium,
                                    claurst_tui::EffortLevel::High =>
                                        claurst_core::effort::EffortLevel::High,
                                    claurst_tui::EffortLevel::Max =>
                                        claurst_core::effort::EffortLevel::Max,
                                });
                            }

                            // Honour exit/quit triggered by TUI intercept immediately.
                            if app.should_quit {
                                break 'main;
                            }

                            // ── Step 2: CLI-layer (real side effects) ──────────────────
                            // Handles: config changes, session ops, file I/O, OAuth, etc.
                            // Always runs — some commands need BOTH (e.g. /clear clears
                            // app state via TUI AND the messages vec via CLI).
                            cmd_ctx.messages = messages.clone();
                            let cli_result = execute_command(&input, &mut cmd_ctx).await;
                            // Start optimistically true; set false for Silent/None below.
                            let mut handled_by_cli = cli_result.is_some();

                            // Whether we need to fall through and submit a user message.
                            let mut submit_user_msg: Option<String> = None;

                            match cli_result {
                                Some(CommandResult::Exit) => break 'main,
                                Some(CommandResult::ClearConversation) => {
                                    messages.clear();
                                    app.replace_messages(Vec::new());
                                    session.messages.clear();
                                    session.updated_at = chrono::Utc::now();
                                    app.status_message =
                                        Some("Conversation cleared.".to_string());
                                }
                                Some(CommandResult::SetMessages(new_msgs)) => {
                                    let removed =
                                        messages.len().saturating_sub(new_msgs.len());
                                    messages = new_msgs.clone();
                                    app.replace_messages(new_msgs);
                                    session.messages = messages.clone();
                                    session.updated_at = chrono::Utc::now();
                                    app.status_message = Some(format!(
                                        "Rewound {} message{}.",
                                        removed,
                                        if removed == 1 { "" } else { "s" }
                                    ));
                                }
                                Some(CommandResult::OpenRewindOverlay) => {
                                    app.replace_messages(messages.clone());
                                    app.open_rewind_flow();
                                    app.status_message =
                                        Some("Select a message to rewind to.".to_string());
                                }
                                Some(CommandResult::OpenHooksOverlay) => {
                                    // Open the 4-screen hooks configuration browser.
                                    // intercept_slash_command("hooks") already does this
                                    // when the user types /hooks in the TUI prompt, so
                                    // this branch only triggers when the command returns
                                    // the variant explicitly (e.g. from a non-prompt context).
                                    app.hooks_config_menu.open();
                                    app.status_message =
                                        Some("Hooks configuration browser".to_string());
                                }
                                Some(CommandResult::OpenImportConfigOverlay) => {
                                    app.open_import_config_picker();
                                    app.status_message =
                                        Some("Select what to import from ~/.claude.".to_string());
                                }
                                Some(CommandResult::ResumeSession(resumed_session)) => {
                                    session = resumed_session;
                                    messages = session.messages.clone();
                                    app.replace_messages(messages.clone());
                                    cmd_ctx.config.model = Some(session.model.clone());
                                    app.config.model = Some(session.model.clone());
                                    tool_ctx.config.model = Some(session.model.clone());
                                    app.model_name = session.model.clone();
                                    tool_ctx.session_id = session.id.clone();
                                    tool_ctx.file_history = Arc::new(ParkingMutex::new(
                                        claurst_core::file_history::FileHistory::new(),
                                    ));
                                    tool_ctx.current_turn = Arc::new(
                                        std::sync::atomic::AtomicUsize::new(0),
                                    );
                                    cmd_ctx.session_id = session.id.clone();
                                    cmd_ctx.session_title = session.title.clone();
                                    if let Some(saved_dir) = session.working_dir.as_ref() {
                                        let saved_path =
                                            std::path::PathBuf::from(saved_dir);
                                        if saved_path.exists() {
                                            tool_ctx.working_dir = saved_path.clone();
                                            cmd_ctx.working_dir = saved_path;
                                        }
                                    }
                                    app.config.project_dir =
                                        Some(tool_ctx.working_dir.clone());
                                    app.attach_turn_diff_state(
                                        tool_ctx.file_history.clone(),
                                        tool_ctx.current_turn.clone(),
                                    );
                                    claurst_tui::update_terminal_title(
                                        session.title.as_deref(),
                                    );
                                    app.status_message = Some(format!(
                                        "Resumed session {}.",
                                        &session.id[..8]
                                    ));
                                }
                                Some(CommandResult::RenameSession(title)) => {
                                    session.title = Some(title.clone());
                                    session.updated_at = chrono::Utc::now();
                                    cmd_ctx.session_title = session.title.clone();
                                    let _ =
                                        claurst_core::history::save_session(&session).await;
                                    claurst_tui::update_terminal_title(Some(&title));
                                    app.status_message = Some(format!(
                                        "Session renamed to \"{}\".",
                                        title
                                    ));
                                }
                                Some(CommandResult::RefreshProviderState) => {
                                    if app.is_streaming || current_query.is_some() {
                                        app.status_message = Some(
                                            "Wait for the current response to finish before running /refresh."
                                                .to_string(),
                                        );
                                    } else {
                                        match refresh_provider_runtime_state(&cmd_ctx.config).await {
                                            Ok(refreshed) => {
                                                cmd_ctx.config = refreshed.config.clone();
                                                tool_ctx.config = refreshed.config.clone();
                                                base_query_config.provider_registry =
                                                    Some(refreshed.provider_registry.clone());
                                                base_query_config.model_registry =
                                                    Some(refreshed.model_registry.clone());
                                                base_query_config.model =
                                                    claurst_api::effective_model_for_config(
                                                        &cmd_ctx.config,
                                                        refreshed.model_registry.as_ref(),
                                                    );
                                                client = refreshed.client;
                                                model_registry = refreshed.model_registry;
                                                session.model =
                                                    claurst_api::effective_model_for_config(
                                                        &cmd_ctx.config,
                                                        model_registry.as_ref(),
                                                    );
                                                session.updated_at = chrono::Utc::now();
                                                app.apply_provider_refresh(
                                                    refreshed.config,
                                                    Some(refreshed.provider_registry),
                                                    refreshed.auth_store,
                                                    false,
                                                    "Saved provider state cleared. Run /connect to reconnect."
                                                        .to_string(),
                                                );
                                            }
                                            Err(err) => {
                                                app.status_message = Some(format!(
                                                    "Error: {}",
                                                    err
                                                ));
                                            }
                                        }
                                    }
                                }
                                Some(CommandResult::SpeechMode { mode, level }) => {
                                    app.set_speech_mode(mode.as_deref(), &level);
                                    cmd_ctx.config = app.config.clone();
                                    tool_ctx.config = app.config.clone();
                                }
                                Some(CommandResult::McpAuthFlow {
                                    server_name,
                                    auth_url,
                                    redirect_uri,
                                }) => {
                                    app.status_message = Some(format!(
                                        "MCP OAuth — '{}' started. Complete authentication in your browser.\nURL: {}\nCallback URL: {}",
                                        server_name, auth_url, redirect_uri
                                    ));
                                }
                                Some(CommandResult::Message(msg)) => {
                                    // Suppress text output when TUI already opened an
                                    // overlay for this command (e.g. /stats opens dialog
                                    // AND would push a text message — drop the text).
                                    if !handled_by_tui {
                                        app.push_message(
                                            claurst_core::types::Message::assistant(msg),
                                        );
                                    }
                                }
                                Some(CommandResult::ConfigChange(new_cfg)) => {
                                    let mut applied_cfg = new_cfg;
                                    normalize_provider_from_model(&mut applied_cfg);
                                    cmd_ctx.config = applied_cfg.clone();
                                    tool_ctx.config = applied_cfg.clone();
                                    app.config = applied_cfg.clone();
                                    // Sync model/provider shown in the TUI header.
                                    if let Some(ref model) = applied_cfg.model {
                                        app.set_model(model.clone());
                                    }
                                    // Sync fast_mode visual indicator.
                                    app.fast_mode = applied_cfg.model
                                        .as_deref()
                                        .map(|m| m.contains("haiku"))
                                        .unwrap_or(false);
                                    // Sync plan_mode visual indicator.
                                    app.plan_mode = matches!(
                                        applied_cfg.permission_mode,
                                        claurst_core::config::PermissionMode::Plan
                                    );
                                    session.model = claurst_api::effective_model_for_config(
                                        &cmd_ctx.config,
                                        &model_registry,
                                    );
                                    app.status_message =
                                        Some("Configuration updated.".to_string());
                                }
                                Some(CommandResult::ConfigChangeMessage(new_cfg, msg)) => {
                                    let mut applied_cfg = new_cfg;
                                    normalize_provider_from_model(&mut applied_cfg);
                                    cmd_ctx.config = applied_cfg.clone();
                                    tool_ctx.config = applied_cfg.clone();
                                    // Sync model/provider + fast_mode visual indicator.
                                    if let Some(ref model) = applied_cfg.model {
                                        app.set_model(model.clone());
                                        app.fast_mode = model.contains("haiku");
                                    } else {
                                        // model reset to None means fast mode off.
                                        app.fast_mode = false;
                                    }
                                    app.config = applied_cfg.clone();
                                    session.model = claurst_api::effective_model_for_config(
                                        &cmd_ctx.config,
                                        &model_registry,
                                    );
                                    app.status_message = Some(msg);
                                }
                                Some(CommandResult::UserMessage(msg)) => {
                                    // Queue a user-visible turn for the model.
                                    submit_user_msg = Some(msg);
                                }
                                Some(CommandResult::StartOAuthFlow(with_claude_ai)) => {
                                    claurst_tui::restore_terminal(&mut terminal).ok();
                                    match oauth_flow::run_oauth_login_flow(
                                        with_claude_ai,
                                    )
                                    .await
                                    {
                                        Ok(_) => {
                                            app.status_message =
                                                Some("Login successful!".to_string());
                                            eprintln!(
                                                "\nLogin successful! Please restart \
                                                 claude to use the new credentials."
                                            );
                                            break 'main;
                                        }
                                        Err(e) => {
                                            eprintln!("\nLogin failed: {}", e);
                                        }
                                    }
                                    terminal = claurst_tui::setup_terminal()?;
                                }
                                Some(CommandResult::Error(e)) => {
                                    app.status_message = Some(format!("Error: {}", e));
                                }
                                Some(CommandResult::Silent) | None => {
                                    handled_by_cli = false;
                                }
                            }

                            // Sync effort visual + API level when CLI handled
                            // /effort with explicit args (/effort high).
                            if handled_by_cli
                                && cmd_name == "effort"
                                && !cmd_args.is_empty()
                            {
                                if let Some(level) =
                                    claurst_core::effort::EffortLevel::from_str(&cmd_args)
                                {
                                    current_effort = Some(level);
                                    app.effort_level = match level {
                                        claurst_core::effort::EffortLevel::Low =>
                                            claurst_tui::EffortLevel::Low,
                                        claurst_core::effort::EffortLevel::Medium =>
                                            claurst_tui::EffortLevel::Normal,
                                        claurst_core::effort::EffortLevel::High =>
                                            claurst_tui::EffortLevel::High,
                                        claurst_core::effort::EffortLevel::Max =>
                                            claurst_tui::EffortLevel::Max,
                                    };
                                    app.status_message = Some(format!(
                                        "Effort: {} {}",
                                        app.effort_level.symbol(),
                                        app.effort_level.label(),
                                    ));
                                }
                            }

                            // Sync vim mode when CLI handled /vim with explicit args.
                            if handled_by_cli
                                && matches!(cmd_name.as_str(), "vim" | "vi")
                                && !cmd_args.is_empty()
                            {
                                app.prompt_input.vim_enabled =
                                    matches!(cmd_args.trim(), "on" | "vim");
                            }

                            if !handled_by_cli && !handled_by_tui {
                                app.status_message = Some(format!(
                                    "Unknown command: /{}",
                                    cmd_name
                                ));
                            }

                            // If a UserMessage was queued (e.g. /compact), submit it.
                            if let Some(msg) = submit_user_msg {
                                messages.push(claurst_core::types::Message::user(msg.clone()));
                                app.push_message(claurst_core::types::Message::user(msg));
                                // Fall through to the send path below.
                            } else {
                                continue;
                            }
                        }

                        // Fire UserPromptSubmit hook (non-blocking)
                        if !config.hooks.is_empty() {
                            let hook_ctx = claurst_core::hooks::HookContext {
                                event: "UserPromptSubmit".to_string(),
                                tool_name: None,
                                tool_input: None,
                                tool_output: Some(input.clone()),
                                is_error: None,
                                session_id: Some(tool_ctx.session_id.clone()),
                            };
                            claurst_core::hooks::run_hooks(
                                &config.hooks,
                                claurst_core::config::HookEvent::UserPromptSubmit,
                                &hook_ctx,
                                &tool_ctx.working_dir,
                            )
                            .await;
                        }

                        // Regular user message (with optional image attachments)
                        let pending_imgs = app.prompt_input.clear_images();
                        let user_msg = if pending_imgs.is_empty() {
                            claurst_core::types::Message::user(input.clone())
                        } else {
                            let mut blocks: Vec<claurst_core::types::ContentBlock> = pending_imgs
                                .iter()
                                .filter_map(|img| {
                                    claurst_tui::image_paste::encode_image_base64(&img.path)
                                        .map(|b64| claurst_core::types::ContentBlock::Image {
                                            source: claurst_core::types::ImageSource {
                                                source_type: "base64".to_string(),
                                                media_type: Some("image/png".to_string()),
                                                data: Some(b64),
                                                url: None,
                                            },
                                        })
                                })
                                .collect();
                            blocks.push(claurst_core::types::ContentBlock::Text { text: input.clone() });
                            claurst_core::types::Message::user_blocks(blocks)
                        };
                        messages.push(user_msg.clone());
                        app.push_message(user_msg);
                        session.messages = messages.clone();
                        session.updated_at = chrono::Utc::now();

                        // Update terminal title from session title or first message
                        if session.title.is_some() {
                            claurst_tui::update_terminal_title(session.title.as_deref());
                        } else {
                            // Use a truncated version of the first user message
                            let topic: String = input.chars().take(60).collect();
                            claurst_tui::update_terminal_title(Some(&topic));
                        }

                        // Start async query
                        app.is_streaming = true;
                        app.streaming_text.clear();

                        let ct = CancellationToken::new();
                        cancel = Some(ct.clone());

                        // Use Arc<Mutex> so the task can write updated messages back
                        let msgs_arc = Arc::new(tokio::sync::Mutex::new(messages.clone()));
                        let msgs_arc_clone = msgs_arc.clone();

                        // Share the Arc so the spawned task can access all tools (incl. MCP).
                        let tools_arc_clone = tools_arc.clone();
                        let mut ctx_clone = tool_ctx.clone();
                        let mut qcfg = base_query_config.clone();
                        qcfg.model = claurst_api::effective_model_for_config(&cmd_ctx.config, &model_registry);
                        qcfg.max_tokens = cmd_ctx.config.effective_max_tokens();
                        qcfg.append_system_prompt = cmd_ctx.config.append_system_prompt.clone();
                        qcfg.system_prompt = base_query_config.system_prompt.clone();
                        qcfg.output_style = cmd_ctx.config.effective_output_style();
                        qcfg.output_style_prompt = cmd_ctx.config.resolve_output_style_prompt();
                        qcfg.working_directory = Some(tool_ctx.working_dir.display().to_string());
                        // Inject active goal addendum into system prompt (if goals enabled).
                        if let Some(goal) = claurst_core::GoalStore::open_default()
                            .and_then(|s| s.get_active_goal(&session.id))
                        {
                            let addendum = claurst_core::goal_system_prompt_addendum(&goal);
                            qcfg.append_system_prompt = Some(match qcfg.append_system_prompt {
                                Some(existing) => format!("{}\n{}", existing, addendum),
                                None => addendum,
                            });
                        }
                        // Apply active effort level (set via /effort command).
                        if let Some(level) = current_effort {
                            qcfg.effort_level = Some(level);
                        }
                        // Wire completion_notifier if a command queue is available.
                        if let Some(ref cq) = qcfg.command_queue {
                            let cq = cq.clone();
                            ctx_clone.completion_notifier = Some(claurst_tools::CompletionNotifier::new(move |msg| {
                                cq.push(
                                    claurst_query::QueuedCommand::InjectSystemMessage(msg),
                                    claurst_query::CommandPriority::Normal,
                                );
                            }));
                        }
                        let tracker = cost_tracker.clone();
                        let tx = event_tx.clone();
                        let client_clone = client.clone();
                        goal_turn_start = std::time::Instant::now();

                        let handle = tokio::spawn(async move {
                            let mut msgs = msgs_arc_clone.lock().await.clone();
                            let outcome = claurst_query::run_query_loop(
                                client_clone.as_ref(),
                                &mut msgs,
                                tools_arc_clone.as_slice(),
                                &ctx_clone,
                                &qcfg,
                                tracker,
                                Some(tx),
                                ct,
                                None,
                            )
                            .await;
                            // Write updated messages (with tool calls + assistant response) back
                            *msgs_arc_clone.lock().await = msgs;
                            outcome
                        });

                        // Store the Arc so we can read messages after task completes
                        current_query = Some((handle, msgs_arc));
                        continue;
                    }
                    if let Some(pr) = app.permission_request.as_mut() {
                        if claurst_tui::dialogs::handle_permission_key(pr, key) {
                            let tool_use_id = pr.tool_use_id.clone();
                            let selected_option = pr.selected_option;
                            let selected_key = pr.options.get(selected_option).map(|o| o.key);
                            let should_record_bash_prefix = selected_key == Some('P');
                            let selected_path = pending_permissions
                                .lock()
                                .waiting
                                .get(&tool_use_id)
                                .and_then(|p| p.request.path.clone());
                            let bash_prefix = if should_record_bash_prefix {
                                match &pr.kind {
                                    claurst_tui::dialogs::PermissionDialogKind::Bash { command, .. } => {
                                        let first_word = command.split_whitespace().next().unwrap_or("").to_string();
                                        if first_word.is_empty() { None } else { Some(first_word) }
                                    }
                                    _ => None,
                                }
                            } else {
                                None
                            };
                            app.permission_request = None;

                            if let Some(prefix) = bash_prefix {
                                app.bash_prefix_allowlist.insert(prefix);
                            }

                            if let Some(mut pending) = pending_permissions.lock().waiting.remove(&tool_use_id) {
                                let decision = match selected_key {
                                    Some('n') => claurst_core::permissions::PermissionDecision::Deny,
                                    _ => claurst_core::permissions::PermissionDecision::Allow,
                                };

                                if let Some(manager) = tool_ctx.permission_manager.as_ref() {
                                    if let Ok(mut manager) = manager.lock() {
                                        match selected_key {
                                            Some('Y') => {
                                                if let Some(path) = selected_path.as_deref() {
                                                    manager.add_session_allow_path(&pending.request.tool_name, path);
                                                } else {
                                                    manager.add_session_allow(&pending.request.tool_name);
                                                }
                                            }
                                            Some('p') => {
                                                let mut settings = match claurst_core::config::Settings::load_sync() {
                                                    Ok(s) => s,
                                                    Err(_) => claurst_core::config::Settings::default(),
                                                };
                                                if let Some(path) = selected_path.as_deref() {
                                                    let pattern = format!("{}*", path);
                                                    let _ = manager.add_persistent_allow_path(&pending.request.tool_name, &pattern, &mut settings);
                                                } else {
                                                    let _ = manager.add_persistent_allow(&pending.request.tool_name, &mut settings);
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }

                                if let Some(tx) = pending.decision_tx.take() {
                                    let _ = tx.send(decision);
                                }
                            }
                            continue;
                        }
                        continue;
                    }

                    app.handle_key_event(key);
                    cmd_ctx.config = app.config.clone();
                    tool_ctx.config = app.config.clone();
                    if let Some(manager) = tool_ctx.permission_manager.as_ref() {
                        if let Ok(mut manager) = manager.lock() {
                            manager.mode = tool_ctx.config.permission_mode.clone();
                        }
                    }
                    if !app.model_name.is_empty() {
                        session.model = app.model_name.clone();
                    }
                    // Handle agent mode change (Tab key cycles build→plan→explore)
                    if app.agent_mode_changed {
                        app.agent_mode_changed = false;
                        let mode = app.agent_mode.as_deref().unwrap_or("build");
                        let mut all_agents = claurst_core::default_agents();
                        all_agents.extend(cmd_ctx.config.agents.clone());
                        if let Some(def) = all_agents.get(mode) {
                            base_query_config.agent_name = Some(mode.to_string());
                            base_query_config.agent_definition = Some(def.clone());
                            if let Some(turns) = def.max_turns {
                                base_query_config.max_turns = turns;
                            }
                            tools_arc = filter_tools_for_agent(all_tools_arc.clone(), &def.access);
                        } else {
                            // "build" with no explicit definition = full access, no agent
                            base_query_config.agent_name = None;
                            base_query_config.agent_definition = None;
                            tools_arc = all_tools_arc.clone();
                        }
                    }
                    if !app.is_streaming && app.messages.len() < messages.len() {
                        messages = app.messages.clone();
                        session.messages = messages.clone();
                        session.updated_at = chrono::Utc::now();
                    }
                }
                Event::Mouse(mouse) => {
                    app.handle_mouse_event(mouse);
                }
                Event::Resize(_, _) => {
                    // Terminal resize - will be handled on next draw
                }
                _ => {}
            }
        }

        if app.permission_request.is_none() {
            loop {
                let next_pending = pending_permissions.lock().queue.pop_front();
                let Some(mut pending) = next_pending else {
                    break;
                };

                let prefix_allowed = pending.request.tool_name == "Bash"
                    && pending
                        .request
                        .path
                        .as_deref()
                        .map(|command| app.bash_command_allowed_by_prefix(command))
                        .unwrap_or(false);

                let reevaluated = if prefix_allowed {
                    Some(claurst_core::permissions::PermissionDecision::Allow)
                } else {
                    tool_ctx
                        .permission_manager
                        .as_ref()
                        .and_then(|manager| manager.lock().ok())
                        .map(|manager| {
                            manager.evaluate(
                                &pending.request.tool_name,
                                &pending.request.description,
                                pending.request.path.as_deref(),
                                pending.request.working_dir.as_deref(),
                                &pending.request.allowed_roots,
                            )
                        })
                };

                match reevaluated {
                    Some(claurst_core::permissions::PermissionDecision::Ask { .. }) | None => {
                        let tool_use_id = pending.tool_use_id.clone();
                        app.permission_request = Some(permission_request_from_core(&pending));
                        pending_permissions.lock().waiting.insert(tool_use_id, pending);
                        break;
                    }
                    Some(decision) => {
                        if let Some(tx) = pending.decision_tx.take() {
                            let _ = tx.send(decision);
                        }
                    }
                }
            }
        }

        // Drain query events — also forward relevant ones to the bridge as outbound.
        while let Ok(evt) = event_rx.try_recv() {
            // Forward to bridge before consuming (clone only what we need).
            if let Some(ref runtime) = bridge_runtime {
                let outbound: Option<BridgeOutbound> = match &evt {
                    QueryEvent::Stream(claurst_api::AnthropicStreamEvent::ContentBlockDelta {
                        delta: claurst_api::streaming::ContentDelta::TextDelta { text },
                        index,
                        ..
                    }) => Some(BridgeOutbound::TextDelta {
                        delta: text.clone(),
                        message_id: format!("msg-{}", index),
                    }),
                    QueryEvent::ToolStart { tool_name, tool_id, input_json } => {
                        Some(BridgeOutbound::ToolStart {
                            id: tool_id.clone(),
                            name: tool_name.clone(),
                            input_preview: Some(input_json.clone()),
                        })
                    }
                    QueryEvent::ToolEnd { tool_id, result, is_error, .. } => {
                        Some(BridgeOutbound::ToolEnd {
                            id: tool_id.clone(),
                            output: result.clone(),
                            is_error: *is_error,
                        })
                    }
                    QueryEvent::TurnComplete { stop_reason, turn, .. } => {
                        Some(BridgeOutbound::TurnComplete {
                            message_id: format!("turn-{}", turn),
                            stop_reason: stop_reason.clone(),
                        })
                    }
                    QueryEvent::Error(msg) => Some(BridgeOutbound::Error {
                        message: msg.clone(),
                    }),
                    _ => None,
                };
                if let Some(ob) = outbound {
                    let _ = runtime.outbound_tx.try_send(ob);
                }
            }
            // Also forward to the BridgeSessionInfo relay channel (best-effort).
            // This drives the post_bridge_event relay task spawned on Connected.
            if bridge_session_info.is_some() {
                let relay_payload: Option<String> = match &evt {
                    QueryEvent::Stream(claurst_api::AnthropicStreamEvent::ContentBlockDelta {
                        delta: claurst_api::streaming::ContentDelta::TextDelta { text },
                        ..
                    }) => Some(serde_json::json!({
                        "type": "text_chunk",
                        "text": text,
                    }).to_string()),
                    QueryEvent::ToolStart { tool_name, tool_id, input_json } => {
                        Some(serde_json::json!({
                            "type": "tool_start",
                            "tool_name": tool_name,
                            "tool_id": tool_id,
                            "input": input_json,
                        }).to_string())
                    }
                    QueryEvent::ToolEnd { tool_name, tool_id, result, is_error } => {
                        Some(serde_json::json!({
                            "type": "tool_end",
                            "tool_name": tool_name,
                            "tool_id": tool_id,
                            "result": result,
                            "is_error": is_error,
                        }).to_string())
                    }
                    _ => None,
                };
                if let Some(payload) = relay_payload {
                    let _ = relay_ev_tx.try_send(payload);
                }
            }
            app.handle_query_event(evt);
        }

        // Auto-compact: when context usage hits 99% and no query is running,
        // automatically submit a compact request.
        if app.context_window_size > 0
            && !app.is_streaming
            && current_query.is_none()
            && !app.auto_compact_running
        {
            let used_pct = (app.context_used_tokens as f64 / app.context_window_size as f64 * 100.0) as u64;
            if used_pct >= 99 {
                app.auto_compact_running = true;
                let msg_count = messages.len();
                let compact_msg = format!(
                    "[Auto-compact triggered ({} messages, {}% context used). \
                     Provide a detailed summary of our conversation so far, \
                     preserving all key technical details, decisions made, \
                     file paths mentioned, and current task status.]",
                    msg_count, used_pct
                );
                app.status_message = Some("Context 99% full — auto-compacting…".to_string());
                let user_msg = claurst_core::types::Message::user(compact_msg);
                messages.push(user_msg.clone());
                app.push_message(user_msg);
                session.messages = messages.clone();
                session.updated_at = chrono::Utc::now();

                // Dispatch the compact query immediately.
                let ct = CancellationToken::new();
                cancel = Some(ct.clone());
                let msgs_arc = Arc::new(tokio::sync::Mutex::new(messages.clone()));
                let msgs_arc_clone = msgs_arc.clone();
                let tools_arc_clone = tools_arc.clone();
                let ctx_clone = tool_ctx.clone();
                let mut qcfg = base_query_config.clone();
                qcfg.model = claurst_api::effective_model_for_config(&cmd_ctx.config, &model_registry);
                qcfg.max_tokens = cmd_ctx.config.effective_max_tokens();
                let tracker = cost_tracker.clone();
                let tx = event_tx.clone();
                let client_clone = client.clone();
                app.is_streaming = true;

                let handle = tokio::spawn(async move {
                    let mut msgs = msgs_arc_clone.lock().await.clone();
                    let outcome = claurst_query::run_query_loop(
                        client_clone.as_ref(),
                        &mut msgs,
                        tools_arc_clone.as_slice(),
                        &ctx_clone,
                        &qcfg,
                        tracker,
                        Some(tx),
                        ct,
                        None,
                    )
                    .await;
                    *msgs_arc_clone.lock().await = msgs;
                    outcome
                });
                current_query = Some((handle, msgs_arc));
            }
        }

        // Drain TUI-facing bridge events.
        let mut disconnect_bridge = false;
        if let Some(runtime) = bridge_runtime.as_mut() {
            loop {
                match runtime.tui_rx.try_recv() {
                    Ok(TuiBridgeEvent::Connected { session_url, session_id: conn_sid }) => {
                        let short = if session_url.len() > 60 {
                            format!("{}…", &session_url[..60])
                        } else {
                            session_url.clone()
                        };
                        app.bridge_state = BridgeConnectionState::Connected {
                            session_url: session_url.clone(),
                            peer_count: 0,
                        };
                        app.remote_session_url = Some(session_url.clone());
                        cmd_ctx.remote_session_url = Some(session_url.clone());
                        app.notifications.push(
                            NotificationKind::Success,
                            format!("Remote control active: {}", short),
                            Some(5),
                        );
                        // Persist the session URL into the saved session record.
                        session.remote_session_url = Some(session_url.clone());
                        session.updated_at = chrono::Utc::now();
                        let _ = claurst_core::history::save_session(&session).await;

                        // Wire the BridgeSessionInfo relay so live tool/text events reach
                        // the web UI via /api/bridge/sessions. This runs alongside
                        // run_bridge_loop as a best-effort supplementary delivery path.
                        if let Some(ref token) = bridge_token {
                            let info = std::sync::Arc::new(claurst_bridge::BridgeSessionInfo {
                                session_id: conn_sid.clone(),
                                session_url: session_url.clone(),
                                token: token.clone(),
                            });
                            bridge_session_info = Some(info.clone());

                            // Relay consumer: moves relay_ev_rx (taken from the Option)
                            // into a background task that calls post_bridge_event per item.
                            if let Some(rx) = relay_ev_rx_opt.take() {
                                let info_relay = info.clone();
                                tokio::spawn(async move {
                                    let mut rx = rx;
                                    while let Some(payload) = rx.recv().await {
                                        let _ = claurst_bridge::post_bridge_event(
                                            &info_relay,
                                            payload,
                                        )
                                        .await;
                                    }
                                });
                            }

                            // Poll task: periodically calls poll_bridge_messages and
                            // forwards inbound user messages to remote_prompt_tx.
                            let info_poll = info.clone();
                            let poll_tx = remote_prompt_tx.clone();
                            tokio::spawn(async move {
                                let mut since_id: Option<String> = None;
                                loop {
                                    match claurst_bridge::poll_bridge_messages(
                                        &info_poll,
                                        since_id.as_deref(),
                                    )
                                    .await
                                    {
                                        Ok(msgs) if !msgs.is_empty() => {
                                            for msg in &msgs {
                                                since_id = Some(msg.id.clone());
                                                if msg.role == "user" {
                                                    if poll_tx
                                                        .send(msg.content.clone())
                                                        .await
                                                        .is_err()
                                                    {
                                                        return;
                                                    }
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                    tokio::time::sleep(
                                        std::time::Duration::from_secs(2),
                                    )
                                    .await;
                                }
                            });
                        }
                    }
                    Ok(TuiBridgeEvent::Disconnected { reason }) => {
                        app.bridge_state = BridgeConnectionState::Disconnected;
                        app.remote_session_url = None;
                        cmd_ctx.remote_session_url = None;
                        if let Some(r) = reason {
                            app.notifications.push(
                                NotificationKind::Warning,
                                format!("Bridge disconnected: {}", r),
                                Some(5),
                            );
                        }
                        disconnect_bridge = true;
                        break;
                    }
                    Ok(TuiBridgeEvent::Reconnecting { attempt }) => {
                        app.bridge_state = BridgeConnectionState::Reconnecting { attempt };
                    }
                    Ok(TuiBridgeEvent::InboundPrompt { content, .. }) => {
                        // Inject the remote prompt as if the user typed it, then
                        // trigger submission automatically.
                        app.set_prompt_text(content.clone());
                        // Push as a user message and fire a query immediately.
                        messages.push(claurst_core::types::Message::user(content.clone()));
                        app.push_message(claurst_core::types::Message::user(content.clone()));
                        session.messages = messages.clone();
                        session.updated_at = chrono::Utc::now();
                        app.is_streaming = true;
                        app.streaming_text.clear();
                        let ct = CancellationToken::new();
                        cancel = Some(ct.clone());
                        let msgs_arc = Arc::new(tokio::sync::Mutex::new(messages.clone()));
                        let msgs_arc_clone = msgs_arc.clone();
                        let tools_arc_clone = tools_arc.clone();
                        let ctx_clone = tool_ctx.clone();
                        let mut qcfg = base_query_config.clone();
                        qcfg.model = claurst_api::effective_model_for_config(&cmd_ctx.config, &model_registry);
                        qcfg.max_tokens = cmd_ctx.config.effective_max_tokens();
                        let tracker = cost_tracker.clone();
                        let tx = event_tx.clone();
                        let client_clone = client.clone();
                        let handle = tokio::spawn(async move {
                            let mut msgs = msgs_arc_clone.lock().await.clone();
                            let outcome = claurst_query::run_query_loop(
                                client_clone.as_ref(),
                                &mut msgs,
                                tools_arc_clone.as_slice(),
                                &ctx_clone,
                                &qcfg,
                                tracker,
                                Some(tx),
                                ct,
                                None,
                            )
                            .await;
                            *msgs_arc_clone.lock().await = msgs;
                            outcome
                        });
                        current_query = Some((handle, msgs_arc));
                    }
                    Ok(TuiBridgeEvent::Cancelled) => {
                        if app.is_streaming {
                            if let Some(ref ct) = cancel {
                                ct.cancel();
                            }
                            app.is_streaming = false;
                            app.status_message =
                                Some("Cancelled by remote control.".to_string());
                        }
                    }
                    Ok(TuiBridgeEvent::PermissionResponse { tool_use_id, response }) => {
                        // Resolve a pending permission dialog if IDs match.
                        if let Some(ref pr) = app.permission_request {
                            if pr.tool_use_id == tool_use_id {
                                use claurst_bridge::PermissionResponseKind;
                                let _allow = matches!(
                                    response,
                                    PermissionResponseKind::Allow | PermissionResponseKind::AllowSession
                                );
                                app.permission_request = None;
                            }
                        }
                    }
                    Ok(TuiBridgeEvent::SessionNameUpdate { title }) => {
                        session.title = Some(title.clone());
                        session.updated_at = chrono::Utc::now();
                        cmd_ctx.session_title = Some(title.clone());
                        app.session_title = Some(title);
                        let _ = claurst_core::history::save_session(&session).await;
                    }
                    Ok(TuiBridgeEvent::Error(msg)) => {
                        app.bridge_state = BridgeConnectionState::Failed {
                            reason: msg.clone(),
                        };
                        app.notifications.push(
                            NotificationKind::Warning,
                            format!("Bridge error: {}", msg),
                            Some(5),
                        );
                        disconnect_bridge = true;
                        break;
                    }
                    Ok(TuiBridgeEvent::Ping) => {
                        // No TUI action needed; pong is handled inside run_bridge_loop.
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        app.bridge_state = BridgeConnectionState::Disconnected;
                        app.remote_session_url = None;
                        cmd_ctx.remote_session_url = None;
                        app.notifications.push(
                            NotificationKind::Warning,
                            "Remote control connection lost.".to_string(),
                            Some(5),
                        );
                        disconnect_bridge = true;
                        break;
                    }
                }
            }
        }
        if disconnect_bridge {
            bridge_runtime = None;
        }

        // Drain inbound prompts from the BridgeSessionInfo poll task.
        // These are user messages received from the web UI via poll_bridge_messages
        // and injected here just like TuiBridgeEvent::InboundPrompt.
        while let Ok(content) = remote_prompt_rx.try_recv() {
            if !app.is_streaming {
                app.set_prompt_text(content.clone());
                messages.push(claurst_core::types::Message::user(content.clone()));
                app.push_message(claurst_core::types::Message::user(content.clone()));
                session.messages = messages.clone();
                session.updated_at = chrono::Utc::now();
                app.is_streaming = true;
                app.streaming_text.clear();
                let ct = CancellationToken::new();
                cancel = Some(ct.clone());
                let msgs_arc = Arc::new(tokio::sync::Mutex::new(messages.clone()));
                let msgs_arc_clone = msgs_arc.clone();
                let tools_arc_clone = tools_arc.clone();
                let ctx_clone = tool_ctx.clone();
                let mut qcfg = base_query_config.clone();
                qcfg.model = claurst_api::effective_model_for_config(&cmd_ctx.config, &model_registry);
                qcfg.max_tokens = cmd_ctx.config.effective_max_tokens();
                let tracker = cost_tracker.clone();
                let tx = event_tx.clone();
                let client_clone = client.clone();
                let handle = tokio::spawn(async move {
                    let mut msgs = msgs_arc_clone.lock().await.clone();
                    let outcome = claurst_query::run_query_loop(
                        client_clone.as_ref(),
                        &mut msgs,
                        tools_arc_clone.as_slice(),
                        &ctx_clone,
                        &qcfg,
                        tracker,
                        Some(tx),
                        ct,
                        None,
                    )
                    .await;
                    *msgs_arc_clone.lock().await = msgs;
                    outcome
                });
                current_query = Some((handle, msgs_arc));
                break; // process one prompt per frame
            }
        }

        // Drain CLAUDE_STATUS_COMMAND results (most recent wins)
        if status_cmd_str.is_some() {
            loop {
                match status_cmd_rx.try_recv() {
                    Ok(text) => {
                        app.status_line_override = if text.is_empty() { None } else { Some(text) };
                    }
                    Err(_) => break,
                }
            }
        }

        // Sync cost/token counters and expire transient UI state.
        app.cost_usd = app.cost_tracker.total_cost_usd();
        app.token_count = app.cost_tracker.total_tokens() as u32;
        app.notifications.tick();
        app.memory_update_notification.tick();

        // Drain background model-fetch results (non-blocking).
        if let Some(ref mut rx) = app.model_fetch_rx {
            match rx.try_recv() {
                Ok(Ok(entries)) => {
                    let provider = app
                        .config
                        .provider
                        .clone()
                        .unwrap_or_else(|| "anthropic".to_string());
                    let provider_prefix = format!("{}/", provider);
                    let current = app
                        .model_name
                        .strip_prefix(&provider_prefix)
                        .unwrap_or(app.model_name.as_str())
                        .to_string();
                    app.model_picker.set_models(entries);
                    for m in &mut app.model_picker.models {
                        m.is_current = m.id == current;
                    }
                    app.model_picker.loading_models = false;
                    app.model_fetch_rx = None;
                }
                Ok(Err(()))
                | Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    app.model_picker.loading_models = false;
                    app.model_fetch_rx = None;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
            }
        }

        // Spawn async provider model-list fetch when requested.
        if app.model_picker_fetch_pending {
            app.model_picker_fetch_pending = false;
            let provider_id_str = app
                .config
                .provider
                .clone()
                .unwrap_or_else(|| "anthropic".to_string());
            if let Some(ref registry) = app.provider_registry {
                let pid = claurst_core::ProviderId::new(&provider_id_str);
                if let Some(provider) = registry.get(&pid) {
                    let provider = provider.clone();
                    let (tx, rx) = tokio::sync::mpsc::channel(1);
                    app.model_fetch_rx = Some(rx);
                    app.model_picker.loading_models = true;
                    tokio::spawn(async move {
                        match provider.list_models().await {
                            Ok(models) => {
                                let entries: Vec<claurst_tui::model_picker::ModelEntry> = models
                                    .into_iter()
                                    .map(|m| claurst_tui::model_picker::ModelEntry {
                                        id: m.id.to_string(),
                                        display_name: m.name.clone(),
                                        description: claurst_tui::model_picker::format_context_window(
                                            m.context_window,
                                        ),
                                        is_current: false,
                                    })
                                    .collect();
                                let _ = tx.send(Ok(entries)).await;
                            }
                            Err(_) => {
                                let _ = tx.send(Err(())).await;
                            }
                        }
                    });
                }
            }
        }

        // Refresh task list if the overlay is visible.
        if app.tasks_overlay.visible {
            app.tasks_overlay.refresh_tasks(&claurst_tools::TASK_STORE);
        }

        // Check if the background update task has reported a result.
        if app.update_available.is_none() {
            if let Ok(Some(version)) = update_rx.try_recv() {
                app.update_available = Some(version);
            }
        }

        // ---- Device code / OAuth auth: spawn background task when pending ----
        if let Some(provider_id) = app.device_auth_pending.take() {
            let _tx = device_auth_tx.clone();
            match provider_id.as_str() {
                "github-copilot" => {
                    let tx2 = device_auth_tx.clone();
                    // Use the OpenCode Copilot OAuth app (Ov23li8tweQw6odWQebz)
                    // which is registered and authorised for the Copilot API.
                    // Tokens from an unregistered app get "model not supported"
                    // on every model.
                    const COPILOT_CLIENT_ID: &str = "Ov23li8tweQw6odWQebz";
                    tokio::spawn(async move {
                        // Step 1: Request device code
                        match claurst_core::device_code::request_device_code(
                            COPILOT_CLIENT_ID,
                            "read:user",
                            "https://github.com/login/device/code",
                        ).await {
                            Ok(resp) => {
                                let _ = tx2.send(DeviceAuthEvent::GotCode {
                                    user_code: resp.user_code,
                                    verification_uri: resp.verification_uri,
                                    device_code: resp.device_code.clone(),
                                    interval: resp.interval,
                                }).await;
                                // Step 2: Poll for access token
                                match claurst_core::device_code::poll_for_token(
                                    COPILOT_CLIENT_ID,
                                    &resp.device_code,
                                    "https://github.com/login/oauth/access_token",
                                    resp.interval,
                                    300,
                                ).await {
                                    Ok(token) => {
                                        let _ = tx2.send(DeviceAuthEvent::TokenReceived(token)).await;
                                    }
                                    Err(e) => {
                                        let _ = tx2.send(DeviceAuthEvent::Error(e)).await;
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = tx2.send(DeviceAuthEvent::Error(e)).await;
                            }
                        }
                    });
                }
                "anthropic" => {
                    let tx2 = device_auth_tx.clone();
                    // Anthropic OAuth requires a registered application.
                    // Claurst does not have its own registered OAuth app with Anthropic.
                    // Users should use an API key from console.anthropic.com instead.
                    tokio::spawn(async move {
                        let _ = tx2.send(DeviceAuthEvent::Error(
                            "Anthropic OAuth requires a registered application.\n\
                             Use an API key instead: console.anthropic.com/settings/keys".to_string()
                        )).await;
                    });
                }
                "codex" | "openai-codex" => {
                    let tx2 = device_auth_tx.clone();
                    // Keep the dialog in WaitingForCode until GotBrowserUrl arrives.
                    // (set_browser_url() transitions it to BrowserAuth with the URL.)
                    tokio::spawn(async move {
                        match crate::codex_oauth_flow::run_oauth_flow(tx2.clone()).await {
                            Ok(tokens) => {
                                let _ = tx2.send(DeviceAuthEvent::TokenReceived(
                                    tokens.access_token,
                                )).await;
                            }
                            Err(e) => {
                                let _ = tx2.send(DeviceAuthEvent::Error(
                                    format!("Codex OAuth failed: {}", e),
                                )).await;
                            }
                        }
                    });
                }
                _ => {
                    // Unknown provider for device auth — should not happen
                    app.device_auth_dialog
                        .set_error(format!("Unsupported auth flow for {}", provider_id));
                }
            }
        }

        // ---- Drain device auth events from the background task ----
        while let Ok(evt) = device_auth_rx.try_recv() {
            match evt {
                DeviceAuthEvent::GotCode {
                    user_code,
                    verification_uri,
                    device_code,
                    interval,
                } => {
                    // Auto-copy the user code to clipboard
                    let _ = claurst_tui::try_copy_to_clipboard(&user_code);

                    // Auto-open the verification URL in the browser
                    let _ = open::that(&verification_uri);

                    app.device_auth_dialog
                        .set_code(user_code, verification_uri, device_code, interval);

                    app.notifications.push(
                        claurst_tui::NotificationKind::Info,
                        "Code copied to clipboard & browser opened.".to_string(),
                        Some(4),
                    );
                }
                DeviceAuthEvent::GotBrowserUrl { url } => {
                    // Copy the URL to clipboard so the user can paste it even
                    // when the automatic browser launch silently fails (headless
                    // terminals, tty2, Wayland-without-xdg-open, etc.).
                    let _ = claurst_tui::try_copy_to_clipboard(&url);
                    app.device_auth_dialog.set_browser_url(url);
                    app.notifications.push(
                        claurst_tui::NotificationKind::Info,
                        "Login URL copied to clipboard.".to_string(),
                        Some(5),
                    );
                }
                DeviceAuthEvent::TokenReceived(token) => {
                    app.device_auth_dialog.set_success(token);
                }
                DeviceAuthEvent::Error(msg) => {
                    app.device_auth_dialog.set_error(msg);
                }
            }
        }

        while let Ok(evt) = mcp_auth_rx.try_recv() {
            match evt {
                McpAuthEvent::Completed(result) => {
                    // Schedule a runtime rebuild so the newly persisted token is
                    // picked up by the next MCP manager instance.
                    app.pending_mcp_reconnect = true;
                    app.status_message = Some(format!(
                        "MCP OAuth — '{}' authentication completed; token saved to: {}",
                        result.server_name,
                        result.token_path.display()
                    ));
                }
                McpAuthEvent::Failed(error) => {
                    app.status_message = Some(format!("MCP OAuth failed: {}", error));
                }
            }
        }
        // Check if query task is done; sync messages from the task
        let task_finished = current_query
            .as_ref()
            .map(|(h, _)| h.is_finished())
            .unwrap_or(false);

        if task_finished {
            if let Some((handle, msgs_arc)) = current_query.take() {
                // Get the outcome (ignore errors for now)
                let _ = handle.await;
                // Sync the updated conversation back to our local vector
                messages = msgs_arc.lock().await.clone();
                session.messages = messages.clone();
                session.updated_at = chrono::Utc::now();
                session.model = claurst_api::effective_model_for_config(&cmd_ctx.config, &model_registry);
                session.working_dir = Some(tool_ctx.working_dir.display().to_string());
                app.is_streaming = false;
                app.status_message = None;
                if app.auto_compact_running {
                    app.auto_compact_running = false;
                    // After auto-compact the context was summarised — reset usage.
                    app.context_used_tokens = 0;
                    app.status_message = Some("Auto-compact complete.".to_string());
                }

                // Save session to JSONL (primary storage)
                let _ = claurst_core::history::save_session(&session).await;

                // Also index into SQLite for /search support
                {
                    let db_path = claurst_core::config::Settings::config_dir().join("sessions.db");
                    if let Ok(store) = claurst_core::SqliteSessionStore::open(&db_path) {
                        let _ = store.save_session(
                            &session.id,
                            session.title.as_deref(),
                            &session.model,
                        );
                        for msg in &session.messages {
                            let content_str = match &msg.content {
                                claurst_core::types::MessageContent::Text(t) => t.clone(),
                                claurst_core::types::MessageContent::Blocks(blocks) => blocks.iter()
                                    .filter_map(|b| if let claurst_core::types::ContentBlock::Text { text } = b { Some(text.as_str()) } else { None })
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            };
                            let role = match msg.role {
                                claurst_core::types::Role::User => "user",
                                claurst_core::types::Role::Assistant => "assistant",
                            };
                            let msg_id = msg.uuid.as_deref().unwrap_or("unknown");
                            let _ = store.save_message(&session.id, msg_id, role, &content_str, None);
                        }
                    }
                }

                // --- Goal continuation ---
                // After every completed turn check if there is an active goal.
                // If so, inject a continuation user message and dispatch another turn
                // without waiting for user input.
                if !app.auto_compact_running && claurst_core::goals_enabled() {
                    let elapsed_secs = goal_turn_start.elapsed().as_secs();
                    let total_tokens = cost_tracker.total_tokens();
                    match claurst_query::check_and_continue_goal(
                        &session.id,
                        total_tokens,
                        elapsed_secs,
                    ) {
                        claurst_query::GoalContinuation::Continue { message } => {
                            // Show a subtle status notice.
                            app.status_message = Some(
                                "Goal: continuing autonomously… (use /goal pause to stop)".to_string()
                            );
                            // Update the footer badge.
                            if let Some(goal) = claurst_core::GoalStore::open_default()
                                .and_then(|s| s.get_active_goal(&session.id))
                            {
                                app.active_goal_badge = Some(format!(
                                    "active · {} · {} turns",
                                    goal.elapsed_display(),
                                    goal.turns_used
                                ));
                            }

                            // Inject the continuation message into the conversation.
                            let cont_msg = claurst_core::types::Message::user(message);
                            messages.push(cont_msg.clone());
                            app.push_message(cont_msg);
                            session.messages = messages.clone();
                            session.updated_at = chrono::Utc::now();
                            app.is_streaming = true;
                            app.streaming_text.clear();

                            let ct = CancellationToken::new();
                            cancel = Some(ct.clone());

                            let msgs_arc = Arc::new(tokio::sync::Mutex::new(messages.clone()));
                            let msgs_arc_clone = msgs_arc.clone();
                            let tools_arc_clone = tools_arc.clone();
                            let mut ctx_clone = tool_ctx.clone();
                            let mut qcfg = base_query_config.clone();
                            qcfg.model = claurst_api::effective_model_for_config(&cmd_ctx.config, &model_registry);
                            qcfg.max_tokens = cmd_ctx.config.effective_max_tokens();
                            qcfg.append_system_prompt = cmd_ctx.config.append_system_prompt.clone();
                            qcfg.system_prompt = base_query_config.system_prompt.clone();
                            qcfg.output_style = cmd_ctx.config.effective_output_style();
                            qcfg.output_style_prompt = cmd_ctx.config.resolve_output_style_prompt();
                            qcfg.working_directory = Some(tool_ctx.working_dir.display().to_string());
                            // Re-inject the goal addendum for this continuation turn.
                            if let Some(goal) = claurst_core::GoalStore::open_default()
                                .and_then(|s| s.get_active_goal(&session.id))
                            {
                                let addendum = claurst_core::goal_system_prompt_addendum(&goal);
                                qcfg.append_system_prompt = Some(match qcfg.append_system_prompt {
                                    Some(existing) => format!("{}\n{}", existing, addendum),
                                    None => addendum,
                                });
                            }
                            if let Some(level) = current_effort {
                                qcfg.effort_level = Some(level);
                            }
                            if let Some(ref cq) = qcfg.command_queue {
                                let cq = cq.clone();
                                ctx_clone.completion_notifier = Some(claurst_tools::CompletionNotifier::new(move |msg| {
                                    cq.push(
                                        claurst_query::QueuedCommand::InjectSystemMessage(msg),
                                        claurst_query::CommandPriority::Normal,
                                    );
                                }));
                            }
                            let tracker = cost_tracker.clone();
                            let tx = event_tx.clone();
                            let client_clone = client.clone();
                            goal_turn_start = std::time::Instant::now();

                            let handle = tokio::spawn(async move {
                                let mut msgs = msgs_arc_clone.lock().await.clone();
                                let outcome = claurst_query::run_query_loop(
                                    client_clone.as_ref(),
                                    &mut msgs,
                                    tools_arc_clone.as_slice(),
                                    &ctx_clone,
                                    &qcfg,
                                    tracker,
                                    Some(tx),
                                    ct,
                                    None,
                                )
                                .await;
                                *msgs_arc_clone.lock().await = msgs;
                                outcome
                            });
                            current_query = Some((handle, msgs_arc));
                        }
                        claurst_query::GoalContinuation::Stop { reason } => {
                            app.active_goal_badge = None;
                            if let Some(msg) = reason.user_message() {
                                app.status_message = Some(msg);
                            }
                        }
                        claurst_query::GoalContinuation::NoGoal => {
                            app.active_goal_badge = None;
                        }
                    }
                }
            }
        }

        if !app.is_streaming && current_query.is_none() {
            if let Some(server_name) = app.take_pending_mcp_panel_auth() {
                let server_config = cmd_ctx
                    .config
                    .mcp_servers
                    .iter()
                    .find(|server| server.name == server_name);
                let supports_panel_auth = server_config.is_some_and(|server| {
                    matches!(server.server_type.as_str(), "http" | "sse")
                        && server.url.as_deref().is_some()
                });

                if !supports_panel_auth {
                    app.status_message = Some(format!(
                        "Selected MCP server '{}' does not support panel auth.",
                        server_name
                    ));
                } else if let Some(manager) = app.mcp_manager.clone() {
                    match manager.begin_auth(&server_name).await {
                        Ok(session) => {
                            let auth_url = session.auth_url.clone();
                            let redirect_uri = session.redirect_uri.clone();
                            mcp_auth_runner(session);
                            app.status_message = Some(format!(
                                "MCP auth — '{}' started. Complete authentication in your browser.\nURL: {}\nCallback URL: {}",
                                server_name, auth_url, redirect_uri
                            ));
                        }
                        Err(error) => {
                            app.status_message = Some(format!(
                                "MCP auth failed for '{}': {}",
                                server_name, error
                            ));
                        }
                    }
                } else {
                    app.status_message = Some(
                        "MCP auth is unavailable because the MCP runtime is not connected."
                            .to_string(),
                    );
                }
            }
        }

        if !app.is_streaming && current_query.is_none() && app.take_pending_mcp_reconnect() {
            let new_mcp_manager = connect_mcp_manager_arc(&cmd_ctx.config).await;
            tool_ctx.mcp_manager = new_mcp_manager.clone();
            app.mcp_manager = new_mcp_manager.clone();
            tools_arc = build_tools_with_mcp(new_mcp_manager.clone());
            if app.mcp_view.open {
                app.refresh_mcp_view();
            }

            let connected = new_mcp_manager
                .as_ref()
                .map(|manager| manager.server_count())
                .unwrap_or(0);
            app.status_message = Some(if cmd_ctx.config.mcp_servers.is_empty() {
                "No MCP servers configured.".to_string()
            } else {
                format!(
                    "Reconnected MCP runtime ({} connected server{}).",
                    connected,
                    if connected == 1 { "" } else { "s" }
                )
            });
        }

        if app.should_quit {
            break 'main;
        }
    }

    if let Some(runtime) = bridge_runtime.take() {
        runtime.cancel.cancel();
    }
    restore_terminal(&mut terminal)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// `claude auth` subcommand handler
// ---------------------------------------------------------------------------
// Mirrors TypeScript cli.tsx `if (args[0] === 'auth') { ... }` fast-path.
// Called before Cli::parse() so it doesn't conflict with positional `prompt`.
//
// Usage:
//   claude auth login [--console]   — OAuth PKCE login (claude.ai by default)
//   claude auth logout              — Clear stored credentials
//   claude auth status [--json]     — Show authentication status

async fn handle_auth_command(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(|s| s.as_str()) {
        Some("login") => {
            // --console flag selects the Console OAuth flow (creates an API key)
            // Default (no flag) uses the Claude.ai flow (Bearer token)
            let login_with_claude_ai = !args.iter().any(|a| a == "--console");
            println!("Starting authentication...");
            match oauth_flow::run_oauth_login_flow(login_with_claude_ai).await {
                Ok(result) => {
                    println!("Successfully logged in!");
                    if let Some(email) = &result.tokens.email {
                        println!("  Account: {}", email);
                    }
                    if result.use_bearer_auth {
                        println!("  Auth method: claude.ai");
                    } else {
                        println!("  Auth method: console (API key)");
                    }
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("Login failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Some("logout") => {
            auth_logout().await;
        }

        Some("status") => {
            let json_output = args.iter().any(|a| a == "--json");
            auth_status(json_output).await;
        }

        Some(unknown) => {
            eprintln!("Unknown auth subcommand: '{}'", unknown);
            eprintln!();
            eprintln!("Usage: claude auth <subcommand>");
            eprintln!("  login [--console]   Authenticate (claude.ai by default; --console for API key)");
            eprintln!("  logout              Remove stored credentials");
            eprintln!("  status [--json]     Show authentication status");
            std::process::exit(1);
        }

        None => {
            eprintln!("Usage: claude auth <login|logout|status>");
            eprintln!("  login [--console]   Authenticate with Anthropic");
            eprintln!("  logout              Remove stored credentials");
            eprintln!("  status [--json]     Show authentication status");
            std::process::exit(1);
        }
    }

    Ok(())
}

fn provider_status_lookup_keys(provider_id: &str) -> Vec<&str> {
    match provider_id {
        "togetherai" | "together-ai" => vec!["togetherai", "together-ai"],
        "lmstudio" | "lm-studio" => vec!["lmstudio", "lm-studio"],
        "llamacpp" | "llama-cpp" | "llama-server" => vec!["llamacpp", "llama-cpp", "llama-server"],
        "moonshot" | "moonshotai" => vec!["moonshot", "moonshotai"],
        "zhipu" | "zhipuai" => vec!["zhipu", "zhipuai"],
        "vultr" | "vultr-ai" => vec!["vultr", "vultr-ai"],
        "google" | "google-vertex" => vec!["google", "google-vertex"],
        _ => vec![provider_id],
    }
}

fn format_provider_name(provider_id: &str) -> String {
    match provider_id {
        "anthropic" => "Anthropic".to_string(),
        "openai" => "OpenAI".to_string(),
        "google" => "Google".to_string(),
        "google-vertex" => "Google Vertex".to_string(),
        "github-copilot" => "GitHub Copilot".to_string(),
        "xai" => "xAI".to_string(),
        "lmstudio" | "lm-studio" => "LM Studio".to_string(),
        "llamacpp" | "llama-cpp" | "llama-server" => "llama.cpp".to_string(),
        other => other
            .split('-')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Print current auth status, then exit with code 0 (logged in) or 1 (not logged in).
async fn auth_status(json_output: bool) {
    let settings = Settings::load().await.unwrap_or_default();
    let config = &settings.config;
    let active_provider = config.selected_provider_id();
    let provider_cfg = config
        .provider_configs
        .get(active_provider)
        .filter(|provider| provider.enabled);
    let auth_store = claurst_core::AuthStore::load();
    let oauth_tokens = if active_provider == "anthropic" {
        claurst_core::oauth::OAuthTokens::load().await
    } else {
        None
    };

    let env_api_key_source = claurst_core::config::api_key_env_vars_for_provider(active_provider)
        .iter()
        .find_map(|env_var| {
            std::env::var(env_var)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|_| (*env_var).to_string())
        });
    let stored_api_key_source = provider_status_lookup_keys(active_provider)
        .into_iter()
        .find_map(|provider_id| match auth_store.get(provider_id) {
            Some(claurst_core::StoredCredential::ApiKey { key }) if !key.is_empty() => {
                Some("stored credential".to_string())
            }
            Some(claurst_core::StoredCredential::OAuthToken {
                access, refresh, ..
            }) if active_provider == "github-copilot"
                && (!access.is_empty() || !refresh.is_empty()) =>
            {
                Some("stored token".to_string())
            }
            _ => None,
        });

    let api_provider = format_provider_name(active_provider);
    let api_key_source = config
        .api_key
        .as_ref()
        .filter(|key| !key.is_empty())
        .map(|_| "settings.api_key".to_string())
        .or_else(|| {
            provider_cfg
                .and_then(|provider| provider.api_key.as_ref())
                .filter(|key| !key.is_empty())
                .map(|_| format!("settings.provider_configs.{active_provider}.api_key"))
        })
        .or(stored_api_key_source)
        .or(env_api_key_source)
        .or_else(|| {
            oauth_tokens
                .as_ref()
                .filter(|tokens| !tokens.uses_bearer_auth() && tokens.api_key.is_some())
                .map(|_| "/login managed key".to_string())
        });
    let token_source = oauth_tokens.as_ref().map(|tokens| {
        if tokens.uses_bearer_auth() {
            "claude.ai".to_string()
        } else {
            "console_oauth".to_string()
        }
    });
    let login_method = oauth_tokens
        .as_ref()
        .and_then(|tokens| subscription_label(tokens.subscription_type.as_deref()))
        .or_else(|| {
            oauth_tokens.as_ref().map(|tokens| {
                if tokens.uses_bearer_auth() {
                    "Claurst Account".to_string()
                } else {
                    "Console Account".to_string()
                }
            })
        })
        .or_else(|| api_key_source.as_ref().map(|_| "API Key".to_string()));
    let billing_mode = oauth_tokens.as_ref().map_or_else(
        || {
            if api_key_source.is_some() {
                "API".to_string()
            } else {
                "None".to_string()
            }
        },
        |tokens| {
            if tokens.uses_bearer_auth() {
                "Subscription".to_string()
            } else {
                "API".to_string()
            }
        },
    );

    let (auth_method, logged_in) = if let Some(ref tokens) = oauth_tokens {
        let method = if tokens.uses_bearer_auth() {
            "claude.ai"
        } else {
            "oauth_token"
        };
        (method.to_string(), true)
    } else if api_key_source.is_some() {
        ("api_key".to_string(), true)
    } else {
        ("none".to_string(), false)
    };

    if json_output {
        let mut obj = serde_json::json!({
            "loggedIn": logged_in,
            "authMethod": auth_method,
            "apiProvider": api_provider,
            "billing": billing_mode,
        });

        if let Some(ref source) = api_key_source {
            obj["apiKeySource"] = serde_json::Value::String(source.clone());
        }
        if let Some(ref source) = token_source {
            obj["tokenSource"] = serde_json::Value::String(source.clone());
        }
        if let Some(ref method) = login_method {
            obj["loginMethod"] = serde_json::Value::String(method.clone());
        }

        if let Some(ref tokens) = oauth_tokens {
            obj["email"] = json_null_or_string(&tokens.email);
            obj["orgId"] = json_null_or_string(&tokens.organization_uuid);
            obj["subscriptionType"] = json_null_or_string(&tokens.subscription_type);
        }

        println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
    } else {
        if !logged_in {
            let hint = if active_provider == "anthropic" {
                "Run `claude auth login` or set ANTHROPIC_API_KEY.".to_string()
            } else if let Some(env_var) =
                claurst_core::config::primary_api_key_env_var_for_provider(active_provider)
            {
                format!("Set {} or store a credential for {}.", env_var, api_provider)
            } else {
                format!("Configure credentials for {}.", api_provider)
            };
            println!("Not logged in for {}. {}", api_provider, hint);
        } else {
            println!("Logged in.");
            println!("  API provider: {}", api_provider);
            println!("  Billing: {}", billing_mode);
            if let Some(ref method) = login_method {
                println!("  Login method: {}", method);
            }
            if let Some(ref source) = token_source {
                println!("  Auth token: {}", source);
            }
            if let Some(ref source) = api_key_source {
                println!("  API key: {}", source);
            }
            match auth_method.as_str() {
                "claude.ai" | "oauth_token" => {
                    if let Some(ref tokens) = oauth_tokens {
                        if let Some(ref email) = tokens.email {
                            println!("  Email: {}", email);
                        }
                        if let Some(ref org) = tokens.organization_uuid {
                            println!("  Organization ID: {}", org);
                        } else {
                            println!("  Organization ID: unavailable");
                        }
                        if let Some(ref sub) = tokens.subscription_type {
                            println!("  Subscription: {}", sub);
                        }
                    }
                }
                "api_key" => {
                    println!("  Organization ID: unavailable for direct API key auth");
                }
                _ => {}
            }
        }
    }

    std::process::exit(if logged_in { 0 } else { 1 });
}

/// Clear all stored credentials and exit.
async fn auth_logout() {
    let mut had_error = false;

    // Clear OAuth tokens
    if let Err(e) = claurst_core::oauth::OAuthTokens::clear().await {
        eprintln!("Warning: failed to clear OAuth tokens: {}", e);
        had_error = true;
    }

    // Also clear any API key stored in settings.json
    match Settings::load().await {
        Ok(mut settings) => {
            if settings.config.api_key.is_some() {
                settings.config.api_key = None;
                if let Err(e) = settings.save().await {
                    eprintln!("Warning: failed to update settings.json: {}", e);
                    had_error = true;
                }
            }
        }
        Err(e) => {
            eprintln!("Warning: failed to load settings.json: {}", e);
        }
    }

    if had_error {
        eprintln!("Logout completed with warnings.");
        std::process::exit(1);
    } else {
        println!("Successfully logged out from your Anthropic account.");
        std::process::exit(0);
    }
}

/// Helper: convert `Option<String>` to a JSON string or null.
fn subscription_label(subscription_type: Option<&str>) -> Option<String> {
    match subscription_type? {
        "enterprise" => Some("Claurst Enterprise Account".to_string()),
        "team" => Some("Claurst Team Account".to_string()),
        "max" => Some("Claurst Max Account".to_string()),
        "pro" => Some("Claurst Pro Account".to_string()),
        other if !other.is_empty() => Some(format!("{} Account", other)),
        _ => None,
    }
}

/// Helper: convert `Option<String>` to a JSON string or null.
fn json_null_or_string(opt: &Option<String>) -> serde_json::Value {
    match opt {
        Some(s) => serde_json::Value::String(s.clone()),
        None => serde_json::Value::Null,
    }
}

