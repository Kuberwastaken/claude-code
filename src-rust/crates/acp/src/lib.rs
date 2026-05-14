//! Agent Client Protocol (ACP) server for Claurst.
//!
//! ACP is the open protocol pioneered by Zed for standardizing communication
//! between AI coding agents and editors (Zed, Neovim, JetBrains, VS Code, …).
//! Spec: <https://agentclientprotocol.com>
//!
//! This crate turns the local `claurst` binary into a compliant ACP agent
//! over newline-delimited JSON-RPC 2.0 on stdio. Editors launch `claurst acp`
//! as a subprocess and drive it through the protocol's standard methods:
//!
//! | Method                       | Direction  | Notes                                       |
//! |------------------------------|------------|---------------------------------------------|
//! | `initialize`                 | C → A      | Capability negotiation                      |
//! | `authenticate`               | C → A      | No-op (Claurst uses local credentials)      |
//! | `session/new`                | C → A      | Create a session with cwd + MCP roster      |
//! | `session/prompt`             | C → A      | Run a turn; streams `session/update` events |
//! | `session/cancel`             | C → A (no resp) | Cancel an in-flight prompt             |
//! | `session/update`             | A → C (no resp) | Streamed text/tool deltas              |
//! | `session/request_permission` | A → C      | Tool approval dialog                        |
//!
//! Per-session MCP server configs supplied via `session/new` are accepted
//! but currently ignored in favour of the global `settings.json` MCP roster.
//! This will be resolved as part of the planned unified MCP routing work
//! (see `src-rust/plan/migration-todo.md`).

mod connection;
mod permission;
mod prompt;
mod runtime;
mod server;
mod sessions;

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{error, info};

pub use connection::Connection;
pub use runtime::AgentRuntime;
pub use server::AgentServer;

/// Run the ACP server on the current process' stdin / stdout. Returns when
/// stdin reaches EOF or when the runtime fails to initialize.
pub async fn run_acp_server() -> anyhow::Result<()> {
    // We must NEVER write to stdout outside the protocol — every byte on
    // stdout is parsed by the client as JSON-RPC. Force logs to stderr.
    install_stderr_tracing();

    let working_dir = std::env::current_dir()?;
    info!(cwd = %working_dir.display(), version = env!("CARGO_PKG_VERSION"), "ACP: starting server");

    let runtime = AgentRuntime::build(working_dir).await?;
    let runtime = Arc::new(runtime);
    let connection = Connection::new(tokio::io::stdout());
    let server = AgentServer::new(connection.clone(), runtime);

    let (tx, mut rx) = mpsc::unbounded_channel();
    let reader_fut = connection::run_reader(connection, tokio::io::stdin(), tx);

    // Track in-flight dispatch tasks so they can finish writing their
    // responses before the runtime shuts down. The dispatch future only
    // returns once the reader has dropped `tx` (closing `rx`) AND every
    // spawned handler has resolved.
    let dispatch_fut = async {
        let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
        while let Some(msg) = rx.recv().await {
            tasks.push(server.dispatch(msg));
        }
        for handle in tasks {
            let _ = handle.await;
        }
    };

    let (reader_res, _) = tokio::join!(reader_fut, dispatch_fut);
    if let Err(e) = reader_res {
        error!(?e, "ACP: reader loop failed");
    }

    info!("ACP: server shutdown");
    Ok(())
}

fn install_stderr_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_env("CLAURST_ACP_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .try_init();
}
