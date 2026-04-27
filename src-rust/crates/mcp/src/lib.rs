// cc-mcp: Model Context Protocol (MCP) client implementation.
//
// MCP is a JSON-RPC 2.0 based protocol for connecting Claude to external
// tool/resource servers. This crate implements:
//
// - JSON-RPC 2.0 client primitives
// - MCP protocol handshake (initialize, initialized)
// - Tool discovery (tools/list)
// - Tool execution (tools/call)
// - Resource management (resources/list, resources/read)
// - Prompt templates (prompts/list, prompts/get)
// - Transport: stdio (subprocess) and HTTP/SSE
// - Environment variable expansion in server configs
// - Connection manager with exponential-backoff reconnection

use async_trait::async_trait;
use claurst_core::config::McpServerConfig;
use claurst_core::mcp_templates::TemplateRenderer;
use claurst_core::types::ToolDefinition;
use dashmap::DashMap;
use futures::stream::{BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

pub use client::McpClient;
pub use types::*;
pub use connection_manager::{McpConnectionManager, McpServerStatus};

pub mod backend;
pub mod connection_manager;
pub mod registry;
pub mod oauth;
pub mod rmcp_backend;

// ---------------------------------------------------------------------------
// Environment variable expansion
// ---------------------------------------------------------------------------

/// Expand `${VAR_NAME}` and `${VAR_NAME:-default}` patterns in `input` using
/// the process environment.  Unknown variables without a default are left as-is
/// (matching the TS behaviour: report missing but don't crash).
pub fn expand_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    // We iterate from left to right, always restarting the search after each
    // substitution so that replaced values are not re-scanned.
    let mut search_from = 0;
    loop {
        match result[search_from..].find("${") {
            None => break,
            Some(rel_start) => {
                let start = search_from + rel_start;
                match result[start..].find('}') {
                    None => break, // unclosed brace — stop
                    Some(rel_end) => {
                        let end = start + rel_end; // index of '}'
                        let inner = &result[start + 2..end]; // content between ${ and }

                        // Support ${VAR:-default} syntax
                        let (var_name, default_value) = if let Some(pos) = inner.find(":-") {
                            (&inner[..pos], Some(&inner[pos + 2..]))
                        } else {
                            (inner, None)
                        };

                        let replacement = match std::env::var(var_name) {
                            Ok(val) => val,
                            Err(_) => match default_value {
                                Some(def) => def.to_string(),
                                None => {
                                    // Leave the original text in place; advance past it
                                    search_from = end + 1;
                                    continue;
                                }
                            },
                        };

                        result = format!("{}{}{}", &result[..start], replacement, &result[end + 1..]);
                        // Continue scanning from where the replacement ends
                        search_from = start + replacement.len();
                    }
                }
            }
        }
    }
    result
}

/// Expand env vars in every string field of a `McpServerConfig`.
/// Returns a new owned config; the original is not modified.
pub fn expand_server_config(config: &McpServerConfig) -> McpServerConfig {
    McpServerConfig {
        name: config.name.clone(),
        command: config.command.as_deref().map(expand_env_vars),
        args: config.args.iter().map(|a| expand_env_vars(a)).collect(),
        env: config
            .env
            .iter()
            .map(|(k, v)| (k.clone(), expand_env_vars(v)))
            .collect(),
        url: config.url.as_deref().map(expand_env_vars),
        server_type: config.server_type.clone(),
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 Types
// ---------------------------------------------------------------------------

pub mod types {
    use super::*;

    /// A JSON-RPC 2.0 request.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct JsonRpcRequest {
        pub jsonrpc: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub id: Option<Value>,
        pub method: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub params: Option<Value>,
    }

    impl JsonRpcRequest {
        pub fn new(id: impl Into<Value>, method: impl Into<String>, params: Option<Value>) -> Self {
            Self {
                jsonrpc: "2.0".to_string(),
                id: Some(id.into()),
                method: method.into(),
                params,
            }
        }

        pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
            Self {
                jsonrpc: "2.0".to_string(),
                id: None,
                method: method.into(),
                params,
            }
        }
    }


    /// A JSON-RPC 2.0 response.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct JsonRpcResponse {
        pub jsonrpc: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub id: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub error: Option<JsonRpcError>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct JsonRpcError {
        pub code: i64,
        pub message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub data: Option<Value>,
    }

    // ---- MCP protocol types ------------------------------------------------

    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    pub struct ServerCapabilities {
        #[serde(skip_serializing_if = "Option::is_none")]
        pub tools: Option<ToolsCapability>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub resources: Option<ResourcesCapability>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub prompts: Option<PromptsCapability>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub logging: Option<Value>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ToolsCapability {
        #[serde(default)]
        pub list_changed: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ResourcesCapability {
        #[serde(default)]
        pub subscribe: bool,
        #[serde(default)]
        pub list_changed: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PromptsCapability {
        #[serde(default)]
        pub list_changed: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ServerInfo {
        pub name: String,
        pub version: String,
    }

    /// An MCP tool definition.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct McpTool {
        pub name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        pub input_schema: Value,
    }

    impl From<&McpTool> for ToolDefinition {
        fn from(t: &McpTool) -> Self {
            ToolDefinition {
                name: t.name.clone(),
                description: t.description.clone().unwrap_or_default(),
                input_schema: t.input_schema.clone(),
            }
        }
    }

    /// tools/list response.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ListToolsResult {
        pub tools: Vec<McpTool>,
        #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
        pub next_cursor: Option<String>,
    }

    /// tools/call params.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct CallToolParams {
        pub name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub arguments: Option<Value>,
    }

    /// tools/call response.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CallToolResult {
        pub content: Vec<McpContent>,
        #[serde(default)]
        pub is_error: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "lowercase")]
    pub enum McpContent {
        Text { text: String },
        Image {
            data: String,
            #[serde(rename = "mimeType")]
            mime_type: String,
        },
        Resource { resource: ResourceContents },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ResourceContents {
        pub uri: String,
        #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
        pub mime_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub blob: Option<String>,
    }

    /// An MCP resource.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct McpResource {
        pub uri: String,
        pub name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub mime_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub annotations: Option<Value>,
    }

    /// resources/list response.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ListResourcesResult {
        pub resources: Vec<McpResource>,
        #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
        pub next_cursor: Option<String>,
    }

    /// An MCP prompt template.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct McpPrompt {
        pub name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        #[serde(default)]
        pub arguments: Vec<McpPromptArgument>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct McpPromptArgument {
        pub name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        #[serde(default)]
        pub required: bool,
    }

    /// prompts/list response.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ListPromptsResult {
        pub prompts: Vec<McpPrompt>,
    }

    /// A single message returned by prompts/get.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptMessage {
        /// "user" or "assistant"
        pub role: String,
        pub content: PromptMessageContent,
    }

    /// Content inside a PromptMessage.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "lowercase")]
    pub enum PromptMessageContent {
        Text { text: String },
        Image { data: String, mime_type: String },
        Resource { resource: serde_json::Value },
    }

    /// prompts/get response.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct GetPromptResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        pub messages: Vec<PromptMessage>,
    }
}

// ---------------------------------------------------------------------------
// Transport layer
// ---------------------------------------------------------------------------

pub mod transport {
    use super::*;
    use reqwest::header::{CONTENT_TYPE, HeaderValue};
    #[cfg(test)]
    use tokio::sync::mpsc;

    pub const LEGACY_PROTOCOL_VERSION: &str = "2024-11-05";
    pub const STREAMABLE_HTTP_PROTOCOL_VERSION: &str = "2025-11-25";
    pub const STREAMABLE_HTTP_PROTOCOL_VERSIONS: &[&str] = &[
        STREAMABLE_HTTP_PROTOCOL_VERSION,
        "2025-06-18",
        "2025-03-26",
        LEGACY_PROTOCOL_VERSION,
    ];

    /// A transport can send requests and receive responses.
    #[async_trait]
    pub trait McpTransport: Send + Sync {
        async fn send(&self, message: &JsonRpcRequest) -> anyhow::Result<()>;
        async fn recv(&self) -> anyhow::Result<Option<JsonRpcResponse>>;
        async fn close(&self) -> anyhow::Result<()>;
        /// Non-blocking poll: return the next raw JSON message if one is
        /// immediately available, or `Ok(None)` if the queue is empty.
        /// Used by the notification dispatch loop to drain server-initiated
        /// notifications without blocking an async task.
        async fn try_receive_raw(&self) -> anyhow::Result<Option<serde_json::Value>>;
        /// Subscribe to raw JSON notifications from the transport.
        /// Returns an async stream of notification messages.
        ///
        /// For transports that natively support push notifications (e.g., WebSocket),
        /// this returns a stream that yields messages directly from the transport.
        /// For transports without native push support (e.g., stdio), this returns
        /// a stream that polls periodically.
        fn subscribe_to_notifications(
            &self,
        ) -> BoxStream<'static, anyhow::Result<serde_json::Value>>;

        fn protocol_version(&self) -> &'static str {
            LEGACY_PROTOCOL_VERSION
        }
    }

    pub(crate) fn bearer_header_value(token: &str) -> anyhow::Result<HeaderValue> {
        HeaderValue::from_str(&format!("Bearer {}", token))
            .map_err(|e| anyhow::anyhow!("invalid bearer token header: {}", e))
    }

    pub(crate) fn is_event_stream_response(response: &reqwest::Response) -> bool {
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.contains("text/event-stream"))
            .unwrap_or(false)
    }

    pub(crate) fn resolve_legacy_endpoint(base_url: &str, endpoint: &str) -> anyhow::Result<String> {
        let endpoint = endpoint.trim();
        if endpoint.is_empty() {
            anyhow::bail!("legacy SSE endpoint event did not include a POST endpoint");
        }
        if let Ok(url) = url::Url::parse(endpoint) {
            return Ok(url.to_string());
        }
        let base = url::Url::parse(base_url)
            .map_err(|e| anyhow::anyhow!("invalid legacy SSE base URL '{}': {}", base_url, e))?;
        base.join(endpoint)
            .map(|url| url.to_string())
            .map_err(|e| anyhow::anyhow!("failed to resolve legacy SSE endpoint '{}': {}", endpoint, e))
    }

    #[cfg(test)]
    pub(super) fn route_incoming_value(
        server_name: &str,
        value: serde_json::Value,
        response_tx: &mpsc::UnboundedSender<JsonRpcResponse>,
        notification_tx: &mpsc::UnboundedSender<serde_json::Value>,
    ) -> anyhow::Result<()> {
        let is_response = value.get("id").map(|id| !id.is_null()).unwrap_or(false)
            || value.get("result").is_some()
            || value.get("error").is_some();

        if is_response {
            let response: JsonRpcResponse = serde_json::from_value(value).map_err(|e| {
                anyhow::anyhow!(
                    "MCP server '{}': failed to parse JSON-RPC response from HTTP transport: {}",
                    server_name,
                    e
                )
            })?;
            let _ = response_tx.send(response);
            return Ok(());
        }

        if value.get("method").is_some() {
            let _ = notification_tx.send(value);
        }

        Ok(())
    }

    fn dispatch_sse_event<F>(
        event_name: &mut Option<String>,
        data_lines: &mut Vec<String>,
        on_event: &mut F,
    ) -> anyhow::Result<()>
    where
        F: FnMut(Option<&str>, &str) -> anyhow::Result<()>,
    {
        if event_name.is_none() && data_lines.is_empty() {
            return Ok(());
        }
        let data = data_lines.join("\n");
        on_event(event_name.as_deref(), &data)?;
        *event_name = None;
        data_lines.clear();
        Ok(())
    }

    pub(super) fn process_sse_line(
        line: &str,
        event_name: &mut Option<String>,
        data_lines: &mut Vec<String>,
    ) {
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => *event_name = Some(value.to_string()),
            "data" => data_lines.push(value.to_string()),
            _ => {}
        }
    }

    pub(crate) async fn process_sse_response<F>(
        response: reqwest::Response,
        mut on_event: F,
    ) -> anyhow::Result<()>
    where
        F: FnMut(Option<&str>, &str) -> anyhow::Result<()>,
    {
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut event_name: Option<String> = None;
        let mut data_lines: Vec<String> = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("failed reading SSE stream: {}", e))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find('\n') {
                let mut line: String = buffer.drain(..=pos).collect();
                if line.ends_with('\n') {
                    line.pop();
                }
                if line.ends_with('\r') {
                    line.pop();
                }
                if line.is_empty() {
                    dispatch_sse_event(&mut event_name, &mut data_lines, &mut on_event)?;
                } else {
                    process_sse_line(&line, &mut event_name, &mut data_lines);
                }
            }
        }

        if !buffer.is_empty() {
            if buffer.ends_with('\r') {
                buffer.pop();
            }
            if buffer.is_empty() {
                dispatch_sse_event(&mut event_name, &mut data_lines, &mut on_event)?;
            } else {
                process_sse_line(&buffer, &mut event_name, &mut data_lines);
            }
        }

        dispatch_sse_event(&mut event_name, &mut data_lines, &mut on_event)?;
        Ok(())
    }

}

// ---------------------------------------------------------------------------
// MCP Client
// ---------------------------------------------------------------------------

pub mod client {
    use super::*;

    /// A fully initialized MCP client connected to a single server.
    pub struct McpClient {
        pub server_name: String,
        pub server_info: Option<ServerInfo>,
        pub capabilities: ServerCapabilities,
        pub tools: Vec<McpTool>,
        pub resources: Vec<McpResource>,
        pub prompts: Vec<McpPrompt>,
        pub instructions: Option<String>,
        backend: Option<Arc<dyn backend::McpClientBackend>>,
    }

    impl McpClient {
        fn from_snapshot(snapshot: backend::McpClientSnapshot) -> Self {
            Self {
                server_name: snapshot.server_name,
                server_info: snapshot.server_info,
                capabilities: snapshot.capabilities,
                tools: snapshot.tools,
                resources: snapshot.resources,
                prompts: snapshot.prompts,
                instructions: snapshot.instructions,
                backend: None,
            }
        }

        fn from_backend(backend: Arc<dyn backend::McpClientBackend>) -> Self {
            let snapshot = backend.snapshot();
            let mut client = Self::from_snapshot(snapshot);
            client.backend = Some(backend);
            client
        }

        fn backend(&self) -> anyhow::Result<&Arc<dyn backend::McpClientBackend>> {
            self.backend
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("MCP client backend missing"))
        }

        pub async fn connect(config: &McpServerConfig, auth_token: Option<String>) -> anyhow::Result<Self> {
            match config.server_type.as_str() {
                "stdio" => Self::connect_stdio(config).await,
                "sse" => {
                    let backend = crate::rmcp_backend::RmcpClientBackend::connect_legacy_sse(
                        config,
                        auth_token,
                    )
                    .await?;
                    Ok(Self::from_backend(Arc::new(backend)))
                }
                "http" => {
                    let mut last_error = None;
                    for &protocol_version in transport::STREAMABLE_HTTP_PROTOCOL_VERSIONS {
                        let protocol_version = serde_json::from_value::<rmcp::model::ProtocolVersion>(
                            Value::String(protocol_version.to_string()),
                        )
                        .map_err(|e| {
                            anyhow::anyhow!(
                                "MCP server '{}': invalid streamable HTTP protocol version '{}': {}",
                                config.name,
                                protocol_version,
                                e
                            )
                        })?;
                        let protocol_version_label = protocol_version.as_str().to_string();
                        match crate::rmcp_backend::RmcpClientBackend::connect_http(
                            config,
                            auth_token.clone(),
                            protocol_version,
                        )
                        .await
                        {
                            Ok(backend) => return Ok(Self::from_backend(Arc::new(backend))),
                            Err(e) => {
                                let message = e.to_string();
                                if Self::is_unsupported_protocol_error(&message) {
                                    debug!(
                                        server = %config.name,
                                        protocol_version = %protocol_version_label,
                                        error = %message,
                                        "Streamable HTTP protocol version unsupported; trying fallback"
                                    );
                                    last_error = Some(e);
                                    continue;
                                }
                                return Err(e);
                            }
                        }
                    }
                    Err(last_error.unwrap_or_else(|| {
                        anyhow::anyhow!(
                            "MCP server '{}': no supported streamable HTTP protocol version found",
                            config.name
                        )
                    }))
                }
                other => Err(anyhow::anyhow!(
                    "MCP server '{}': unsupported transport type '{}'",
                    config.name,
                    other
                )),
            }
        }

        pub(crate) fn is_unsupported_protocol_error(message: &str) -> bool {
            // rmcp, servers, and gateways do not emit a single stable
            // protocol-negotiation error string, so match the known variants
            // conservatively to keep downgrade fallback working.
            let lower = message.to_ascii_lowercase();
            lower.contains("unsupported protocol version")
                || lower.contains("unsupported mcp-protocol-version")
                || (lower.contains("protocol version") && lower.contains("unsupported"))
                || (lower.contains("mcp-protocol-version") && lower.contains("bad request"))
        }

        /// Connect to an MCP server using stdio transport. The `config` should
        /// already have env vars expanded via `expand_server_config`.
        pub async fn connect_stdio(config: &McpServerConfig) -> anyhow::Result<Self> {
            let backend = crate::rmcp_backend::RmcpClientBackend::connect_stdio(config).await?;
            Ok(Self::from_backend(Arc::new(backend)))
        }

        // ---- High-level API -----------------------------------------------

        pub async fn list_tools(&self) -> anyhow::Result<Vec<McpTool>> {
            self.backend()?.list_tools().await
        }

        pub async fn call_tool(
            &self,
            name: &str,
            arguments: Option<Value>,
        ) -> anyhow::Result<CallToolResult> {
            self.backend()?.call_tool(name, arguments).await.map_err(|e| {
                anyhow::anyhow!(
                    "MCP server '{}': tool '{}' call failed: {}",
                    self.server_name,
                    name,
                    e
                )
            })
        }

        pub async fn list_resources(&self) -> anyhow::Result<Vec<McpResource>> {
            let mut resources = self.backend()?.list_resources().await?;
            apply_resource_templates(&mut resources);
            Ok(resources)
        }

        pub async fn read_resource(&self, uri: &str) -> anyhow::Result<ResourceContents> {
            self.backend()?.read_resource(uri).await
        }

        pub async fn list_prompts(&self) -> anyhow::Result<Vec<McpPrompt>> {
            self.backend()?.list_prompts().await
        }

        /// Invoke `prompts/get` with the given name and optional arguments map.
        ///
        /// Returns the expanded prompt messages that should be injected into the
        /// conversation as-is (mirrors TS `getMCPPrompt`).
        pub async fn get_prompt(
            &self,
            name: &str,
            arguments: Option<std::collections::HashMap<String, String>>,
        ) -> anyhow::Result<GetPromptResult> {
            self.backend()?.get_prompt(name, arguments).await
        }

        pub async fn subscribe_resource(&self, uri: &str) -> anyhow::Result<()> {
            self.backend()?.subscribe_resource(uri).await
        }

        pub async fn unsubscribe_resource(&self, uri: &str) -> anyhow::Result<()> {
            self.backend()?.unsubscribe_resource(uri).await
        }

        /// Get all tools as `ToolDefinition` objects suitable for the API.
        pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
            self.tools.iter().map(|t| t.into()).collect()
        }

        /// Subscribe to raw JSON notifications from the active backend.
        pub fn subscribe_to_notifications(&self) -> BoxStream<'static, anyhow::Result<serde_json::Value>> {
            self.backend()
                .expect("MCP client backend missing")
                .subscribe_to_notifications()
        }

        // ---- Notification dispatch ----------------------------------------

        /// Process a single notification message from the transport stream.
        /// Routes resource updates to subscribers and logs other notifications.
        pub(crate) async fn process_notification(
            &self,
            raw: serde_json::Value,
            resource_subscriptions: &dashmap::DashMap<
                (String, String),
                tokio::sync::mpsc::Sender<ResourceChangedEvent>,
            >,
        ) {
            // Only process server-initiated notifications (have "method", no non-null "id")
            let has_method = raw.get("method").is_some();
            let has_id = raw.get("id").map(|v| !v.is_null()).unwrap_or(false);
            if !has_method || has_id {
                // This is an RPC response, not a notification — skip it.
                debug!(
                    server = %self.server_name,
                    "process_notification: skipping non-notification message"
                );
                return;
            }

            let method = raw["method"].as_str().unwrap_or("");
            match method {
                "notifications/resources/updated" => {
                    let uri = raw["params"]["uri"].as_str().unwrap_or("").to_string();
                    let key = (self.server_name.clone(), uri.clone());
                    if let Some(tx) = resource_subscriptions.get(&key) {
                        let event = ResourceChangedEvent {
                            server_name: self.server_name.clone(),
                            uri,
                        };
                        if let Err(e) = tx.send(event).await {
                            debug!(
                                server = %self.server_name,
                                error = %e,
                                "process_notification: resource subscription receiver dropped"
                            );
                        }
                    } else {
                        debug!(
                            server = %self.server_name,
                            uri = %raw["params"]["uri"],
                            "process_notification: no subscriber for resource update"
                        );
                    }
                }
                "notifications/tools/list_changed" => {
                    info!(server = %self.server_name, "MCP tools list changed");
                }
                other => {
                    debug!(
                        server = %self.server_name,
                        method = %other,
                        "Unhandled MCP notification"
                    );
                }
            }
        }

        #[cfg(test)]
        pub fn new_for_test(server_name: impl Into<String>) -> Self {
            Self {
                server_name: server_name.into(),
                server_info: None,
                capabilities: ServerCapabilities::default(),
                tools: vec![],
                resources: vec![],
                prompts: vec![],
                instructions: None,
                backend: None,
            }
        }
    }

    fn apply_resource_templates(resources: &mut [McpResource]) {
        for resource in resources {
            if let Some(annotations) = &resource.annotations {
                if let Some(prompt_template) = annotations.get("prompt") {
                    if let Some(template_str) = prompt_template.as_str() {
                        let context = serde_json::json!({
                            "uri": resource.uri,
                            "name": resource.name,
                            "description": resource.description,
                            "mimeType": resource.mime_type,
                        });
                        let rendered = TemplateRenderer::render(template_str, &context);
                        resource.description = Some(rendered);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// MCP Auth State
// ---------------------------------------------------------------------------

/// Authentication state for a single MCP server.
#[derive(Debug, Clone)]
pub enum McpAuthState {
    /// Server does not require OAuth authentication.
    NotRequired,
    /// OAuth required; `auth_url` is where the user should go.
    Required { auth_url: String },
    /// Successfully authenticated; token may have an expiry.
    Authenticated { token_expiry: Option<chrono::DateTime<chrono::Utc>> },
    /// An error occurred reading / initiating auth.
    Error(String),
}

// ---------------------------------------------------------------------------
// MCP Manager: manages multiple server connections
// ---------------------------------------------------------------------------

/// Manages a pool of MCP server connections.
pub struct McpManager {
    clients: HashMap<String, Arc<McpClient>>,
    /// Servers that failed to connect during `connect_all`.
    failed_servers: Vec<(String, String)>, // (name, error)
    /// Original (unexpanded) server configs — needed for OAuth initiation.
    server_configs: HashMap<String, McpServerConfig>,
    /// Active resource subscriptions: (server_name, uri) → change event sender.
    pub resource_subscriptions: DashMap<(String, String), tokio::sync::mpsc::Sender<ResourceChangedEvent>>,
}

#[derive(Debug, Clone)]
pub struct McpServerCatalog {
    pub tool_count: usize,
    pub resource_count: usize,
    pub prompt_count: usize,
    pub resources: Vec<String>,
    pub prompts: Vec<String>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            failed_servers: Vec::new(),
            server_configs: HashMap::new(),
            resource_subscriptions: DashMap::new(),
        }
    }

    /// Connect to all configured MCP servers.
    ///
    /// - Expands env vars in each config before connecting.
    /// - Logs success/failure clearly.
    /// - Continues on failure (does not bail out on first error).
    /// - Tracks failed servers in `failed_servers()`.
    pub async fn connect_all(configs: &[McpServerConfig]) -> Self {
        let mut manager = Self::new();
        for config in configs {
            // Store original config for later OAuth use
            manager.server_configs.insert(config.name.clone(), config.clone());
            // Expand env vars before using the config
            let expanded = expand_server_config(config);

            match expanded.server_type.as_str() {
                "stdio" | "sse" | "http" => {
                    let auth_token = if matches!(expanded.server_type.as_str(), "sse" | "http") {
                        manager.load_token(&expanded.name).await
                    } else {
                        None
                    };
                    match client::McpClient::connect(&expanded, auth_token).await {
                        Ok(client) => {
                            info!(
                                server = %expanded.name,
                                transport = %expanded.server_type,
                                tools = client.tools.len(),
                                resources = client.resources.len(),
                                "MCP server connected"
                            );
                            manager.clients.insert(expanded.name.clone(), Arc::new(client));
                        }
                        Err(e) => {
                            error!(
                                server = %expanded.name,
                                transport = %expanded.server_type,
                                error = %e,
                                "Failed to connect to MCP server"
                            );
                            manager
                                .failed_servers
                                .push((expanded.name.clone(), e.to_string()));
                        }
                    }
                }
                other => {
                    warn!(
                        server = %expanded.name,
                        transport = other,
                        "Unsupported MCP transport type; skipping server"
                    );
                    manager.failed_servers.push((
                        expanded.name.clone(),
                        format!("unsupported transport: {}", other),
                    ));
                }
            }
        }
        manager
    }

    // -----------------------------------------------------------------------
    // Status / query API (used by /mcp command and McpConnectionManager)
    // -----------------------------------------------------------------------

    /// Return all connected server names.
    pub fn server_names(&self) -> Vec<String> {
        self.clients.keys().cloned().collect()
    }

    /// Return status for a single server.
    pub fn server_status(&self, name: &str) -> McpServerStatus {
        if let Some(client) = self.clients.get(name) {
            McpServerStatus::Connected {
                tool_count: client.tools.len(),
            }
        } else if let Some((_, err)) = self.failed_servers.iter().find(|(n, _)| n == name) {
            McpServerStatus::Disconnected {
                last_error: Some(err.clone()),
            }
        } else {
            McpServerStatus::Disconnected { last_error: None }
        }
    }

    /// Return status for every configured server (connected + failed).
    pub fn all_statuses(&self) -> HashMap<String, McpServerStatus> {
        let mut map = HashMap::new();
        for (name, client) in &self.clients {
            map.insert(
                name.clone(),
                McpServerStatus::Connected {
                    tool_count: client.tools.len(),
                },
            );
        }
        for (name, err) in &self.failed_servers {
            map.insert(
                name.clone(),
                McpServerStatus::Disconnected {
                    last_error: Some(err.clone()),
                },
            );
        }
        map
    }

    /// Servers that failed to connect during `connect_all`.
    /// Each entry is `(server_name, error_message)`.
    pub fn failed_servers(&self) -> &[(String, String)] {
        &self.failed_servers
    }

    /// Return counts and names for tools/resources/prompts on connected servers.
    pub fn server_catalog(&self, name: &str) -> Option<McpServerCatalog> {
        let client = self.clients.get(name)?;
        Some(McpServerCatalog {
            tool_count: client.tools.len(),
            resource_count: client.resources.len(),
            prompt_count: client.prompts.len(),
            resources: client.resources.iter().map(|r| r.name.clone()).collect(),
            prompts: client.prompts.iter().map(|p| p.name.clone()).collect(),
        })
    }

    // -----------------------------------------------------------------------
    // Tool / resource helpers
    // -----------------------------------------------------------------------

    /// Get all tool definitions from all connected servers.
    pub fn all_tool_definitions(&self) -> Vec<(String, ToolDefinition)> {
        let mut defs = vec![];
        for (server_name, client) in &self.clients {
            for td in client.tool_definitions() {
                // Prefix tool name with server name to avoid conflicts
                let prefixed = ToolDefinition {
                    name: format!("{}_{}", server_name, td.name),
                    description: format!("[{}] {}", server_name, td.description),
                    input_schema: td.input_schema.clone(),
                };
                defs.push((server_name.clone(), prefixed));
            }
        }
        defs
    }

    /// Execute a tool call, routing to the correct server.
    /// Tool name format: `<server_name>_<tool_name>`.
    pub async fn call_tool(
        &self,
        prefixed_name: &str,
        arguments: Option<Value>,
    ) -> anyhow::Result<CallToolResult> {
        // Find the server name by matching prefix
        for (server_name, client) in &self.clients {
            let prefix = format!("{}_", server_name);
            if let Some(tool_name) = prefixed_name.strip_prefix(&prefix) {
                return client.call_tool(tool_name, arguments).await;
            }
        }
        Err(anyhow::anyhow!(
            "No MCP server found for tool '{}'. Connected servers: [{}]",
            prefixed_name,
            self.clients.keys().cloned().collect::<Vec<_>>().join(", ")
        ))
    }

    /// Number of connected servers.
    pub fn server_count(&self) -> usize {
        self.clients.len()
    }

    /// Get server instructions (from initialize response).
    pub fn server_instructions(&self) -> Vec<(String, String)> {
        self.clients
            .iter()
            .filter_map(|(name, client)| {
                client.instructions.as_ref().map(|instr| (name.clone(), instr.clone()))
            })
            .collect()
    }

    /// List all resources from all (or a specific) connected server.
    pub async fn list_all_resources(
        &self,
        server_filter: Option<&str>,
    ) -> Vec<serde_json::Value> {
        let mut all = vec![];
        for (name, client) in &self.clients {
            if let Some(filter) = server_filter {
                if name != filter {
                    continue;
                }
            }
            match client.list_resources().await {
                Ok(resources) => {
                    for r in resources {
                        all.push(serde_json::json!({
                            "uri": r.uri,
                            "name": r.name,
                            "description": r.description,
                            "mimeType": r.mime_type,
                            "server": name,
                        }));
                    }
                }
                Err(e) => {
                    warn!(server = %name, error = %e, "Failed to list resources");
                }
            }
        }
        all
    }

    /// Read a specific resource from a named server.
    pub async fn read_resource(
        &self,
        server_name: &str,
        uri: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let client = self
            .clients
            .get(server_name)
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' not found or not connected", server_name))?;

        let contents = client.read_resource(uri).await?;
        Ok(serde_json::to_value(&contents)?)
    }

    /// List all prompts from all (or a specific) connected server.
    pub async fn list_all_prompts(
        &self,
        server_filter: Option<&str>,
    ) -> Vec<serde_json::Value> {
        let mut all = vec![];
        for (name, client) in &self.clients {
            if let Some(filter) = server_filter {
                if name != filter {
                    continue;
                }
            }
            match client.list_prompts().await {
                Ok(prompts) => {
                    for p in prompts {
                        all.push(serde_json::json!({
                            "name": p.name,
                            "description": p.description,
                            "arguments": p.arguments,
                            "server": name,
                        }));
                    }
                }
                Err(e) => {
                    warn!(server = %name, error = %e, "Failed to list prompts");
                }
            }
        }
        all
    }

    /// Get an expanded prompt from a named server by prompt name and arguments.
    ///
    /// Returns the `GetPromptResult` with fully-rendered messages suitable for
    /// injection into the conversation (mirrors TS `getMCPPrompt`).
    pub async fn get_prompt(
        &self,
        server_name: &str,
        prompt_name: &str,
        arguments: Option<std::collections::HashMap<String, String>>,
    ) -> anyhow::Result<GetPromptResult> {
        let client = self
            .clients
            .get(server_name)
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' not found or not connected", server_name))?;
        client.get_prompt(prompt_name, arguments).await
    }

    // -----------------------------------------------------------------------
    // OAuth / authentication helpers
    // -----------------------------------------------------------------------

    /// Return the current authentication state for a server.
    ///
    /// - Returns `Authenticated` if a valid (non-expired) token exists on disk.
    /// - Returns `NotRequired` for stdio servers.
    /// - Returns `Required` for remote `http` / `sse` servers that lack a valid token.
    pub fn auth_state(&self, server_name: &str) -> McpAuthState {
        let config = match self.server_configs.get(server_name) {
            Some(c) => c,
            None => return McpAuthState::NotRequired,
        };

        if !matches!(config.server_type.as_str(), "http" | "sse") {
            return McpAuthState::NotRequired;
        }

        if let Some(token) = oauth::get_mcp_token(server_name) {
            if !token.is_expired(60) {
                return McpAuthState::Authenticated {
                    token_expiry: token.expiry_datetime(),
                };
            }

            if token.refresh_token.is_some() {
                return McpAuthState::Required {
                    auth_url: format!(
                        "{} (refreshable token detected; connection will try to refresh automatically)",
                        config.url
                            .clone()
                            .unwrap_or_else(|| "(unknown URL)".to_string())
                    ),
                };
            }
        }

        McpAuthState::Required {
            auth_url: config
                .url
                .clone()
                .unwrap_or_else(|| "(unknown URL)".to_string()),
        }
    }

    /// Initiate OAuth 2.0 + PKCE for an HTTP MCP server.
    pub async fn initiate_auth(&self, server_name: &str) -> anyhow::Result<String> {
        Ok(self.begin_auth(server_name).await?.auth_url)
    }

    /// Build a full OAuth authorization session for an HTTP/SSE MCP server.
    pub async fn begin_auth(&self, server_name: &str) -> anyhow::Result<oauth::McpAuthSession> {
        let config = self
            .server_configs
            .get(server_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown MCP server: {}", server_name))?;

        let base_url = config
            .url
            .as_deref()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "MCP server '{}' has no URL configured (required for OAuth)",
                    server_name
                )
            })?;

        oauth::begin_mcp_auth(server_name, base_url).await
    }

    /// Run the browser-based OAuth flow and persist the resulting token.
    pub async fn authenticate(&self, server_name: &str) -> anyhow::Result<oauth::McpAuthResult> {
        let config = self
            .server_configs
            .get(server_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown MCP server: {}", server_name))?;

        let base_url = config
            .url
            .as_deref()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "MCP server '{}' has no URL configured (required for OAuth)",
                    server_name
                )
            })?;

        oauth::run_mcp_auth_flow(server_name, base_url).await
    }

    /// Store an OAuth access token for an MCP server.
    ///
    /// `expires_in` is the lifetime in seconds (as returned by the token endpoint).
    pub fn store_token(
        &self,
        server_name: &str,
        token: &str,
        expires_in: Option<u64>,
    ) -> anyhow::Result<()> {
        let expires_at = expires_in.map(|secs| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                + secs
        });
        let mcp_token = oauth::McpToken {
            access_token: token.to_string(),
            refresh_token: None,
            expires_at,
            scope: None,
            server_name: server_name.to_string(),
        };
        oauth::store_mcp_token(&mcp_token)
            .map_err(|e| anyhow::anyhow!("Failed to store MCP token for '{}': {}", server_name, e))
    }

    /// Load the stored OAuth access token for an MCP server, if any.
    ///
    /// Returns `None` if no token is stored or the token cannot be refreshed.
    pub async fn load_token(&self, server_name: &str) -> Option<String> {
        let config = self.server_configs.get(server_name)?;
        let server_url = config.url.as_deref()?;
        oauth::get_valid_mcp_access_token(server_name, server_url)
            .await
            .ok()
            .flatten()
    }

    // -----------------------------------------------------------------------
    // Notification dispatch loop
    // -----------------------------------------------------------------------

    /// Spawn background Tokio tasks for each connected MCP client to handle
    /// server-initiated notifications via async streams. Uses native push notifications
    /// when available (e.g., WebSocket) and falls back to polling for other transports (e.g., stdio).
    ///
    /// Routes `notifications/resources/updated` events to the appropriate sender in
    /// `self.resource_subscriptions`.
    ///
    /// Call this once after constructing an `Arc<McpManager>` (e.g. immediately
    /// after `McpManager::connect_all`).  Each notification handler task exits
    /// when the transport closes or the manager is dropped.
    pub fn spawn_notification_poll_loop(self: Arc<Self>) {
        let clients = self.clients.clone();

        // Spawn a task for each client to handle notifications via the stream
        for client in clients.values() {
            let client_clone = Arc::clone(&client);
            let manager_weak = Arc::downgrade(&self);

            tokio::spawn(async move {
                // Subscribe to the transport's notification stream
                let mut notification_stream = client_clone.subscribe_to_notifications();

                // Process notifications from the stream
                while let Some(result) = notification_stream.next().await {
                    // Check if the manager is still alive
                    let manager = match manager_weak.upgrade() {
                        Some(m) => m,
                        None => break, // Manager dropped — shut down
                    };

                    match result {
                        Ok(raw) => {
                            client_clone
                                .process_notification(raw, &manager.resource_subscriptions)
                                .await;
                        }
                        Err(e) => {
                            debug!(
                                server = %client_clone.server_name,
                                error = %e,
                                "notification stream error"
                            );
                            break;
                        }
                    }
                }
            });
        }
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// MCP result → string conversion
// ---------------------------------------------------------------------------

/// Convert an MCP tool call result to a string for the model.
///
/// Content type handling:
/// - `text`     → the text itself
/// - `image`    → `[Image: <mime_type>]` with a short base64 preview
/// - `resource` → `[Resource: <uri>]` plus text content if present
///
/// Mixed content is joined with newlines.
/// If all content is empty, returns an empty string.
pub fn mcp_result_to_string(result: &CallToolResult) -> String {
    let parts: Vec<String> = result
        .content
        .iter()
        .map(|c| match c {
            McpContent::Text { text } => text.clone(),
            McpContent::Image { data, mime_type } => {
                // Show a short preview (first 32 chars of base64) so the model
                // knows an image was returned without embedding the full blob.
                let preview_len = data.len().min(32);
                let preview = &data[..preview_len];
                let ellipsis = if data.len() > 32 { "…" } else { "" };
                format!(
                    "[Image: {} | base64 preview: {}{}]",
                    mime_type, preview, ellipsis
                )
            }
            McpContent::Resource { resource } => {
                let mut parts = vec![format!("[Resource: {}]", resource.uri)];
                if let Some(ref text) = resource.text {
                    parts.push(text.clone());
                } else if resource.blob.is_some() {
                    let mime = resource
                        .mime_type
                        .as_deref()
                        .unwrap_or("application/octet-stream");
                    parts.push(format!("[Binary resource: {}]", mime));
                }
                parts.join("\n")
            }
        })
        .collect();

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- env expansion -----------------------------------------------------

    #[test]
    fn test_expand_env_vars_no_vars() {
        assert_eq!(expand_env_vars("hello world"), "hello world");
    }

    #[test]
    fn test_expand_env_vars_known_var() {
        std::env::set_var("_CC_TEST_VAR", "rustacean");
        let out = expand_env_vars("hello ${_CC_TEST_VAR}!");
        assert_eq!(out, "hello rustacean!");
        std::env::remove_var("_CC_TEST_VAR");
    }

    #[test]
    fn test_expand_env_vars_default_value() {
        std::env::remove_var("_CC_MISSING_VAR");
        let out = expand_env_vars("val=${_CC_MISSING_VAR:-fallback}");
        assert_eq!(out, "val=fallback");
    }

    #[test]
    fn test_expand_env_vars_missing_no_default() {
        std::env::remove_var("_CC_REALLY_MISSING");
        // Missing with no default → keep original
        let out = expand_env_vars("${_CC_REALLY_MISSING}");
        assert_eq!(out, "${_CC_REALLY_MISSING}");
    }

    #[test]
    fn test_expand_env_vars_multiple() {
        std::env::set_var("_CC_A", "foo");
        std::env::set_var("_CC_B", "bar");
        let out = expand_env_vars("${_CC_A}/${_CC_B}");
        assert_eq!(out, "foo/bar");
        std::env::remove_var("_CC_A");
        std::env::remove_var("_CC_B");
    }

    #[test]
    fn test_expand_server_config() {
        std::env::set_var("_CC_TEST_HOME", "/home/user");
        let cfg = McpServerConfig {
            name: "test".to_string(),
            command: Some("${_CC_TEST_HOME}/bin/server".to_string()),
            args: vec!["--root".to_string(), "${_CC_TEST_HOME}".to_string()],
            env: {
                let mut m = HashMap::new();
                m.insert("PATH".to_string(), "${_CC_TEST_HOME}/bin".to_string());
                m
            },
            url: None,
            server_type: "stdio".to_string(),
        };
        let expanded = expand_server_config(&cfg);
        assert_eq!(expanded.command.as_deref(), Some("/home/user/bin/server"));
        assert_eq!(expanded.args[1], "/home/user");
        assert_eq!(expanded.env.get("PATH").map(|s| s.as_str()), Some("/home/user/bin"));
        std::env::remove_var("_CC_TEST_HOME");
    }

    // ---- JSON-RPC -----------------------------------------------------------

    #[test]
    fn test_json_rpc_request_serialization() {
        let req = JsonRpcRequest::new(1u64, "tools/list", None);
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"tools/list\""));
        assert!(json.contains("\"id\":1"));
    }

    #[test]
    fn test_json_rpc_notification_omits_id() {
        let req = JsonRpcRequest::notification("notifications/initialized", None);
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"notifications/initialized\""));
        assert!(!json.contains("\"id\""));
    }

    // ---- McpTool → ToolDefinition ------------------------------------------

    #[test]
    fn test_mcp_tool_to_definition() {
        let tool = McpTool {
            name: "search".to_string(),
            description: Some("Search the web".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } }
            }),
        };
        let def: ToolDefinition = (&tool).into();
        assert_eq!(def.name, "search");
        assert_eq!(def.description, "Search the web");
    }

    // ---- mcp_result_to_string ----------------------------------------------

    #[test]
    fn test_result_to_string_text() {
        let result = CallToolResult {
            content: vec![McpContent::Text {
                text: "hello".to_string(),
            }],
            is_error: false,
        };
        assert_eq!(mcp_result_to_string(&result), "hello");
    }

    #[test]
    fn test_result_to_string_image() {
        let result = CallToolResult {
            content: vec![McpContent::Image {
                data: "abc123".to_string(),
                mime_type: "image/png".to_string(),
            }],
            is_error: false,
        };
        let s = mcp_result_to_string(&result);
        assert!(s.contains("Image:"));
        assert!(s.contains("image/png"));
        assert!(s.contains("abc123"));
    }

    #[test]
    fn test_result_to_string_resource_with_text() {
        let result = CallToolResult {
            content: vec![McpContent::Resource {
                resource: ResourceContents {
                    uri: "file:///foo.txt".to_string(),
                    mime_type: Some("text/plain".to_string()),
                    text: Some("file contents".to_string()),
                    blob: None,
                },
            }],
            is_error: false,
        };
        let s = mcp_result_to_string(&result);
        assert!(s.contains("[Resource: file:///foo.txt]"));
        assert!(s.contains("file contents"));
    }

    #[test]
    fn test_result_to_string_resource_binary() {
        let result = CallToolResult {
            content: vec![McpContent::Resource {
                resource: ResourceContents {
                    uri: "file:///img.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                    text: None,
                    blob: Some("BASE64==".to_string()),
                },
            }],
            is_error: false,
        };
        let s = mcp_result_to_string(&result);
        assert!(s.contains("[Resource: file:///img.png]"));
        assert!(s.contains("[Binary resource: image/png]"));
    }

    #[test]
    fn test_result_to_string_mixed() {
        let result = CallToolResult {
            content: vec![
                McpContent::Text {
                    text: "line one".to_string(),
                },
                McpContent::Text {
                    text: "line two".to_string(),
                },
            ],
            is_error: false,
        };
        assert_eq!(mcp_result_to_string(&result), "line one\nline two");
    }

    // ---- McpManager --------------------------------------------------------

    #[test]
    fn test_manager_server_names_empty() {
        let mgr = McpManager::new();
        assert!(mgr.server_names().is_empty());
    }

    #[test]
    fn test_manager_all_statuses_empty() {
        let mgr = McpManager::new();
        assert!(mgr.all_statuses().is_empty());
    }

    #[test]
    fn test_manager_failed_servers_empty() {
        let mgr = McpManager::new();
        assert!(mgr.failed_servers().is_empty());
    }

    #[test]
    fn test_auth_state_uses_token_expiry_datetime() {
        let mut mgr = McpManager::new();
        mgr.server_configs.insert(
            "remote".to_string(),
            McpServerConfig {
                name: "remote".to_string(),
                command: None,
                args: vec![],
                env: HashMap::new(),
                url: Some("https://example.com/mcp".to_string()),
                server_type: "http".to_string(),
            },
        );

        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 3600;
        let token = oauth::McpToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at: Some(expires_at),
            scope: None,
            server_name: "remote".to_string(),
        };
        oauth::store_mcp_token(&token).expect("store token");

        match mgr.auth_state("remote") {
            McpAuthState::Authenticated { token_expiry } => {
                assert_eq!(token_expiry, token.expiry_datetime());
            }
            other => panic!("expected authenticated, got {:?}", other),
        }

        oauth::remove_mcp_token("remote").ok();
    }
}

// ---------------------------------------------------------------------------
// Resource subscriptions (T2-12)
// ---------------------------------------------------------------------------

use tokio::sync::mpsc as tokio_mpsc;

/// Notification that a resource has changed.
#[derive(Debug, Clone)]
pub struct ResourceChangedEvent {
    pub server_name: String,
    pub uri: String,
}

/// Subscription handle for a single MCP resource URI.
pub struct ResourceSubscription {
    pub server_name: String,
    pub uri: String,
}

/// Subscribe to resource changes on an MCP server.
///
/// Sends the `resources/subscribe` JSON-RPC request to the named server and
/// returns a channel receiver that will deliver [`ResourceChangedEvent`] values
/// whenever the server fires a `notifications/resources/updated` notification.
/// The notification dispatch loop (elsewhere) looks up the tx in
/// `manager.resource_subscriptions` and forwards events.
///
/// If the server is not connected or the RPC fails, a dead receiver is returned
/// (no events will ever be delivered).
pub async fn subscribe_resource(
    manager: &McpManager,
    server_name: &str,
    uri: &str,
) -> tokio_mpsc::Receiver<ResourceChangedEvent> {
    let make_dead = || {
        let (_tx, rx) = tokio_mpsc::channel::<ResourceChangedEvent>(1);
        rx
    };

    let client = match manager.clients.get(server_name) {
        Some(c) => c,
        None => {
            tracing::warn!(server_name, uri, "subscribe_resource: server not connected");
            return make_dead();
        }
    };

    if let Err(e) = client.subscribe_resource(uri).await {
        tracing::warn!(server_name, uri, error = %e, "subscribe_resource RPC failed");
        return make_dead();
    }

    let (tx, rx) = tokio_mpsc::channel(32);
    manager
        .resource_subscriptions
        .insert((server_name.to_string(), uri.to_string()), tx);
    tracing::info!(server_name, uri, "MCP resource subscription registered");
    rx
}

/// Unsubscribe from resource change notifications.
///
/// Sends `resources/unsubscribe` JSON-RPC request to the named server via
/// `McpManager`.  Returns an error if the server is not connected or the
/// request fails.
pub async fn unsubscribe_resource(
    manager: &McpManager,
    server_name: &str,
    uri: &str,
) -> Result<(), String> {
    let client = manager
        .clients
        .get(server_name)
        .ok_or_else(|| format!("unsubscribe_resource: server '{}' not connected", server_name))?;

    client
        .unsubscribe_resource(uri)
        .await
        .map_err(|e| format!("unsubscribe_resource failed: {e}"))
        .map(|_| ())
}

// ---------------------------------------------------------------------------
// Transport tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod transport_tests {
    use super::transport::*;
    use tokio::sync::mpsc as tokio_mpsc;

    #[tokio::test]
    async fn test_route_incoming_value_sends_response_to_response_queue() {
        let (response_tx, mut response_rx) = tokio_mpsc::unbounded_channel();
        let (notification_tx, mut notification_rx) = tokio_mpsc::unbounded_channel();
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "result": {"ok": true}
        });

        route_incoming_value("srv", value, &response_tx, &notification_tx).unwrap();

        let response = response_rx.recv().await.expect("expected response");
        assert_eq!(response.id, Some(serde_json::json!(7)));
        assert!(notification_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_route_incoming_value_sends_notification_to_notification_queue() {
        let (response_tx, mut response_rx) = tokio_mpsc::unbounded_channel();
        let (notification_tx, mut notification_rx) = tokio_mpsc::unbounded_channel();
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/resources/updated",
            "params": {"uri": "file:///foo.txt"}
        });

        route_incoming_value("srv", value.clone(), &response_tx, &notification_tx).unwrap();

        let notification = notification_rx.recv().await.expect("expected notification");
        assert_eq!(notification, value);
        assert!(response_rx.try_recv().is_err());
    }

    #[test]
    fn test_resolve_legacy_endpoint_supports_relative_path() {
        let endpoint = resolve_legacy_endpoint("https://example.com/mcp", "/messages").unwrap();
        assert_eq!(endpoint, "https://example.com/messages");
    }

    #[test]
    fn test_process_sse_line_collects_event_and_data() {
        let mut event_name = None;
        let mut data_lines = Vec::new();
        process_sse_line("event: endpoint", &mut event_name, &mut data_lines);
        process_sse_line("data: /messages", &mut event_name, &mut data_lines);
        assert_eq!(event_name.as_deref(), Some("endpoint"));
        assert_eq!(data_lines, vec!["/messages".to_string()]);
    }

    #[test]
    fn test_is_unsupported_protocol_error_matches_rmcp_variants() {
        assert!(crate::McpClient::is_unsupported_protocol_error(
            "unsupported protocol version: 2025-11-25"
        ));
        assert!(crate::McpClient::is_unsupported_protocol_error(
            "Bad Request: Unsupported MCP-Protocol-Version: 2025-11-25"
        ));
        assert!(!crate::McpClient::is_unsupported_protocol_error("connection reset by peer"));
    }

}

// ---------------------------------------------------------------------------
// Notification dispatch tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod notification_tests {
    use super::*;

    #[tokio::test]
    async fn test_process_notification_routes_resource_updated() {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/resources/updated",
            "params": { "uri": "file:///foo.txt" }
        });

        let client = client::McpClient::new_for_test("myserver");

        let subscriptions: DashMap<
            (String, String),
            tokio::sync::mpsc::Sender<ResourceChangedEvent>,
        > = DashMap::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ResourceChangedEvent>(4);
        subscriptions.insert(("myserver".to_string(), "file:///foo.txt".to_string()), tx);

        client.process_notification(notification, &subscriptions).await;

        let event = rx.try_recv().expect("expected a ResourceChangedEvent");
        assert_eq!(event.server_name, "myserver");
        assert_eq!(event.uri, "file:///foo.txt");
        assert!(rx.try_recv().is_err(), "channel should be empty after one event");
    }

    #[tokio::test]
    async fn test_process_notification_no_subscriber_does_not_panic() {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/resources/updated",
            "params": { "uri": "file:///unsubscribed.txt" }
        });

        let client = client::McpClient::new_for_test("myserver");
        let subscriptions: DashMap<
            (String, String),
            tokio::sync::mpsc::Sender<ResourceChangedEvent>,
        > = DashMap::new();
        client.process_notification(notification, &subscriptions).await;
    }

    #[tokio::test]
    async fn test_process_notification_tools_list_changed() {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/tools/list_changed",
            "params": {}
        });

        let client = client::McpClient::new_for_test("myserver");
        let subscriptions: DashMap<
            (String, String),
            tokio::sync::mpsc::Sender<ResourceChangedEvent>,
        > = DashMap::new();
        client.process_notification(notification, &subscriptions).await;
    }

    #[tokio::test]
    async fn test_process_notification_multiple_events() {
        let n1 = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/resources/updated",
            "params": { "uri": "file:///a.txt" }
        });
        let n2 = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/resources/updated",
            "params": { "uri": "file:///b.txt" }
        });

        let client = client::McpClient::new_for_test("s1");

        let subscriptions: DashMap<
            (String, String),
            tokio::sync::mpsc::Sender<ResourceChangedEvent>,
        > = DashMap::new();
        let (tx_a, mut rx_a) = tokio::sync::mpsc::channel::<ResourceChangedEvent>(4);
        let (tx_b, mut rx_b) = tokio::sync::mpsc::channel::<ResourceChangedEvent>(4);
        subscriptions.insert(("s1".to_string(), "file:///a.txt".to_string()), tx_a);
        subscriptions.insert(("s1".to_string(), "file:///b.txt".to_string()), tx_b);

        client.process_notification(n1, &subscriptions).await;
        client.process_notification(n2, &subscriptions).await;

        let ev_a = rx_a.try_recv().expect("expected event for a.txt");
        assert_eq!(ev_a.uri, "file:///a.txt");

        let ev_b = rx_b.try_recv().expect("expected event for b.txt");
        assert_eq!(ev_b.uri, "file:///b.txt");
    }

    #[tokio::test]
    async fn test_process_notification_skips_rpc_responses() {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "notifications/resources/updated",
            "params": { "uri": "file:///foo.txt" }
        });

        let client = client::McpClient::new_for_test("myserver");

        let subscriptions: DashMap<
            (String, String),
            tokio::sync::mpsc::Sender<ResourceChangedEvent>,
        > = DashMap::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ResourceChangedEvent>(4);
        subscriptions.insert(("myserver".to_string(), "file:///foo.txt".to_string()), tx);

        client.process_notification(response, &subscriptions).await;

        assert!(
            rx.try_recv().is_err(),
            "RPC response must not be dispatched as a notification"
        );
    }
}
