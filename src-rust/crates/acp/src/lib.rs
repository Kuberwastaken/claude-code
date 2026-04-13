//! Agent Client Protocol (ACP) server for Claurst.
//!
//! Implements JSON-RPC 2.0 over stdio so that editors (Zed, VS Code, …) can
//! use Claurst as an AI back-end without launching a full TUI session.
//!
//! # Wire format
//! - Each message is a single UTF-8 line terminated with `\n`.
//! - Requests follow the JSON-RPC 2.0 schema.
//! - The server sends a `server/ready` notification immediately on startup.
//!
//! # Supported methods
//! | Method           | Description                                    |
//! |------------------|------------------------------------------------|
//! | `initialize`     | Handshake — returns server capabilities        |
//! | `session/create` | Create a new conversation session              |
//! | `session/message`| Send a message to a session (placeholder)      |
//! | `session/list`   | List recent sessions from SQLite store         |
//! | `tool/list`      | Enumerate the built-in tools                   |
//! | `model/list`     | List all models from the registry              |

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    pub fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<serde_json::Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError { code, message }),
        }
    }
}

// ---------------------------------------------------------------------------
// Server entry-point
// ---------------------------------------------------------------------------

/// Run the ACP server.
///
/// Reads newline-delimited JSON-RPC 2.0 requests from stdin, writes
/// responses to stdout.  Returns when stdin reaches EOF.
pub async fn run_acp_server() -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    // Send the server/ready notification before the first request.
    let capabilities = serde_json::json!({
        "name": "claurst",
        "version": env!("CARGO_PKG_VERSION"),
        "capabilities": {
            "sessions": true,
            "tools": true,
            "streaming": false
        }
    });
    let notif = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "server/ready",
        "params": capabilities
    });
    write_line(&mut stdout, &notif).await?;

    // Request loop.
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        debug!(method = ?line.get(..80), "ACP request");

        let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(req) => handle_request(req).await,
            Err(e) => {
                warn!(error = %e, "ACP parse error");
                JsonRpcResponse::error(None, -32700, format!("Parse error: {}", e))
            }
        };

        write_line(&mut stdout, &response).await?;
    }

    Ok(())
}

async fn write_line(stdout: &mut tokio::io::Stdout, value: &impl Serialize) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    stdout.write_all(line.as_bytes()).await?;
    stdout.flush().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Request dispatch
// ---------------------------------------------------------------------------

async fn handle_request(req: JsonRpcRequest) -> JsonRpcResponse {
    let id = req.id.clone();
    debug!(method = %req.method, "ACP dispatch");

    match req.method.as_str() {
        // ------------------------------------------------------------------
        "initialize" => JsonRpcResponse::success(
            id,
            serde_json::json!({
                "serverInfo": {
                    "name": "claurst",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "sessions": { "create": true, "list": true },
                    "tools":    { "list": true },
                    "models":   { "list": true }
                }
            }),
        ),

        // ------------------------------------------------------------------
        "tool/list" => JsonRpcResponse::success(
            id,
            serde_json::json!({
                "tools": [
                    { "name": "Bash",        "description": "Execute shell commands" },
                    { "name": "Read",        "description": "Read file contents" },
                    { "name": "Edit",        "description": "Edit file contents" },
                    { "name": "Write",       "description": "Write file contents" },
                    { "name": "Glob",        "description": "Find files by pattern" },
                    { "name": "Grep",        "description": "Search file contents" },
                    { "name": "WebSearch",   "description": "Search the web" },
                    { "name": "BatchEdit",   "description": "Edit multiple files atomically" },
                    { "name": "ApplyPatch",  "description": "Apply unified diff patch" },
                    { "name": "Lsp",         "description": "Language server protocol integration" },
                ]
            }),
        ),

        // ------------------------------------------------------------------
        "session/create" => {
            let session_id = format!("acp-{}", chrono::Utc::now().timestamp_millis());
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "session_id": session_id,
                    "status": "created"
                }),
            )
        }

        // ------------------------------------------------------------------
        "session/message" => {
            // Placeholder — full implementation would wire into QueryLoop.
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "status": "accepted",
                    "message": "Message received. Streaming support is not yet implemented."
                }),
            )
        }

        // ------------------------------------------------------------------
        "session/list" => {
            // Attempt to open the default SQLite store and list sessions.
            let sessions = try_list_sessions();
            JsonRpcResponse::success(id, serde_json::json!({ "sessions": sessions }))
        }

        // ------------------------------------------------------------------
        "model/list" => {
            let registry = claurst_api::ModelRegistry::new();
            let mut entries = registry.list_all();
            entries.sort_by(|a, b| {
                (&*a.info.provider_id)
                    .cmp(&*b.info.provider_id)
                    .then_with(|| (&*a.info.id).cmp(&*b.info.id))
            });
            let models: Vec<_> = entries
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "id":             format!("{}/{}", e.info.provider_id, e.info.id),
                        "name":           e.info.name,
                        "context_window": e.info.context_window,
                        "provider":       e.info.provider_id.to_string(),
                    })
                })
                .collect();
            JsonRpcResponse::success(id, serde_json::json!({ "models": models }))
        }

        // ------------------------------------------------------------------
        other => JsonRpcResponse::error(id, -32601, format!("Method not found: {}", other)),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Try to open the default SQLite store and return a JSON array of sessions.
/// On any error returns an empty array (ACP server must stay robust).
fn try_list_sessions() -> serde_json::Value {
    let db_path = claurst_core::config::Settings::config_dir().join("sessions.db");
    match claurst_core::SqliteSessionStore::open(&db_path) {
        Ok(store) => match store.list_sessions() {
            Ok(sessions) => {
                let arr: Vec<_> = sessions
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "id":            s.id,
                            "title":         s.title,
                            "model":         s.model,
                            "created_at":    s.created_at,
                            "updated_at":    s.updated_at,
                            "message_count": s.message_count,
                        })
                    })
                    .collect();
                serde_json::Value::Array(arr)
            }
            Err(e) => {
                warn!(error = %e, "ACP: failed to list sessions");
                serde_json::Value::Array(vec![])
            }
        },
        Err(e) => {
            warn!(error = %e, "ACP: failed to open SQLite store");
            serde_json::Value::Array(vec![])
        }
    }
}
