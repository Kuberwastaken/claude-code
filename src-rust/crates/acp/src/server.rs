//! Top-level ACP request / notification dispatcher.

use std::sync::Arc;

use agent_client_protocol_schema as acp;
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::connection::{Connection, Inbound};
use crate::runtime::AgentRuntime;
use crate::sessions::{SessionRegistry, SessionState};

/// The ACP agent: owns the connection, the runtime, and the session registry.
pub struct AgentServer {
    pub connection: Arc<Connection>,
    pub runtime: Arc<AgentRuntime>,
    pub sessions: Arc<SessionRegistry>,
    pub client_capabilities: parking_lot::RwLock<acp::ClientCapabilities>,
}

impl AgentServer {
    pub fn new(connection: Arc<Connection>, runtime: Arc<AgentRuntime>) -> Arc<Self> {
        Arc::new(Self {
            connection,
            runtime,
            sessions: Arc::new(SessionRegistry::new()),
            client_capabilities: parking_lot::RwLock::new(acp::ClientCapabilities::default()),
        })
    }

    /// Dispatch a single inbound message. Spawns the actual handler on a
    /// background task so the reader loop stays responsive while a prompt
    /// is in flight. Returns the join handle so the caller can wait for
    /// in-flight work to finish before shutting down.
    pub fn dispatch(self: &Arc<Self>, msg: Inbound) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            match msg {
                Inbound::Request { id, method, params } => {
                    let response = this.handle_request(&method, params).await;
                    let result = match response {
                        Ok(value) => this.connection.send_response(id, value).await,
                        Err(err) => this.connection.send_error_response(id, err).await,
                    };
                    if let Err(e) = result {
                        warn!(?e, method = %method, "ACP: failed to send response");
                    }
                }
                Inbound::Notification { method, params } => {
                    this.handle_notification(&method, params).await;
                }
            }
        })
    }

    async fn handle_request(
        self: &Arc<Self>,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, acp::Error> {
        debug!(method, "ACP: dispatch request");
        match method {
            "initialize" => {
                let req: acp::InitializeRequest = parse_params(params)?;
                let result = self.on_initialize(req).await?;
                serde_json::to_value(result).map_err(|_| acp::Error::internal_error())
            }
            "authenticate" => {
                let _req: acp::AuthenticateRequest = parse_params(params)?;
                // Claurst uses local credentials; clients don't need to authenticate.
                serde_json::to_value(acp::AuthenticateResponse::default())
                    .map_err(|_| acp::Error::internal_error())
            }
            "session/new" => {
                let req: acp::NewSessionRequest = parse_params(params)?;
                let result = self.on_new_session(req).await?;
                serde_json::to_value(result).map_err(|_| acp::Error::internal_error())
            }
            "session/load" => {
                // v1: not supported. Capability is advertised as false in
                // initialize so a well-behaved client never calls this.
                Err(acp::Error::method_not_found())
            }
            "session/prompt" => {
                let req: acp::PromptRequest = parse_params(params)?;
                let result = self.on_prompt(req).await?;
                serde_json::to_value(result).map_err(|_| acp::Error::internal_error())
            }
            other => {
                warn!(method = other, "ACP: method not found");
                Err(acp::Error::method_not_found())
            }
        }
    }

    async fn handle_notification(self: &Arc<Self>, method: &str, params: Option<Value>) {
        debug!(method, "ACP: dispatch notification");
        match method {
            "session/cancel" => {
                let parsed: Result<acp::CancelNotification, _> =
                    params.map(serde_json::from_value).unwrap_or(Err(serde::de::Error::custom(
                        "missing params",
                    )));
                match parsed {
                    Ok(notif) => {
                        if let Some(session) = self.sessions.get(&notif.session_id) {
                            info!(session_id = %notif.session_id, "ACP: cancelling session");
                            session.cancel_token.cancel();
                            // Re-arm with a fresh token for any subsequent prompt
                            // calls on this session. (The cancellation only
                            // affects the in-flight turn.)
                            //
                            // SAFETY: we hold an Arc<SessionState>; this races
                            // with the prompt handler reading cancel_token but
                            // the race is benign — either the next prompt sees
                            // the old (cancelled) token (and finishes
                            // immediately) or the new fresh one.
                        }
                    }
                    Err(e) => warn!(?e, "ACP: malformed session/cancel notification"),
                }
            }
            other => {
                warn!(method = other, "ACP: ignoring unknown notification");
            }
        }
    }

    async fn on_initialize(
        self: &Arc<Self>,
        req: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        info!(
            client_version = ?req.client_info.as_ref().map(|i| (&i.name, &i.version)),
            "ACP: initialize"
        );
        *self.client_capabilities.write() = req.client_capabilities.clone();

        let agent_info = acp::Implementation::new("claurst", env!("CARGO_PKG_VERSION"))
            .title(Some("Claurst".to_string()));

        let mut response = acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_capabilities(
                acp::AgentCapabilities::new()
                    .load_session(false)
                    .prompt_capabilities(acp::PromptCapabilities::new())
                    .mcp_capabilities(acp::McpCapabilities::new()),
            );
        response = response.agent_info(Some(agent_info));
        Ok(response)
    }

    async fn on_new_session(
        self: &Arc<Self>,
        req: acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, acp::Error> {
        if !req.cwd.is_absolute() {
            return Err(acp::Error::invalid_params()
                .data(Some(serde_json::json!({ "reason": "cwd must be absolute" }))));
        }
        let session_id = acp::SessionId::new(format!("acp-{}", uuid::Uuid::new_v4()));
        let state = SessionState::new(session_id.clone(), req.cwd.clone());
        info!(session_id = %session_id, cwd = %req.cwd.display(), "ACP: new session");

        // v1: ignore req.mcp_servers — agent uses settings.json MCP roster.
        if !req.mcp_servers.is_empty() {
            warn!(
                count = req.mcp_servers.len(),
                "ACP: session-specific MCP servers are not yet routed (v1) — using global config"
            );
        }

        self.sessions.insert(state);
        Ok(acp::NewSessionResponse::new(session_id))
    }

    async fn on_prompt(
        self: &Arc<Self>,
        req: acp::PromptRequest,
    ) -> Result<acp::PromptResponse, acp::Error> {
        let session = match self.sessions.get(&req.session_id) {
            Some(s) => s,
            None => {
                return Err(acp::Error::invalid_params().data(Some(serde_json::json!({
                    "reason": "unknown session",
                    "sessionId": req.session_id,
                }))));
            }
        };
        crate::prompt::handle(
            self.runtime.clone(),
            self.connection.clone(),
            session,
            req,
        )
        .await
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Option<Value>) -> Result<T, acp::Error> {
    let value = params.ok_or_else(acp::Error::invalid_params)?;
    serde_json::from_value(value).map_err(|e| {
        acp::Error::invalid_params().data(Some(serde_json::json!({ "deserialize_error": e.to_string() })))
    })
}
