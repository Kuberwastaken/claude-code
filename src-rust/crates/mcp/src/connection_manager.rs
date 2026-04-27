// cc-mcp: Connection manager with reconnection and lifecycle management.
//
// Mirrors the reconnection logic from the TS useManageMCPConnections hook.
// Wraps McpClient with:
//   - per-server status tracking
//   - exponential-backoff reconnection loops
//   - connect / disconnect / restart control plane

use crate::client::McpClient;
use crate::expand_server_config;
use crate::oauth;
use claurst_core::config::McpServerConfig;
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Live connection status for a single MCP server.
#[derive(Debug, Clone)]
pub enum McpServerStatus {
    /// Successfully connected; reports how many tools were discovered.
    Connected { tool_count: usize },
    /// Connection attempt in progress.
    Connecting,
    /// Cleanly disconnected (or not yet attempted).
    Disconnected { last_error: Option<String> },
    /// Connection failed; a retry is scheduled.
    Failed {
        error: String,
        retry_at: Instant,
    },
}

impl McpServerStatus {
    /// Human-readable one-liner for display in `/mcp status`.
    pub fn display(&self) -> String {
        match self {
            McpServerStatus::Connected { tool_count } => {
                format!("connected ({} tool{})", tool_count, if *tool_count == 1 { "" } else { "s" })
            }
            McpServerStatus::Connecting => "connecting…".to_string(),
            McpServerStatus::Disconnected { last_error: None } => "disconnected".to_string(),
            McpServerStatus::Disconnected { last_error: Some(e) } => {
                format!("disconnected ({})", e)
            }
            McpServerStatus::Failed { error, retry_at } => {
                let secs = retry_at.saturating_duration_since(Instant::now()).as_secs();
                format!("failed – {} (retry in {}s)", error, secs)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal shared state
// ---------------------------------------------------------------------------

struct ServerState {
    config: McpServerConfig,
    status: McpServerStatus,
    client: Option<Arc<McpClient>>,
}

// ---------------------------------------------------------------------------
// McpConnectionManager
// ---------------------------------------------------------------------------

/// Manages lifecycle (connect / disconnect / reconnect) for a set of MCP servers.
pub struct McpConnectionManager {
    /// Per-server mutable state, guarded by a single Mutex.
    /// (DashMap per-entry would race on status ↔ client updates.)
    state: Arc<DashMap<String, Arc<Mutex<ServerState>>>>,
    /// Reconnect background task handles, keyed by server name.
    reconnect_handles: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl McpConnectionManager {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Create a manager from a map of configs.  Does not connect yet.
    pub fn new(configs: HashMap<String, McpServerConfig>) -> Self {
        let state: DashMap<String, Arc<Mutex<ServerState>>> = DashMap::new();
        for (name, config) in configs {
            state.insert(
                name,
                Arc::new(Mutex::new(ServerState {
                    config,
                    status: McpServerStatus::Disconnected { last_error: None },
                    client: None,
                })),
            );
        }
        Self {
            state: Arc::new(state),
            reconnect_handles: Mutex::new(HashMap::new()),
        }
    }

    /// Connect to all configured servers (env-vars expanded, errors non-fatal).
    pub async fn connect_all(&self) -> anyhow::Result<()> {
        let names: Vec<String> = self.state.iter().map(|e| e.key().clone()).collect();
        for name in names {
            if let Err(e) = self.connect(&name).await {
                error!(server = %name, error = %e, "MCP server failed to connect during startup");
            }
        }
        Ok(())
    }

    async fn connect_expanded_config(name: &str, config: &McpServerConfig) -> anyhow::Result<McpClient> {
        let auth_token = if matches!(config.server_type.as_str(), "sse" | "http") {
            match config.url.as_deref() {
                Some(server_url) => oauth::get_valid_mcp_access_token(name, server_url).await?,
                None => {
                    anyhow::bail!(
                        "MCP server '{}' is configured as '{}' but missing URL",
                        name,
                        config.server_type
                    )
                }
            }
        } else {
            None
        };
        McpClient::connect(config, auth_token).await
    }

    // -----------------------------------------------------------------------
    // Connect / disconnect / restart
    // -----------------------------------------------------------------------

    /// Connect to a single server by name, marking status along the way.
    pub async fn connect(&self, name: &str) -> anyhow::Result<()> {
        let entry = self
            .state
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown MCP server: {}", name))?;
        let state_arc = entry.value().clone();
        drop(entry); // release dashmap read-guard

        {
            let mut st = state_arc.lock().await;
            st.status = McpServerStatus::Connecting;
        }

        let config = {
            let st = state_arc.lock().await;
            expand_server_config(&st.config)
        };

        debug!(server = %name, transport = %config.server_type, command = ?config.command, url = ?config.url, "Connecting to MCP server");

        match Self::connect_expanded_config(name, &config).await {
            Ok(client) => {
                let tool_count = client.tools.len();
                let client_arc = Arc::new(client);
                let mut st = state_arc.lock().await;
                st.client = Some(client_arc);
                st.status = McpServerStatus::Connected { tool_count };
                info!(server = %name, transport = %config.server_type, tools = tool_count, "MCP server connected");
                Ok(())
            }
            Err(e) => {
                let msg = e.to_string();
                let mut st = state_arc.lock().await;
                st.client = None;
                st.status = McpServerStatus::Disconnected {
                    last_error: Some(msg.clone()),
                };
                Err(anyhow::anyhow!("MCP server '{}' connection failed: {}", name, msg))
            }
        }
    }

    /// Disconnect a server and cancel its reconnect loop.
    pub async fn disconnect(&self, name: &str) {
        {
            let mut handles = self.reconnect_handles.lock().await;
            if let Some(handle) = handles.remove(name) {
                handle.abort();
            }
        }

        if let Some(entry) = self.state.get(name) {
            let mut st = entry.value().lock().await;
            st.client = None;
            st.status = McpServerStatus::Disconnected { last_error: None };
        }

        debug!(server = %name, "MCP server disconnected");
    }

    /// Disconnect then reconnect a server.
    pub async fn restart(&self, name: &str) -> anyhow::Result<()> {
        info!(server = %name, "Restarting MCP server");
        self.disconnect(name).await;
        self.connect(name).await
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// Returns `true` when the named server has an active client.
    pub fn is_connected(&self, name: &str) -> bool {
        self.state
            .get(name)
            .map(|e| {
                e.value()
                    .try_lock()
                    .map(|st| st.client.is_some())
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// Return status for every server.
    pub fn all_statuses(&self) -> HashMap<String, McpServerStatus> {
        let mut map = HashMap::new();
        for entry in self.state.iter() {
            let name = entry.key().clone();
            let status = entry
                .value()
                .try_lock()
                .map(|st| st.status.clone())
                .unwrap_or(McpServerStatus::Connecting);
            map.insert(name, status);
        }
        map
    }

    /// Return status for a single server.
    pub fn server_status(&self, name: &str) -> Option<McpServerStatus> {
        self.state.get(name).map(|e| {
            e.value()
                .try_lock()
                .map(|st| st.status.clone())
                .unwrap_or(McpServerStatus::Connecting)
        })
    }

    /// Return names of all configured servers (connected or not).
    pub fn server_names(&self) -> Vec<String> {
        self.state.iter().map(|e| e.key().clone()).collect()
    }

    /// Return the `McpClient` for a connected server, or `None` if disconnected.
    pub fn client(&self, name: &str) -> Option<Arc<McpClient>> {
        self.state
            .get(name)
            .and_then(|e| e.value().try_lock().ok().and_then(|st| st.client.clone()))
    }

    // -----------------------------------------------------------------------
    // Automatic reconnection
    // -----------------------------------------------------------------------

    /// Start a background reconnection loop for `name`.
    /// The loop exits when the server connects successfully.
    ///
    /// Backoff: 1 s → 2 s → 4 s → … capped at 60 s.
    pub async fn start_reconnect_loop(&self, name: &str) {
        {
            let handles = self.reconnect_handles.lock().await;
            if handles.contains_key(name) {
                return;
            }
        }

        let name = name.to_string();
        let state = self.state.clone();

        let name_for_map = name.clone();
        let handle = tokio::spawn(async move {
            Self::reconnect_loop(name, state).await;
        });

        self.reconnect_handles
            .lock()
            .await
            .insert(name_for_map, handle);
    }

    /// Background loop: wait, then attempt reconnect with exponential backoff.
    async fn reconnect_loop(
        name: String,
        state: Arc<DashMap<String, Arc<Mutex<ServerState>>>>,
    ) {
        let mut backoff = Duration::from_secs(1);
        const MAX_BACKOFF: Duration = Duration::from_secs(60);

        loop {
            let retry_at = Instant::now() + backoff;

            if let Some(entry) = state.get(&name) {
                if let Ok(mut st) = entry.value().try_lock() {
                    let prev_error = match &st.status {
                        McpServerStatus::Disconnected { last_error: Some(e) } => e.clone(),
                        McpServerStatus::Failed { error, .. } => error.clone(),
                        _ => "connection lost".to_string(),
                    };
                    st.status = McpServerStatus::Failed {
                        error: prev_error,
                        retry_at,
                    };
                }
            }

            warn!(
                server = %name,
                backoff_secs = backoff.as_secs(),
                "MCP server disconnected; will retry"
            );

            tokio::time::sleep(backoff).await;

            let config = match state.get(&name) {
                Some(entry) => match entry.value().try_lock() {
                    Ok(st) => expand_server_config(&st.config),
                    Err(_) => {
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                },
                None => {
                    debug!(server = %name, "MCP server removed; stopping reconnect loop");
                    break;
                }
            };

            if let Some(entry) = state.get(&name) {
                if let Ok(mut st) = entry.value().try_lock() {
                    st.status = McpServerStatus::Connecting;
                }
            }

            info!(server = %name, transport = %config.server_type, attempt_backoff_secs = backoff.as_secs(), "Attempting MCP reconnect");

            match Self::connect_expanded_config(&name, &config).await {
                Ok(client) => {
                    let tool_count = client.tools.len();
                    let client_arc = Arc::new(client);
                    if let Some(entry) = state.get(&name) {
                        if let Ok(mut st) = entry.value().try_lock() {
                            st.client = Some(client_arc);
                            st.status = McpServerStatus::Connected { tool_count };
                        }
                    }
                    info!(server = %name, transport = %config.server_type, tools = tool_count, "MCP server reconnected successfully");
                    break;
                }
                Err(e) => {
                    let msg = e.to_string();
                    error!(server = %name, transport = %config.server_type, error = %msg, "MCP reconnect attempt failed");
                    if let Some(entry) = state.get(&name) {
                        if let Ok(mut st) = entry.value().try_lock() {
                            st.status = McpServerStatus::Disconnected {
                                last_error: Some(msg),
                            };
                        }
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_configs(names: &[&str]) -> HashMap<String, McpServerConfig> {
        names
            .iter()
            .map(|&n| {
                (
                    n.to_string(),
                    McpServerConfig {
                        name: n.to_string(),
                        command: Some("echo".to_string()),
                        args: vec![],
                        env: std::collections::HashMap::new(),
                        url: None,
                        server_type: "stdio".to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn test_server_names() {
        let mgr = McpConnectionManager::new(make_configs(&["a", "b", "c"]));
        let mut names = mgr.server_names();
        names.sort();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_initial_status_disconnected() {
        let mgr = McpConnectionManager::new(make_configs(&["srv"]));
        let status = mgr.server_status("srv").unwrap();
        assert!(matches!(status, McpServerStatus::Disconnected { .. }));
    }

    #[test]
    fn test_is_connected_false_initially() {
        let mgr = McpConnectionManager::new(make_configs(&["srv"]));
        assert!(!mgr.is_connected("srv"));
    }

    #[test]
    fn test_unknown_server_status_is_none() {
        let mgr = McpConnectionManager::new(make_configs(&["srv"]));
        assert!(mgr.server_status("no-such-server").is_none());
    }

    #[test]
    fn test_all_statuses() {
        let mgr = McpConnectionManager::new(make_configs(&["x", "y"]));
        let statuses = mgr.all_statuses();
        assert_eq!(statuses.len(), 2);
        assert!(statuses.contains_key("x"));
        assert!(statuses.contains_key("y"));
    }
}
