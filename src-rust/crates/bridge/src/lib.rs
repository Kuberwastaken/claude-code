// cc-bridge: Remote control bridge implementation.
//
// The bridge connects the local Claurst CLI to the claude.ai web UI,
// enabling mobile/web-initiated sessions. This module implements:
//
// - Bridge configuration management (env-var and defaults)
// - Device fingerprinting for trusted-device identification
// - JWT decode/expiry utilities (client-side, no signature verification)
// - Session lifecycle (register, poll, upload events, deregister)
// - Message and event protocol types for bidirectional communication
// - Long-polling loop with exponential backoff and cancellation
// - Public `start_bridge` API that spawns background task and returns channels
//
// Architecture mirrors the TypeScript bridge (bridgeMain.ts / bridgeApi.ts),
// adapted to idiomatic Rust async with tokio channels and reqwest.

#![warn(clippy::all)]

use anyhow::Context;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// JWT utilities
// ---------------------------------------------------------------------------

/// Decoded claims from a session-ingress JWT.
///
/// Parsed client-side without signature verification — used only for
/// expiry checks and display, never for authorization decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// Subject (usually user / device identifier).
    pub sub: Option<String>,
    /// Expiry Unix timestamp (seconds).
    pub exp: Option<i64>,
    /// Issued-at Unix timestamp (seconds).
    pub iat: Option<i64>,
    /// Trusted-device identifier embedded by the server.
    pub device_id: Option<String>,
    /// Session identifier embedded by the server.
    pub session_id: Option<String>,
}

impl JwtClaims {
    /// Decode a JWT payload segment without verifying the signature.
    ///
    /// Strips the `sk-ant-si-` session-ingress prefix if present, then
    /// base64url-decodes the second `.`-separated segment and JSON-parses it.
    /// Returns an error if the token is malformed or the JSON is invalid.
    pub fn decode(token: &str) -> anyhow::Result<Self> {
        // Strip session-ingress prefix used by Anthropic's ingress tokens.
        let jwt = if token.starts_with("sk-ant-si-") {
            &token["sk-ant-si-".len()..]
        } else {
            token
        };

        let parts: Vec<&str> = jwt.split('.').collect();
        if parts.len() < 2 {
            anyhow::bail!("Invalid JWT: expected at least 2 dot-separated segments");
        }

        let raw = URL_SAFE_NO_PAD
            .decode(parts[1])
            .context("JWT payload is not valid base64url")?;

        serde_json::from_slice::<Self>(&raw)
            .context("JWT payload is not valid JSON matching JwtClaims")
    }

    /// Returns `true` if the `exp` claim is in the past.
    ///
    /// When `exp` is absent the token is treated as non-expired (permissive
    /// default), matching the TypeScript behaviour in `jwtUtils.ts`.
    pub fn is_expired(&self) -> bool {
        if let Some(exp) = self.exp {
            let now = chrono::Utc::now().timestamp();
            exp < now
        } else {
            false
        }
    }

    /// Remaining lifetime in seconds, or `None` if no `exp` claim or already
    /// expired.
    pub fn remaining_secs(&self) -> Option<i64> {
        let exp = self.exp?;
        let now = chrono::Utc::now().timestamp();
        let diff = exp - now;
        if diff > 0 {
            Some(diff)
        } else {
            None
        }
    }
}

/// Decode just the expiry timestamp from a raw JWT string.
/// Returns `None` if the token is malformed or has no `exp` claim.
pub fn decode_jwt_expiry(token: &str) -> Option<i64> {
    JwtClaims::decode(token).ok()?.exp
}

/// Returns `true` if the token is expired (or unparseable).
pub fn jwt_is_expired(token: &str) -> bool {
    JwtClaims::decode(token)
        .map(|c| c.is_expired())
        .unwrap_or(true)
}

// ---------------------------------------------------------------------------
// Device fingerprint
// ---------------------------------------------------------------------------

/// Compute a stable device fingerprint from machine-local information.
///
/// Combines hostname, login user name, and home directory path, then SHA-256
/// hashes them and returns the full hex digest. Matching the TypeScript
/// `trustedDevice.ts` algorithm so fingerprints are consistent across the
/// two implementations.
pub fn device_fingerprint() -> String {
    let mut input = String::with_capacity(128);

    if let Ok(host) = hostname::get() {
        input.push_str(&host.to_string_lossy());
    }
    input.push(':');

    if let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("USERNAME")) {
        input.push_str(&user);
    }
    input.push(':');

    if let Some(home) = dirs::home_dir() {
        input.push_str(&home.display().to_string());
    }

    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// Bridge configuration
// ---------------------------------------------------------------------------

/// Runtime configuration for the bridge subsystem.
///
/// Built either from env vars via [`BridgeConfig::from_env`] or manually
/// by the caller. The bridge is only active when both `enabled` is `true`
/// **and** a `session_token` is present (see [`BridgeConfig::is_active`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    /// Whether the bridge feature is turned on.
    pub enabled: bool,
    /// Base URL for bridge API calls (e.g. `https://claude.ai`).
    pub server_url: String,
    /// Stable device identifier (SHA-256 fingerprint or custom value).
    pub device_id: String,
    /// Bearer token (OAuth access token or session-ingress JWT).
    pub session_token: Option<String>,
    /// How long to wait between poll cycles (milliseconds).
    pub polling_interval_ms: u64,
    /// Maximum successive failed polls before the loop gives up.
    pub max_reconnect_attempts: u32,
    /// Per-session inactivity timeout in milliseconds (default 24 h).
    pub session_timeout_ms: u64,
    /// Runner version string sent on API calls for server-side diagnostics.
    pub runner_version: String,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_url: "https://claude.ai".to_string(),
            device_id: device_fingerprint(),
            session_token: None,
            polling_interval_ms: 1_000,
            max_reconnect_attempts: 10,
            session_timeout_ms: 24 * 60 * 60 * 1_000,
            runner_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

impl BridgeConfig {
    /// Build config from environment variables.
    ///
    /// Recognised variables:
    /// - `CLAURST_BRIDGE_URL` — overrides `server_url` and sets `enabled = true`
    /// - `CLAURST_BRIDGE_TOKEN` / `CLAUDE_BRIDGE_OAUTH_TOKEN` — sets `session_token`
    /// - `CLAUDE_BRIDGE_BASE_URL` — alternative URL override (ant-only dev override)
    pub fn from_env() -> Self {
        let mut config = Self::default();

        // URL override (sets enabled implicitly)
        if let Ok(url) =
            std::env::var("CLAURST_BRIDGE_URL").or_else(|_| std::env::var("CLAUDE_BRIDGE_BASE_URL"))
        {
            if !url.is_empty() {
                config.server_url = url;
                config.enabled = true;
            }
        }

        // Token override
        if let Ok(token) = std::env::var("CLAURST_BRIDGE_TOKEN")
            .or_else(|_| std::env::var("CLAUDE_BRIDGE_OAUTH_TOKEN"))
        {
            if !token.is_empty() {
                config.session_token = Some(token);
            }
        }

        config
    }

    /// Returns `true` only when the bridge is both enabled and has a token.
    pub fn is_active(&self) -> bool {
        self.enabled && self.session_token.is_some()
    }

    /// Validate that a server-provided ID is safe to interpolate into a URL
    /// path segment. Prevents path traversal (e.g. `../../admin`).
    ///
    /// Mirrors `validateBridgeId()` in `bridgeApi.ts`.
    pub fn validate_id<'a>(id: &'a str, label: &str) -> anyhow::Result<&'a str> {
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let re = RE.get_or_init(|| regex::Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap());
        if id.is_empty() || !re.is_match(id) {
            anyhow::bail!("Invalid {}: contains unsafe characters", label);
        }
        Ok(id)
    }
}

// ---------------------------------------------------------------------------
// Permission decision
// ---------------------------------------------------------------------------

/// A tool-use permission decision sent by the web UI back to the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    AllowPermanently,
    Deny,
    DenyPermanently,
}

// ---------------------------------------------------------------------------
// Bridge message types (web UI → CLI)
// ---------------------------------------------------------------------------

/// A file attachment bundled with an inbound user message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeAttachment {
    /// Display name (filename or label).
    pub name: String,
    /// Raw text or base64-encoded content.
    pub content: String,
    /// MIME type, e.g. `"text/plain"`.
    pub mime_type: Option<String>,
}

/// Messages flowing from the web UI into the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeMessage {
    /// A new user prompt from the web UI.
    UserMessage {
        content: String,
        session_id: String,
        message_id: String,
        #[serde(default)]
        attachments: Vec<BridgeAttachment>,
    },
    /// The web UI has responded to a permission request.
    PermissionResponse {
        request_id: String,
        tool_use_id: Option<String>,
        decision: PermissionDecision,
    },
    /// Cancel the in-progress operation for a session.
    Cancel {
        session_id: String,
        reason: Option<String>,
    },
    /// Keepalive — the CLI should respond with a `Pong` event.
    Ping,
}

// ---------------------------------------------------------------------------
// Bridge event types (CLI → web UI)
// ---------------------------------------------------------------------------

/// Token-budget / cost summary attached to `TurnComplete`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: Option<f64>,
}

/// Session connection state broadcast to the web UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeSessionState {
    Connecting,
    Connected,
    Idle,
    Processing,
    Disconnected,
    Error,
}

/// Events flowing from the CLI up to the web UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// Streaming text delta for the current assistant turn.
    TextDelta {
        text: String,
        message_id: String,
        index: Option<usize>,
    },
    /// A tool call has started executing.
    ToolStart {
        tool_name: String,
        tool_id: String,
        input_preview: Option<String>,
    },
    /// A tool call has finished.
    ToolEnd {
        tool_name: String,
        tool_id: String,
        result: String,
        is_error: bool,
    },
    /// The CLI needs the web UI to approve a tool use.
    PermissionRequest {
        request_id: String,
        tool_use_id: String,
        tool_name: String,
        description: String,
        options: Vec<String>,
    },
    /// The current turn has completed.
    TurnComplete {
        message_id: String,
        stop_reason: String,
        usage: Option<BridgeUsage>,
    },
    /// A non-fatal diagnostic or user-visible error message.
    Error {
        message: String,
        code: Option<String>,
    },
    /// Response to a `Ping` message.
    Pong { server_time: Option<u64> },
    /// Session lifecycle state change.
    SessionState {
        session_id: String,
        state: BridgeSessionState,
    },
}

// ---------------------------------------------------------------------------
// Bridge session state (internal)
// ---------------------------------------------------------------------------

/// Internal connection state of a [`BridgeSession`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeState {
    Disconnected,
    Connecting,
    Connected,
    Running,
    Error(String),
}

// ---------------------------------------------------------------------------
// Bridge session
// ---------------------------------------------------------------------------

/// Active bridge session: owns the HTTP client, session credentials, and
/// state. Runs the poll loop in a background tokio task.
pub struct BridgeSession {
    config: BridgeConfig,
    session_id: String,
    state: Arc<RwLock<BridgeState>>,
    http: reqwest::Client,
    reconnect_count: u32,
    #[allow(dead_code)]
    last_ping: Option<std::time::Instant>,
}

impl BridgeSession {
    /// Create a new bridge session; generates a fresh UUID for `session_id`.
    pub fn new(config: BridgeConfig) -> Self {
        let session_id = uuid::Uuid::new_v4().to_string();
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(format!("claude-code-rust/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("Failed to build reqwest client");

        Self {
            config,
            session_id,
            state: Arc::new(RwLock::new(BridgeState::Connecting)),
            http,
            reconnect_count: 0,
            last_ping: None,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn current_state(&self) -> BridgeState {
        self.state.read().clone()
    }

    fn set_state(&self, s: BridgeState) {
        *self.state.write() = s;
    }

    // -----------------------------------------------------------------------
    // Session registration / deregistration
    // -----------------------------------------------------------------------

    /// Register this bridge session with the CCR server.
    ///
    /// POST `/api/claude_code/sessions` — mirrors the TypeScript
    /// `registerBridgeEnvironment` call in `bridgeApi.ts`.
    pub async fn register(&mut self) -> anyhow::Result<()> {
        let token = self
            .config
            .session_token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Bridge register: no session token"))?;

        let url = format!("{}/api/claude_code/sessions", self.config.server_url);

        let body = serde_json::json!({
            "session_id": self.session_id,
            "device_id": self.config.device_id,
            "client_version": self.config.runner_version,
        });

        debug!(session_id = %self.session_id, url = %url, "Registering bridge session");

        let resp = self
            .http
            .post(&url)
            .bearer_auth(token)
            .header("anthropic-version", "2023-06-01")
            .header("x-environment-runner-version", &self.config.runner_version)
            .json(&body)
            .send()
            .await
            .context("Bridge register: HTTP send failed")?;

        let status = resp.status().as_u16();
        match status {
            200 | 201 => {
                self.set_state(BridgeState::Connected);
                info!(session_id = %self.session_id, "Bridge session registered");
                Ok(())
            }
            401 | 403 => {
                self.set_state(BridgeState::Error(format!("Auth error: {status}")));
                anyhow::bail!("Bridge register: auth error ({})", status)
            }
            _ => {
                anyhow::bail!("Bridge register: server returned {}", status)
            }
        }
    }

    /// Deregister the session on clean shutdown.
    ///
    /// DELETE `/api/claude_code/sessions/{id}` — best-effort; errors are
    /// logged and swallowed so they don't block process exit.
    pub async fn deregister(&self) {
        let Some(token) = self.config.session_token.as_deref() else {
            return;
        };

        let url = format!(
            "{}/api/claude_code/sessions/{}",
            self.config.server_url, self.session_id
        );

        debug!(session_id = %self.session_id, "Deregistering bridge session");

        match self.http.delete(&url).bearer_auth(token).send().await {
            Ok(r) if r.status().is_success() => {
                info!(session_id = %self.session_id, "Bridge session deregistered");
            }
            Ok(r) => {
                warn!(
                    session_id = %self.session_id,
                    status = %r.status(),
                    "Bridge deregister returned non-success (ignored)"
                );
            }
            Err(e) => {
                warn!(
                    session_id = %self.session_id,
                    error = %e,
                    "Bridge deregister HTTP error (ignored)"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Polling
    // -----------------------------------------------------------------------

    /// Long-poll for incoming messages from the web UI.
    ///
    /// GET `/api/claude_code/sessions/{id}/poll`
    ///
    /// - `200` → JSON array of [`BridgeMessage`]; may be empty.
    /// - `204` → No messages; returns empty vec.
    /// - `401`/`403` → Auth failure; sets state to `Disconnected` and errors.
    async fn poll_messages(&self) -> anyhow::Result<Vec<BridgeMessage>> {
        let token = self
            .config
            .session_token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Poll: no token"))?;

        let url = format!(
            "{}/api/claude_code/sessions/{}/poll",
            self.config.server_url, self.session_id
        );

        let resp = self
            .http
            .get(&url)
            .bearer_auth(token)
            .timeout(std::time::Duration::from_secs(35))
            .send()
            .await
            .context("Bridge poll: HTTP send failed")?;

        let status = resp.status().as_u16();
        match status {
            200 => {
                let text = resp.text().await.context("Bridge poll: reading body")?;
                if text.trim().is_empty() || text.trim() == "[]" {
                    return Ok(vec![]);
                }
                let msgs: Vec<BridgeMessage> =
                    serde_json::from_str(&text).context("Bridge poll: JSON parse")?;
                Ok(msgs)
            }
            204 => Ok(vec![]),
            401 | 403 => {
                self.set_state(BridgeState::Error(format!("Auth error: {status}")));
                anyhow::bail!("Bridge poll: auth error ({})", status)
            }
            _ => {
                anyhow::bail!("Bridge poll: server returned {}", status)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Event upload
    // -----------------------------------------------------------------------

    /// Batch-upload outgoing events to the web UI.
    ///
    /// POST `/api/claude_code/sessions/{id}/events`
    async fn upload_events(&self, events: Vec<BridgeEvent>) -> anyhow::Result<()> {
        if events.is_empty() {
            return Ok(());
        }

        let token = self
            .config
            .session_token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Upload: no token"))?;

        let url = format!(
            "{}/api/claude_code/sessions/{}/events",
            self.config.server_url, self.session_id
        );

        let body = serde_json::json!({ "events": events });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .context("Bridge upload: HTTP send failed")?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            warn!(
                session_id = %self.session_id,
                status,
                count = events.len(),
                "Bridge event upload failed"
            );
            anyhow::bail!("Bridge upload: server returned {}", status);
        }

        debug!(
            session_id = %self.session_id,
            count = events.len(),
            "Bridge events uploaded"
        );
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Main poll loop
    // -----------------------------------------------------------------------

    /// Run the bridge poll loop until `cancel` is triggered or a fatal error
    /// occurs.
    ///
    /// On each iteration:
    /// 1. Drain any pending outgoing events and upload them in a batch.
    /// 2. Long-poll for incoming messages and forward them to `msg_tx`.
    /// 3. Back off exponentially on consecutive errors; give up after
    ///    `config.max_reconnect_attempts`.
    /// 4. Sleep `polling_interval_ms` between successful cycles.
    pub async fn run_poll_loop(
        mut self,
        msg_tx: mpsc::Sender<BridgeMessage>,
        mut event_rx: mpsc::Receiver<BridgeEvent>,
        cancel: CancellationToken,
    ) {
        info!(session_id = %self.session_id, "Bridge poll loop started");

        let base_interval =
            std::time::Duration::from_millis(self.config.polling_interval_ms.max(500));
        let max_backoff = std::time::Duration::from_secs(60);

        loop {
            // Respect cancellation at the top of every iteration.
            if cancel.is_cancelled() {
                info!(session_id = %self.session_id, "Bridge poll loop cancelled");
                break;
            }

            // --- Drain and upload pending events ---
            let mut events: Vec<BridgeEvent> = Vec::new();
            while let Ok(ev) = event_rx.try_recv() {
                events.push(ev);
            }
            if !events.is_empty() {
                if let Err(e) = self.upload_events(events).await {
                    warn!(session_id = %self.session_id, error = %e, "Event upload error");
                }
            }

            // --- Poll for incoming messages ---
            match self.poll_messages().await {
                Ok(messages) => {
                    // Successful poll — reset reconnect counter.
                    self.reconnect_count = 0;

                    for msg in messages {
                        if msg_tx.send(msg).await.is_err() {
                            debug!(
                                session_id = %self.session_id,
                                "Incoming message channel closed; stopping poll loop"
                            );
                            return;
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        session_id = %self.session_id,
                        error = %e,
                        reconnect_count = self.reconnect_count,
                        "Bridge poll error"
                    );

                    self.reconnect_count += 1;

                    if self.config.max_reconnect_attempts > 0
                        && self.reconnect_count >= self.config.max_reconnect_attempts
                    {
                        error!(
                            session_id = %self.session_id,
                            "Max bridge reconnect attempts ({}) reached; stopping",
                            self.config.max_reconnect_attempts
                        );
                        self.set_state(BridgeState::Error("max reconnects exceeded".into()));
                        break;
                    }

                    // Exponential backoff capped at `max_backoff`.
                    let backoff = (base_interval
                        * 2u32.pow(self.reconnect_count.saturating_sub(1).min(5)))
                    .min(max_backoff);

                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = cancel.cancelled() => {
                            info!(
                                session_id = %self.session_id,
                                "Bridge cancelled during backoff sleep"
                            );
                            break;
                        }
                    }
                    continue;
                }
            }

            // --- Wait for the next poll cycle ---
            tokio::select! {
                _ = tokio::time::sleep(base_interval) => {}
                _ = cancel.cancelled() => {
                    info!(
                        session_id = %self.session_id,
                        "Bridge cancelled during idle sleep"
                    );
                    break;
                }
            }
        }

        // Best-effort deregister on shutdown.
        self.deregister().await;
        info!(session_id = %self.session_id, "Bridge poll loop terminated");
    }
}

// ---------------------------------------------------------------------------
// Bridge manager
// ---------------------------------------------------------------------------

/// High-level manager wrapping configuration and a shared HTTP client.
///
/// Prefer [`start_bridge`] for the simple one-shot API.
pub struct BridgeManager {
    config: BridgeConfig,
    http: reqwest::Client,
}

impl BridgeManager {
    pub fn new(config: BridgeConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(format!("claude-code-rust/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .context("BridgeManager: failed to build HTTP client")?;
        Ok(Self { config, http })
    }

    /// Start the bridge polling loop, returning channel endpoints and the
    /// session ID.
    ///
    /// The background task runs until `cancel` is triggered.
    pub async fn start(
        &self,
        cancel: CancellationToken,
    ) -> anyhow::Result<(
        mpsc::Receiver<BridgeMessage>,
        mpsc::Sender<BridgeEvent>,
        String,
    )> {
        start_bridge_with_client(self.config.clone(), self.http.clone(), cancel).await
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start the bridge subsystem in a background task.
///
/// Registers a new session with the CCR server, then spawns a tokio task
/// running the poll loop. Returns:
/// - `msg_rx` — incoming messages from the web UI (e.g. user prompts).
/// - `event_tx` — sender for outgoing events (e.g. text deltas, tool calls).
/// - `session_id` — the UUID assigned to this session.
///
/// The background task runs until `cancel` is triggered or too many
/// consecutive errors occur. On shutdown the session is deregistered.
pub async fn start_bridge(
    config: BridgeConfig,
    cancel: CancellationToken,
) -> anyhow::Result<(
    mpsc::Receiver<BridgeMessage>,
    mpsc::Sender<BridgeEvent>,
    String,
)> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(format!("claude-code-rust/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("start_bridge: failed to build HTTP client")?;

    start_bridge_with_client(config, http, cancel).await
}

async fn start_bridge_with_client(
    config: BridgeConfig,
    _http: reqwest::Client,
    cancel: CancellationToken,
) -> anyhow::Result<(
    mpsc::Receiver<BridgeMessage>,
    mpsc::Sender<BridgeEvent>,
    String,
)> {
    if !config.is_active() {
        anyhow::bail!(
            "start_bridge: bridge is not active (enabled={}, token={})",
            config.enabled,
            config.session_token.is_some()
        );
    }

    let mut session = BridgeSession::new(config);
    session
        .register()
        .await
        .context("start_bridge: session registration failed")?;

    let session_id = session.session_id().to_string();

    // Bounded channels — back-pressure prevents unbounded memory growth on a
    // slow consumer.
    let (msg_tx, msg_rx) = mpsc::channel::<BridgeMessage>(64);
    let (event_tx, event_rx) = mpsc::channel::<BridgeEvent>(256);

    tokio::spawn(async move {
        session.run_poll_loop(msg_tx, event_rx, cancel).await;
    });

    info!(session_id = %session_id, "Bridge started");
    Ok((msg_rx, event_tx, session_id))
}

// ---------------------------------------------------------------------------
// High-level session API (start_bridge_session / poll / respond)
// ---------------------------------------------------------------------------

/// Information about a newly registered bridge session, returned by
/// [`start_bridge_session`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeSessionInfo {
    /// UUID assigned to this session.
    pub session_id: String,
    /// Full URL that can be shared with others to open the session in a browser.
    pub session_url: String,
    /// The auth token used for this session (redacted in Display).
    pub token: String,
}

impl std::fmt::Display for BridgeSessionInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BridgeSessionInfo {{ session_id: {}, session_url: {} }}",
            self.session_id, self.session_url
        )
    }
}

/// A message returned by [`poll_bridge_messages`]: an inbound item from the
/// remote peer identified by a string `id`, `role`, and `content`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleMessage {
    /// Server-assigned message identifier.
    pub id: String,
    /// Sender role (`"user"` or `"assistant"`).
    pub role: String,
    /// Message text content.
    pub content: String,
}

/// Start a bridge session: generate a session ID, register it with the
/// Anthropic API, and return session info including the shareable URL.
///
/// # Authentication
///
/// Reads the bearer token from (in order of precedence):
/// 1. `CLAURST_BRIDGE_TOKEN` environment variable
/// 2. `CLAUDE_BRIDGE_OAUTH_TOKEN` environment variable
///
/// If no token is found, returns an informative error.
///
/// # Errors
///
/// Returns an error if:
/// - No auth token is available
/// - The HTTP POST fails or the server returns a non-2xx status
/// - The server URL is not configured
///
/// # Example
///
/// ```rust,no_run
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// match claurst_bridge::start_bridge_session(None).await {
///     Ok(info) => println!("Session URL: {}", info.session_url),
///     Err(e) => eprintln!("Could not start bridge: {e}"),
/// }
/// # });
/// ```
pub async fn start_bridge_session(
    token_override: Option<String>,
) -> anyhow::Result<BridgeSessionInfo> {
    // Resolve auth token.
    let token = token_override
        .or_else(|| std::env::var("CLAURST_BRIDGE_TOKEN").ok())
        .or_else(|| std::env::var("CLAUDE_BRIDGE_OAUTH_TOKEN").ok())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Remote Control requires a session token.\n\
                 Set CLAURST_BRIDGE_TOKEN=<your-token> to enable.\n\
                 Get a token from https://claude.ai (Settings → Remote Control).\n\
                 Note: Remote Control is only available with claude.ai subscriptions."
            )
        })?;

    // Resolve server base URL.
    let server_url = std::env::var("CLAURST_BRIDGE_URL")
        .or_else(|_| std::env::var("CLAUDE_BRIDGE_BASE_URL"))
        .unwrap_or_else(|_| "https://claude.ai".to_string());

    let session_id = uuid::Uuid::new_v4().to_string();

    let hostname = {
        hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string())
    };

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(format!("claude-code-rust/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("start_bridge_session: failed to build HTTP client")?;

    let register_url = format!("{}/api/bridge/sessions", server_url);

    debug!(
        session_id = %session_id,
        url = %register_url,
        "Registering new bridge session"
    );

    let body = serde_json::json!({
        "session_id": session_id,
        "hostname": hostname,
        "client_version": env!("CARGO_PKG_VERSION"),
        "device_id": device_fingerprint(),
    });

    let resp = http
        .post(&register_url)
        .bearer_auth(&token)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "environments-2025-11-01")
        .json(&body)
        .send()
        .await
        .context("start_bridge_session: HTTP POST failed")?;

    let status = resp.status().as_u16();

    match status {
        200 | 201 => {
            info!(session_id = %session_id, "Bridge session registered successfully");
        }
        401 | 403 => {
            anyhow::bail!(
                "Bridge session registration failed: authentication error (HTTP {}).\n\
                 Your token may be invalid or expired.\n\
                 Get a new token from https://claude.ai (Settings → Remote Control).",
                status
            );
        }
        404 => {
            // The /api/bridge/sessions endpoint may not exist in all deployments.
            // Fall through to synthetic session URL (best-effort mode).
            warn!(
                session_id = %session_id,
                "Bridge registration endpoint not found (HTTP 404) — \
                 using local session ID without server validation"
            );
        }
        _ => {
            let body_text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Bridge session registration failed: server returned HTTP {}. {}",
                status,
                if body_text.is_empty() {
                    String::new()
                } else {
                    format!("Response: {}", &body_text[..body_text.len().min(200)])
                }
            );
        }
    }

    // Build the shareable session URL.
    let session_url = format!("{}/code/sessions/{}", server_url, session_id);

    Ok(BridgeSessionInfo {
        session_id,
        session_url,
        token,
    })
}

/// Poll for incoming messages on an active bridge session.
///
/// GETs `/api/bridge/sessions/<id>/messages?since=<last_msg_id>` and returns
/// the batch of new messages. Uses a 30-second HTTP timeout. On HTTP 429
/// (rate-limited) the function sleeps with exponential back-off before
/// retrying (up to 3 attempts).
///
/// Returns an empty `Vec` when there are no new messages (HTTP 204 or empty
/// body).
pub async fn poll_bridge_messages(
    info: &BridgeSessionInfo,
    since_id: Option<&str>,
) -> anyhow::Result<Vec<SimpleMessage>> {
    let server_url = std::env::var("CLAURST_BRIDGE_URL")
        .or_else(|_| std::env::var("CLAUDE_BRIDGE_BASE_URL"))
        .unwrap_or_else(|_| "https://claude.ai".to_string());

    // Validate session_id before interpolating into URL.
    BridgeConfig::validate_id(&info.session_id, "session_id")?;

    let base_url = format!(
        "{}/api/bridge/sessions/{}/messages",
        server_url, info.session_id
    );

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(35))
        .user_agent(format!("claude-code-rust/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("poll_bridge_messages: failed to build HTTP client")?;

    // Retry loop for 429 back-off.
    let max_retries = 3u32;
    let mut attempt = 0u32;
    loop {
        let mut request = http
            .get(&base_url)
            .bearer_auth(&info.token)
            .header("anthropic-version", "2023-06-01");

        if let Some(since) = since_id {
            request = request.query(&[("since", since)]);
        }

        let resp = request
            .send()
            .await
            .context("poll_bridge_messages: HTTP GET failed")?;

        let status = resp.status().as_u16();
        match status {
            200 => {
                let text = resp
                    .text()
                    .await
                    .context("poll_bridge_messages: reading body")?;
                if text.trim().is_empty() || text.trim() == "[]" {
                    return Ok(vec![]);
                }
                let msgs: Vec<SimpleMessage> =
                    serde_json::from_str(&text).context("poll_bridge_messages: JSON parse")?;
                return Ok(msgs);
            }
            204 => return Ok(vec![]),
            429 => {
                attempt += 1;
                if attempt > max_retries {
                    anyhow::bail!(
                        "poll_bridge_messages: rate-limited (HTTP 429) after {} retries",
                        max_retries
                    );
                }
                let backoff = std::time::Duration::from_millis(1_000 * 2u64.pow(attempt - 1));
                warn!(
                    attempt,
                    "Bridge poll rate-limited; backing off {:?}", backoff
                );
                tokio::time::sleep(backoff).await;
                continue;
            }
            401 | 403 => {
                anyhow::bail!("poll_bridge_messages: auth error (HTTP {})", status);
            }
            _ => {
                anyhow::bail!("poll_bridge_messages: server returned HTTP {}", status);
            }
        }
    }
}

/// Post a response to a specific incoming message on an active bridge session.
///
/// PUTs `/api/bridge/sessions/<session_id>/messages/<msg_id>/response` with
/// a JSON body `{"content": "<response>", "done": true}`.
pub async fn post_bridge_response(
    info: &BridgeSessionInfo,
    msg_id: &str,
    content: &str,
    done: bool,
) -> anyhow::Result<()> {
    let server_url = std::env::var("CLAURST_BRIDGE_URL")
        .or_else(|_| std::env::var("CLAUDE_BRIDGE_BASE_URL"))
        .unwrap_or_else(|_| "https://claude.ai".to_string());

    // Validate IDs before URL interpolation.
    BridgeConfig::validate_id(&info.session_id, "session_id")?;
    BridgeConfig::validate_id(msg_id, "msg_id")?;

    let url = format!(
        "{}/api/bridge/sessions/{}/messages/{}/response",
        server_url, info.session_id, msg_id
    );

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(format!("claude-code-rust/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("post_bridge_response: failed to build HTTP client")?;

    let body = serde_json::json!({
        "content": content,
        "done": done,
    });

    debug!(
        session_id = %info.session_id,
        msg_id = %msg_id,
        done = done,
        "Posting bridge response"
    );

    let resp = http
        .put(&url)
        .bearer_auth(&info.token)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .context("post_bridge_response: HTTP PUT failed")?;

    let status = resp.status().as_u16();
    if resp.status().is_success() {
        debug!(session_id = %info.session_id, msg_id = %msg_id, "Bridge response posted");
        Ok(())
    } else {
        anyhow::bail!(
            "post_bridge_response: server returned HTTP {} for msg {}",
            status,
            msg_id
        )
    }
}

/// Post a single streaming tool/text event to the bridge server (non-blocking,
/// best-effort).
///
/// POSTs `{"event": <payload>, "ts": <unix_ms>}` to
/// `/api/bridge/sessions/<session_id>/events`.
///
/// Errors are returned to the caller, who should treat them as transient and
/// ignore them so the query loop is never blocked.
pub async fn post_bridge_event(info: &BridgeSessionInfo, payload: String) -> anyhow::Result<()> {
    let server_url = std::env::var("CLAURST_BRIDGE_URL")
        .or_else(|_| std::env::var("CLAUDE_BRIDGE_BASE_URL"))
        .unwrap_or_else(|_| "https://claude.ai".to_string());

    // Validate session_id before URL interpolation.
    BridgeConfig::validate_id(&info.session_id, "session_id")?;

    let url = format!(
        "{}/api/bridge/sessions/{}/events",
        server_url, info.session_id
    );

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent(format!("claude-code-rust/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("post_bridge_event: failed to build HTTP client")?;

    let body = serde_json::json!({
        "event": payload,
        "ts": chrono::Utc::now().timestamp_millis(),
    });

    debug!(
        session_id = %info.session_id,
        "Posting bridge event"
    );

    let resp = http
        .post(&url)
        .bearer_auth(&info.token)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .context("post_bridge_event: HTTP POST failed")?;

    let status = resp.status().as_u16();
    if resp.status().is_success() {
        debug!(session_id = %info.session_id, "Bridge event posted");
        Ok(())
    } else {
        anyhow::bail!("post_bridge_event: server returned HTTP {}", status)
    }
}

// ---------------------------------------------------------------------------
// TUI-facing bridge event types (bridge → TUI state machine)
// ---------------------------------------------------------------------------

/// How the remote UI responded to a permission request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionResponseKind {
    Allow,
    Deny,
    AllowSession,
}

/// Internal events sent from the bridge loop to the TUI / main event loop.
///
/// These are *not* the same as [`BridgeEvent`] (which flows CLI → web UI).
/// `TuiBridgeEvent` flows from the bridge worker task into the main loop so
/// the TUI can update connection state, inject prompts, etc.
#[derive(Debug, Clone)]
pub enum TuiBridgeEvent {
    /// The bridge registered successfully and is now polling.
    Connected {
        session_url: String,
        session_id: String,
    },
    /// The connection was lost (cleanly or due to error).
    Disconnected { reason: Option<String> },
    /// Attempting to reconnect after a failure.
    Reconnecting { attempt: u32 },
    /// The web UI sent a new user prompt.
    InboundPrompt {
        content: String,
        sender_id: Option<String>,
    },
    /// The web UI asked to cancel the in-progress operation.
    Cancelled,
    /// The web UI responded to a pending permission request.
    PermissionResponse {
        tool_use_id: String,
        response: PermissionResponseKind,
    },
    /// The web UI requested a session title change.
    SessionNameUpdate { title: String },
    /// A non-fatal diagnostic from the bridge worker.
    Error(String),
    /// Keepalive ping — no TUI action required.
    Ping,
}

// ---------------------------------------------------------------------------
// Outbound event types (query loop → bridge → web UI)
// ---------------------------------------------------------------------------

/// Events from the query/tool loop forwarded outbound to the web UI via the
/// bridge upload channel. The bridge worker serialises these into
/// [`BridgeEvent`] values and POSTs them to the server.
#[derive(Debug, Clone)]
pub enum BridgeOutbound {
    TextDelta {
        delta: String,
        message_id: String,
    },
    ToolStart {
        id: String,
        name: String,
        input_preview: Option<String>,
    },
    ToolEnd {
        id: String,
        output: String,
        is_error: bool,
    },
    TurnComplete {
        message_id: String,
        stop_reason: String,
    },
    Error {
        message: String,
    },
    SessionMeta {
        title: Option<String>,
        session_id: String,
    },
}

// ---------------------------------------------------------------------------
// run_bridge_loop — high-level bridge task entry point
// ---------------------------------------------------------------------------

/// Run the bridge subsystem as a background task, translating low-level
/// [`BridgeMessage`] poll results into [`TuiBridgeEvent`] values and
/// forwarding [`BridgeOutbound`] events to the server.
///
/// # Parameters
/// - `config` — bridge configuration (must be active: `enabled == true` and
///   `session_token` is `Some`).
/// - `tui_tx` — channel used to send state-change events to the TUI / main
///   loop.
/// - `outbound_rx` — channel for receiving outbound events from the query
///   loop to upload to the bridge server.
/// - `cancel` — token that triggers a clean shutdown of the loop.
pub async fn run_bridge_loop(
    config: BridgeConfig,
    tui_tx: mpsc::Sender<TuiBridgeEvent>,
    mut outbound_rx: mpsc::Receiver<BridgeOutbound>,
    cancel: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    if !config.is_active() {
        anyhow::bail!(
            "run_bridge_loop: bridge is not active (enabled={}, token={})",
            config.enabled,
            config.session_token.is_some()
        );
    }

    // Build a BridgeSession and register with the server.
    let mut session = BridgeSession::new(config.clone());

    // Attempt initial registration; retry with back-off on transient errors.
    let base_backoff = std::time::Duration::from_millis(1_000);
    let max_backoff = std::time::Duration::from_secs(30);
    let mut reg_attempts = 0u32;

    loop {
        match session.register().await {
            Ok(()) => break,
            Err(e) => {
                reg_attempts += 1;
                warn!(
                    attempt = reg_attempts,
                    error = %e,
                    "Bridge registration failed"
                );

                // Auth errors are fatal — don't retry.
                let msg = e.to_string();
                if msg.contains("auth error") || msg.contains("401") || msg.contains("403") {
                    let _ = tui_tx
                        .send(TuiBridgeEvent::Error(format!("Bridge auth failed: {}", e)))
                        .await;
                    return Err(e);
                }

                if reg_attempts >= config.max_reconnect_attempts.max(1) {
                    let _ = tui_tx
                        .send(TuiBridgeEvent::Error(format!(
                            "Bridge registration failed after {} attempts: {}",
                            reg_attempts, e
                        )))
                        .await;
                    return Err(e);
                }

                let backoff = (base_backoff * 2u32.pow(reg_attempts.min(5))).min(max_backoff);
                let _ = tui_tx
                    .send(TuiBridgeEvent::Reconnecting {
                        attempt: reg_attempts,
                    })
                    .await;

                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = cancel.cancelled() => {
                        return Ok(());
                    }
                }
            }
        }
    }

    // Build the session URL from server_url + session_id.
    let session_url = format!(
        "{}/remote?session={}",
        config.server_url,
        session.session_id()
    );
    let session_id = session.session_id().to_string();

    let _ = tui_tx
        .send(TuiBridgeEvent::Connected {
            session_url: session_url.clone(),
            session_id: session_id.clone(),
        })
        .await;

    // Build outgoing BridgeEvent channel for the poll loop.
    let (bridge_ev_tx, bridge_ev_rx) = mpsc::channel::<BridgeEvent>(256);

    // Build incoming message channel.
    let (msg_tx, mut msg_rx) = mpsc::channel::<BridgeMessage>(64);

    // Spawn the low-level poll loop in its own task.
    let poll_cancel = cancel.clone();
    tokio::spawn(async move {
        session
            .run_poll_loop(msg_tx, bridge_ev_rx, poll_cancel)
            .await;
    });

    // Message ID counter for outbound text deltas.
    let mut msg_counter = 0u64;

    let poll_interval = std::time::Duration::from_millis(config.polling_interval_ms.max(50));

    loop {
        tokio::select! {
            // Handle cancellation.
            _ = cancel.cancelled() => {
                let _ = tui_tx.send(TuiBridgeEvent::Disconnected { reason: None }).await;
                break;
            }

            // Convert inbound BridgeMessage → TuiBridgeEvent.
            msg = msg_rx.recv() => {
                match msg {
                    None => {
                        // Poll loop shut down.
                        let _ = tui_tx
                            .send(TuiBridgeEvent::Disconnected {
                                reason: Some("Bridge poll loop terminated".to_string()),
                            })
                            .await;
                        break;
                    }
                    Some(BridgeMessage::UserMessage { content, .. }) => {
                        let _ = tui_tx
                            .send(TuiBridgeEvent::InboundPrompt {
                                content,
                                sender_id: None,
                            })
                            .await;
                    }
                    Some(BridgeMessage::PermissionResponse { tool_use_id, decision, .. }) => {
                        let kind = match decision {
                            PermissionDecision::Allow | PermissionDecision::AllowPermanently => {
                                PermissionResponseKind::Allow
                            }
                            PermissionDecision::Deny | PermissionDecision::DenyPermanently => {
                                PermissionResponseKind::Deny
                            }
                        };
                        let tuid = tool_use_id.unwrap_or_default();
                        if !tuid.is_empty() {
                            let _ = tui_tx
                                .send(TuiBridgeEvent::PermissionResponse {
                                    tool_use_id: tuid,
                                    response: kind,
                                })
                                .await;
                        }
                    }
                    Some(BridgeMessage::Cancel { .. }) => {
                        let _ = tui_tx.send(TuiBridgeEvent::Cancelled).await;
                    }
                    Some(BridgeMessage::Ping) => {
                        let _ = tui_tx.send(TuiBridgeEvent::Ping).await;
                        // Also respond with a Pong to the server.
                        let _ = bridge_ev_tx
                            .send(BridgeEvent::Pong {
                                server_time: Some(chrono::Utc::now().timestamp() as u64),
                            })
                            .await;
                    }
                }
            }

            // Forward outbound events from query loop → bridge server.
            outbound = outbound_rx.recv() => {
                match outbound {
                    None => {
                        // Sender dropped; nothing to forward.
                    }
                    Some(BridgeOutbound::TextDelta { delta, message_id }) => {
                        msg_counter += 1;
                        let _ = bridge_ev_tx
                            .send(BridgeEvent::TextDelta {
                                text: delta,
                                message_id,
                                index: Some(msg_counter as usize),
                            })
                            .await;
                    }
                    Some(BridgeOutbound::ToolStart { id, name, input_preview }) => {
                        let _ = bridge_ev_tx
                            .send(BridgeEvent::ToolStart {
                                tool_name: name,
                                tool_id: id,
                                input_preview,
                            })
                            .await;
                    }
                    Some(BridgeOutbound::ToolEnd { id, output, is_error }) => {
                        let _ = bridge_ev_tx
                            .send(BridgeEvent::ToolEnd {
                                tool_name: String::new(),
                                tool_id: id,
                                result: output,
                                is_error,
                            })
                            .await;
                    }
                    Some(BridgeOutbound::TurnComplete { message_id, stop_reason }) => {
                        let _ = bridge_ev_tx
                            .send(BridgeEvent::TurnComplete {
                                message_id,
                                stop_reason,
                                usage: None,
                            })
                            .await;
                    }
                    Some(BridgeOutbound::Error { message }) => {
                        let _ = bridge_ev_tx
                            .send(BridgeEvent::Error {
                                message,
                                code: None,
                            })
                            .await;
                    }
                    Some(BridgeOutbound::SessionMeta { title, session_id: sid }) => {
                        let _ = bridge_ev_tx
                            .send(BridgeEvent::SessionState {
                                session_id: sid,
                                state: BridgeSessionState::Connected,
                            })
                            .await;
                        if let Some(t) = title {
                            let _ = tui_tx
                                .send(TuiBridgeEvent::SessionNameUpdate { title: t })
                                .await;
                        }
                    }
                }
            }

            // Yield briefly to avoid busy-polling.
            _ = tokio::time::sleep(poll_interval) => {}
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Trusted device module (re-exported for external callers)
// ---------------------------------------------------------------------------

pub mod trusted_device {
    /// Re-export the crate-level device fingerprint function.
    pub use super::device_fingerprint;
}

// ---------------------------------------------------------------------------
// JWT module (re-exported for external callers)
// ---------------------------------------------------------------------------

pub mod jwt {
    pub use super::{decode_jwt_expiry, jwt_is_expired, JwtClaims};
}

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

// Allow downstream crates to use reqwest types without a direct dep.
pub use reqwest;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_fingerprint_is_non_empty() {
        let fp = device_fingerprint();
        assert!(!fp.is_empty(), "fingerprint should not be empty");
        // SHA-256 hex is always 64 chars
        assert_eq!(fp.len(), 64, "SHA-256 hex digest should be 64 chars");
    }

    #[test]
    fn test_device_fingerprint_is_stable() {
        let a = device_fingerprint();
        let b = device_fingerprint();
        assert_eq!(a, b, "fingerprint must be deterministic");
    }

    #[test]
    fn test_jwt_decode_invalid() {
        assert!(JwtClaims::decode("notajwt").is_err());
        assert!(JwtClaims::decode("only.two").is_ok() == false || true); // either way, must not panic
    }

    #[test]
    fn test_jwt_expired_unparseable() {
        // Unparseable token defaults to expired=true
        assert!(jwt_is_expired("bad.token.here"));
    }

    #[test]
    fn test_bridge_config_default_not_active() {
        let cfg = BridgeConfig::default();
        assert!(!cfg.is_active(), "default config must not be active");
    }

    #[test]
    fn test_bridge_config_with_token_still_needs_enabled() {
        let mut cfg = BridgeConfig::default();
        cfg.session_token = Some("tok".into());
        assert!(!cfg.is_active(), "needs enabled=true too");
        cfg.enabled = true;
        assert!(cfg.is_active());
    }

    #[test]
    fn test_validate_id_rejects_traversal() {
        assert!(BridgeConfig::validate_id("../../etc/passwd", "id").is_err());
        assert!(BridgeConfig::validate_id("abc123", "id").is_ok());
        assert!(BridgeConfig::validate_id("env_abc-123", "id").is_ok());
        assert!(BridgeConfig::validate_id("", "id").is_err());
    }

    #[test]
    fn test_permission_decision_serde() {
        let d = PermissionDecision::AllowPermanently;
        let s = serde_json::to_string(&d).unwrap();
        assert_eq!(s, r#""allow_permanently""#);
        let back: PermissionDecision = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn test_bridge_session_state_serde() {
        let s = BridgeSessionState::Processing;
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(j, r#""processing""#);
    }

    #[test]
    fn test_bridge_message_serde_user_message() {
        let msg = BridgeMessage::UserMessage {
            content: "hello".into(),
            session_id: "s1".into(),
            message_id: "m1".into(),
            attachments: vec![],
        };
        let j = serde_json::to_string(&msg).unwrap();
        assert!(j.contains(r#""type":"user_message""#));
    }

    #[test]
    fn test_bridge_event_text_delta_serde() {
        let ev = BridgeEvent::TextDelta {
            text: "hello world".into(),
            message_id: "m1".into(),
            index: Some(0),
        };
        let j = serde_json::to_string(&ev).unwrap();
        assert!(j.contains(r#""type":"text_delta""#));
        assert!(j.contains("hello world"));
    }

    #[test]
    fn test_bridge_event_pong_serde() {
        let ev = BridgeEvent::Pong {
            server_time: Some(1_700_000_000),
        };
        let j = serde_json::to_string(&ev).unwrap();
        assert!(j.contains(r#""type":"pong""#));
    }
}
