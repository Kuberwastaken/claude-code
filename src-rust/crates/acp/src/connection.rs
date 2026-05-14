//! Bidirectional newline-delimited JSON-RPC 2.0 connection over arbitrary
//! `AsyncRead` / `AsyncWrite` half-duplex pair (typically stdin/stdout).
//!
//! Both sides can send requests, responses, and notifications. Outbound
//! requests are tracked in a pending map so their responses can be routed
//! back to the awaiting caller.
//!
//! The wire format matches the Agent Client Protocol: each message is a
//! single UTF-8 line terminated by `\n` carrying a JSON-RPC 2.0 envelope.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use agent_client_protocol_schema as acp;
use dashmap::DashMap;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, trace, warn};

const JSONRPC_VERSION: &str = "2.0";

/// One parsed inbound JSON-RPC message — request, notification, or response.
///
/// Responses are NOT included here because the connection routes them
/// internally to the awaiting `send_request` future.
#[derive(Debug)]
pub enum Inbound {
    Request {
        id: acp::RequestId,
        method: String,
        params: Option<Value>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
}

/// Bidirectional connection over newline-delimited JSON-RPC.
///
/// Cloneable senders are obtained via `Connection::sender()`. The receive
/// loop is driven by `Connection::run_reader()` which yields parsed inbound
/// requests / notifications to the caller via the `mpsc::Receiver` returned
/// by `Connection::new`.
pub struct Connection {
    writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    pending: DashMap<String, oneshot::Sender<Result<Value, acp::Error>>>,
    next_outbound_id: AtomicI64,
}

impl Connection {
    /// Create a new connection over the given writer. The caller drives the
    /// reader by calling `run_reader` separately.
    pub fn new<W>(writer: W) -> Arc<Self>
    where
        W: AsyncWrite + Send + Unpin + 'static,
    {
        Arc::new(Self {
            writer: Mutex::new(Box::new(writer)),
            pending: DashMap::new(),
            next_outbound_id: AtomicI64::new(1),
        })
    }

    /// Send a successful JSON-RPC response to a previously received request.
    pub async fn send_response<R: Serialize>(
        self: &Arc<Self>,
        id: acp::RequestId,
        result: R,
    ) -> anyhow::Result<()> {
        let value = serde_json::to_value(result)?;
        let msg = serde_json::json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "result": value,
        });
        self.write_line(&msg).await
    }

    /// Send a JSON-RPC error response.
    pub async fn send_error_response(
        self: &Arc<Self>,
        id: acp::RequestId,
        error: acp::Error,
    ) -> anyhow::Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "error": error,
        });
        self.write_line(&msg).await
    }

    /// Send a notification (no id, no response expected).
    pub async fn send_notification<P: Serialize>(
        self: &Arc<Self>,
        method: &str,
        params: P,
    ) -> anyhow::Result<()> {
        let value = serde_json::to_value(params)?;
        let msg = serde_json::json!({
            "jsonrpc": JSONRPC_VERSION,
            "method": method,
            "params": value,
        });
        self.write_line(&msg).await
    }

    /// Send an outbound request and await the response.
    ///
    /// The caller is blocked until the matching response arrives via the
    /// reader loop, or the connection is closed (in which case `Err(...)`).
    pub async fn send_request<P: Serialize, R: DeserializeOwned>(
        self: &Arc<Self>,
        method: &str,
        params: P,
    ) -> anyhow::Result<Result<R, acp::Error>> {
        let raw_id = self.next_outbound_id.fetch_add(1, Ordering::Relaxed);
        let id_key = format!("n:{}", raw_id);
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id_key.clone(), tx);

        let params_value = serde_json::to_value(params)?;
        let msg = serde_json::json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": raw_id,
            "method": method,
            "params": params_value,
        });
        if let Err(e) = self.write_line(&msg).await {
            self.pending.remove(&id_key);
            return Err(e);
        }

        match rx.await {
            Ok(Ok(value)) => {
                let typed: R = serde_json::from_value(value)?;
                Ok(Ok(typed))
            }
            Ok(Err(err)) => Ok(Err(err)),
            Err(_) => Err(anyhow::anyhow!(
                "connection closed while awaiting response to '{}'",
                method
            )),
        }
    }

    async fn write_line(self: &Arc<Self>, value: &Value) -> anyhow::Result<()> {
        let mut buf = serde_json::to_vec(value)?;
        buf.push(b'\n');
        let mut w = self.writer.lock().await;
        w.write_all(&buf).await?;
        w.flush().await?;
        trace!(bytes = buf.len(), "ACP wire send");
        Ok(())
    }

    /// Look up and resolve a pending outbound request when a response arrives.
    fn complete_pending(&self, id: &acp::RequestId, payload: Result<Value, acp::Error>) {
        let key = id_to_key(id);
        if let Some((_, tx)) = self.pending.remove(&key) {
            let _ = tx.send(payload);
        } else {
            warn!(?id, "ACP: received response for unknown request id");
        }
    }
}

fn id_to_key(id: &acp::RequestId) -> String {
    match id {
        acp::RequestId::Null => "null".to_string(),
        acp::RequestId::Number(n) => format!("n:{}", n),
        acp::RequestId::Str(s) => format!("s:{}", s),
    }
}

/// Spawn the reader loop. Inbound requests and notifications are forwarded
/// to `tx`. Responses are matched against `connection.pending` internally.
/// The future completes when stdin reaches EOF.
pub async fn run_reader<R>(
    connection: Arc<Connection>,
    reader: R,
    tx: mpsc::UnboundedSender<Inbound>,
) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffered = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let n = buffered.read_line(&mut line).await?;
        if n == 0 {
            debug!("ACP: stdin EOF, shutting down reader");
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                // Spec says: if the message can't be parsed, send a parse-error
                // response with id=null. Best-effort here since we may not even
                // know if it was a request or notification.
                error!(error = %e, "ACP: parse error");
                let _ = connection
                    .send_error_response(acp::RequestId::Null, acp::Error::parse_error())
                    .await;
                continue;
            }
        };

        // Distinguish response (has id + (result|error)) from request (has id + method)
        // from notification (no id, has method).
        let has_id = v.get("id").is_some();
        let has_method = v.get("method").is_some();
        let has_result = v.get("result").is_some();
        let has_error = v.get("error").is_some();

        if has_id && (has_result || has_error) && !has_method {
            // Response — route to pending.
            let id: acp::RequestId = serde_json::from_value(v["id"].clone()).unwrap_or(acp::RequestId::Null);
            if has_result {
                let value = v["result"].clone();
                connection.complete_pending(&id, Ok(value));
            } else {
                let err: acp::Error = match serde_json::from_value(v["error"].clone()) {
                    Ok(e) => e,
                    Err(_) => acp::Error::internal_error().data(Some(v["error"].clone())),
                };
                connection.complete_pending(&id, Err(err));
            }
        } else if has_id && has_method {
            // Request.
            let id: acp::RequestId = serde_json::from_value(v["id"].clone()).unwrap_or(acp::RequestId::Null);
            let method = v
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let params = v.get("params").cloned();
            if tx
                .send(Inbound::Request { id, method, params })
                .is_err()
            {
                break;
            }
        } else if has_method {
            // Notification.
            let method = v
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let params = v.get("params").cloned();
            if tx.send(Inbound::Notification { method, params }).is_err() {
                break;
            }
        } else {
            warn!(line = %trimmed, "ACP: unrecognized JSON-RPC message shape");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncReadExt};

    /// `send_response` must emit a newline-delimited JSON object containing
    /// `jsonrpc: "2.0"`, the request id, and the result payload.
    #[tokio::test]
    async fn send_response_writes_newline_delimited_jsonrpc() {
        let (client_read, server_write) = duplex(8192);
        let connection = Connection::new(server_write);

        connection
            .send_response(acp::RequestId::Number(7), serde_json::json!({"ok": true}))
            .await
            .unwrap();
        drop(connection);

        let mut buf = Vec::new();
        let mut client_read = client_read;
        client_read.read_to_end(&mut buf).await.unwrap();
        let line = std::str::from_utf8(&buf).unwrap();
        assert!(line.ends_with('\n'), "response missing newline: {line:?}");
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 7);
        assert_eq!(parsed["result"]["ok"], true);
    }

    /// `send_notification` must omit `id` so the receiver knows not to reply.
    #[tokio::test]
    async fn notifications_have_no_id() {
        let (client_read, server_write) = duplex(8192);
        let connection = Connection::new(server_write);

        connection
            .send_notification("session/update", serde_json::json!({"sessionId": "s1"}))
            .await
            .unwrap();
        drop(connection);

        let mut buf = Vec::new();
        let mut client_read = client_read;
        client_read.read_to_end(&mut buf).await.unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(std::str::from_utf8(&buf).unwrap().trim()).unwrap();
        assert!(parsed.get("id").is_none(), "notification must not carry id");
        assert_eq!(parsed["method"], "session/update");
    }

    /// The reader must distinguish requests, notifications, and responses by
    /// shape and route each to the right callback path.
    #[tokio::test]
    async fn reader_routes_inbound_messages() {
        // Drive the reader from an in-memory pipe.
        let (writer_handle, server_reader) = duplex(8192);
        let (_to_client, client_writer) = duplex(8192);
        let connection = Connection::new(client_writer);
        let (tx, mut rx) = mpsc::unbounded_channel();

        let reader_handle = tokio::spawn(run_reader(connection.clone(), server_reader, tx));

        // Inject a request, a notification, and an unsolicited response.
        let mut writer_handle = writer_handle;
        writer_handle
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n",
            )
            .await
            .unwrap();
        writer_handle
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"method\":\"session/cancel\",\"params\":{\"sessionId\":\"s1\"}}\n",
            )
            .await
            .unwrap();
        writer_handle
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{\"orphan\":true}}\n",
            )
            .await
            .unwrap();
        drop(writer_handle); // EOF the reader

        let first = rx.recv().await.expect("expected request");
        match first {
            Inbound::Request { id, method, .. } => {
                assert_eq!(method, "initialize");
                assert!(matches!(id, acp::RequestId::Number(1)));
            }
            other => panic!("expected Request, got {other:?}"),
        }
        let second = rx.recv().await.expect("expected notification");
        match second {
            Inbound::Notification { method, .. } => {
                assert_eq!(method, "session/cancel");
            }
            other => panic!("expected Notification, got {other:?}"),
        }
        // Orphan response is routed internally; nothing surfaces here.
        assert!(rx.recv().await.is_none(), "no further messages expected");
        let _ = reader_handle.await;
    }

    /// `send_request` resolves when a response with the matching id arrives.
    #[tokio::test]
    async fn send_request_resolves_on_matching_response() {
        let (client_to_server, server_reader) = duplex(8192);
        let (server_to_client_reader, server_to_client) = duplex(8192);
        let connection = Connection::new(server_to_client);
        let (tx, _rx) = mpsc::unbounded_channel();
        let reader_handle =
            tokio::spawn(run_reader(connection.clone(), server_reader, tx));

        // Background: as a fake client, read the outbound request and write a
        // matching response.
        let client_loop = tokio::spawn(async move {
            let mut buf = Vec::new();
            let mut server_to_client_reader = server_to_client_reader;
            let mut byte = [0u8; 1];
            // Read up to one full line.
            loop {
                let n = server_to_client_reader.read(&mut byte).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            let outbound: serde_json::Value =
                serde_json::from_slice(buf.trim_ascii_end()).unwrap();
            let id = outbound["id"].clone();
            // Send the response back through the client_to_server pipe.
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"echo": outbound["method"].as_str().unwrap()}
            });
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            let mut client_to_server = client_to_server;
            client_to_server.write_all(&bytes).await.unwrap();
            drop(client_to_server);
        });

        let response: Result<serde_json::Value, acp::Error> = connection
            .send_request("fs/read_text_file", serde_json::json!({"path": "/x"}))
            .await
            .unwrap();
        let response = response.unwrap();
        assert_eq!(response["echo"], "fs/read_text_file");
        let _ = client_loop.await;
        let _ = reader_handle.await;
    }
}

