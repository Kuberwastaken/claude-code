//! Shared agent runtime owned by the ACP server.
//!
//! Built once on startup and reused for every session. Per-session state
//! (cwd, transcript, cancellation token, permission queue) is layered on
//! top via `sessions::SessionState`.

use std::path::PathBuf;
use std::sync::Arc;

use claurst_core::config::{Config, Settings};
use claurst_core::permissions::PermissionManager;
use claurst_core::CostTracker;
use claurst_query::QueryConfig;
use claurst_tools::Tool;

/// Snapshot of the global agent runtime — built at server startup, cloned
/// (cheaply, via Arc) into each session.
#[derive(Clone)]
pub struct AgentRuntime {
    pub config: Config,
    pub settings: Settings,
    pub api_client: Arc<claurst_api::AnthropicClient>,
    pub provider_registry: Arc<claurst_api::ProviderRegistry>,
    pub tools: Arc<Vec<Box<dyn Tool>>>,
    pub cost_tracker: Arc<CostTracker>,
    pub query_config: QueryConfig,
    pub mcp_manager: Option<Arc<claurst_mcp::McpManager>>,
    pub permission_manager: Arc<std::sync::Mutex<PermissionManager>>,
    pub working_dir: PathBuf,
}

impl AgentRuntime {
    /// Build the runtime from on-disk settings, env vars, and a working
    /// directory. Mirrors the headless startup path but with ACP-specific
    /// defaults (non-interactive, permission decisions routed back to the
    /// connected client).
    pub async fn build(working_dir: PathBuf) -> anyhow::Result<Self> {
        let settings = Settings::load_sync().unwrap_or_default();
        let mut config = settings.effective_config();
        // Plan mode requires interactive UI — fall back to Default so the
        // ACP permission bridge can route decisions to the client.
        if config.permission_mode == claurst_core::PermissionMode::Plan {
            config.permission_mode = claurst_core::PermissionMode::Default;
        }
        config.project_dir = Some(working_dir.clone());

        let active_provider = config.selected_provider_id().to_string();
        let (api_key, use_bearer_auth) = if active_provider == "anthropic" {
            config
                .resolve_anthropic_auth_async()
                .await
                .unwrap_or_default()
        } else {
            (String::new(), false)
        };

        let client_config = claurst_api::client::ClientConfig {
            api_key: api_key.clone(),
            api_base: config.resolve_anthropic_api_base(),
            use_bearer_auth,
            ..Default::default()
        };
        let api_client = Arc::new(claurst_api::AnthropicClient::new(client_config.clone())?);
        let provider_registry = Arc::new(claurst_api::ProviderRegistry::from_config(
            &config,
            client_config,
        ));

        let permission_manager = Arc::new(std::sync::Mutex::new(PermissionManager::new(
            config.permission_mode.clone(),
            &settings,
        )));

        let cost_tracker = CostTracker::new();

        // MCP servers from settings — connect upfront so their tools are
        // visible to every session. Per-session MCP servers supplied via
        // `session/new` params are additive on top of this (v1: ignored,
        // tracked in plan/migration-todo).
        let mcp_manager = build_mcp_manager(&config).await;

        // Build tools: built-ins + AgentTool. MCP tool wrappers are NOT
        // attached here — the wrapper type lives in the CLI crate today and
        // adding it would create a circular dep. Built-in tools (Bash, Read,
        // Edit, Glob, Grep, WebFetch, …) cover the common ACP-editor flows.
        let mut tools: Vec<Box<dyn Tool>> = claurst_tools::all_tools();
        tools.push(Box::new(claurst_query::AgentTool));
        let tools = Arc::new(tools);

        let mut query_config = QueryConfig::from_config(&config);
        query_config.working_directory = Some(working_dir.display().to_string());
        query_config.provider_registry = Some(provider_registry.clone());

        Ok(Self {
            config,
            settings,
            api_client,
            provider_registry,
            tools,
            cost_tracker,
            query_config,
            mcp_manager,
            permission_manager,
            working_dir,
        })
    }
}

async fn build_mcp_manager(config: &Config) -> Option<Arc<claurst_mcp::McpManager>> {
    if config.mcp_servers.is_empty() {
        return None;
    }
    let mgr = Arc::new(claurst_mcp::McpManager::connect_all(&config.mcp_servers).await);
    mgr.clone().spawn_notification_poll_loop();
    Some(mgr)
}
