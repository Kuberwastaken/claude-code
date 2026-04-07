//! Remote session sync — mirrors src/remote/RemoteSessionManager.ts
//! and src/remote/SessionsWebSocket.ts.
//!
//! Manages background synchronization of local session transcripts
//! with the Claude.ai cloud API.

use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A cloud session summary (from the list API).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudSession {
    pub id: String,
    pub title: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub project: Option<String>,
    pub message_count: u64,
}

/// Events emitted by the remote session WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    SessionCreated(CloudSession),
    SessionUpdated(CloudSession),
    SessionDeleted { id: String },
}

// ---------------------------------------------------------------------------
// Remote Session Manager
// ---------------------------------------------------------------------------

/// Manages remote session listing and background sync.
pub struct RemoteSessionManager {
    base_url: String,
    access_token: String,
    /// Channel to emit SessionEvents to the TUI.
    _event_tx: mpsc::Sender<SessionEvent>,
}

impl RemoteSessionManager {
    pub fn new(access_token: String) -> (Self, mpsc::Receiver<SessionEvent>) {
        let (tx, rx) = mpsc::channel(64);
        (
            Self {
                base_url: "https://api.claude.ai".to_string(),
                access_token,
                _event_tx: tx,
            },
            rx,
        )
    }

    /// Fetch the list of remote sessions for the current user.
    pub async fn list_sessions(&self) -> Result<Vec<CloudSession>, String> {
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/api/sessions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }

        resp.json::<Vec<CloudSession>>()
            .await
            .map_err(|e| format!("Parse error: {e}"))
    }

    /// Push a transcript entry to the cloud for `session_id`.
    pub async fn push_transcript_entry(
        &self,
        session_id: &str,
        entry_json: &str,
    ) -> Result<(), String> {
        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "{}/api/sessions/{}/messages",
                self.base_url, session_id
            ))
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .body(entry_json.to_string())
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("API error: {}", resp.status()));
        }
        Ok(())
    }

    /// Start background sync loop: pushes local transcript to cloud every 30s.
    /// Returns a JoinHandle; caller should keep it alive.
    pub fn start_background_sync(
        self: std::sync::Arc<Self>,
        session_id: String,
        transcript_path: std::path::PathBuf,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            let mut last_sync_len = 0usize;

            loop {
                interval.tick().await;
                // Read transcript file and find new entries since last sync
                if let Ok(content) = tokio::fs::read_to_string(&transcript_path).await {
                    let lines: Vec<&str> = content.lines().collect();
                    if lines.len() > last_sync_len {
                        for line in &lines[last_sync_len..] {
                            if !line.is_empty() {
                                let _ = self.push_transcript_entry(&session_id, line).await;
                            }
                        }
                        last_sync_len = lines.len();
                    }
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Sessions WebSocket
// ---------------------------------------------------------------------------

/// WebSocket client for real-time session events.
pub struct SessionsWebSocket {
    pub ws_url: String,
    pub access_token: String,
}

impl SessionsWebSocket {
    pub fn new(access_token: String) -> Self {
        Self {
            ws_url: "wss://api.claude.ai/ws/sessions".to_string(),
            access_token,
        }
    }

    /// Connect to the sessions WebSocket, emit events, and reconnect on disconnect.
    /// Runs until the sender is dropped or the task is cancelled.
    pub async fn connect(&self, event_tx: mpsc::Sender<SessionEvent>) -> Result<(), String> {
        let mut backoff_secs: u64 = 1;
        loop {
            match self.run_once(&event_tx).await {
                Ok(()) => {
                    // Server closed cleanly — reconnect.
                    tracing::debug!("SessionsWebSocket: server closed connection, reconnecting");
                }
                Err(e) => {
                    tracing::warn!(error = %e, backoff = backoff_secs, "SessionsWebSocket: error, backing off");
                }
            }
            if event_tx.is_closed() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(60);
        }
        Ok(())
    }

    async fn run_once(&self, event_tx: &mpsc::Sender<SessionEvent>) -> Result<(), String> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
        let url = format!(
            "{}?access_token={}",
            self.ws_url,
            urlencoding::encode(&self.access_token)
        );
        let request = url
            .as_str()
            .into_client_request()
            .map_err(|e| e.to_string())?;

        let (ws_stream, _) = connect_async(request).await.map_err(|e| e.to_string())?;
        tracing::info!(url = %self.ws_url, "SessionsWebSocket: connected");

        let mut read = ws_stream;
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    match serde_json::from_str::<SessionEvent>(&text) {
                        Ok(ev) => {
                            if event_tx.send(ev).await.is_err() {
                                return Ok(()); // receiver dropped
                            }
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, raw = %text, "SessionsWebSocket: unrecognised event");
                        }
                    }
                }
                Ok(Message::Close(_)) => return Ok(()),
                Ok(_) => {} // ping/pong/binary — ignore
                Err(e) => return Err::<(), String>(e.to_string()),
            }
        }
        Ok(())
    }
}
