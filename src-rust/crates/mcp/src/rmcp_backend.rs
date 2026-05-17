use crate::backend::{McpBackendKind, McpClientBackend, McpClientSnapshot};
use crate::transport;
use crate::types::{
    CallToolResult, GetPromptResult, McpContent, McpPrompt, McpPromptArgument,
    McpResource, McpTool, PromptMessage, PromptMessageContent, PromptsCapability,
    ResourceContents, ResourcesCapability, ServerCapabilities, ServerInfo, ToolsCapability,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use rmcp::model as rmcp_model;
use rmcp::service::RunningService;
use rmcp::transport::{
    ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess,
    streamable_http_client::StreamableHttpClientTransportConfig,
};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;

pub struct RmcpNotificationClient {
    notifications_tx: mpsc::UnboundedSender<Value>,
    client_info: rmcp_model::ClientInfo,
}

impl RmcpNotificationClient {
    pub fn new(
        notifications_tx: mpsc::UnboundedSender<Value>,
        client_info: rmcp_model::ClientInfo,
    ) -> Self {
        Self {
            notifications_tx,
            client_info,
        }
    }

    fn send_notification(&self, method: &str, params: Option<Value>) {
        let payload = match params {
            Some(params) => json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }),
            None => json!({
                "jsonrpc": "2.0",
                "method": method,
            }),
        };
        let _ = self.notifications_tx.send(payload);
    }
}

impl ClientHandler for RmcpNotificationClient {
    async fn on_resource_updated(
        &self,
        params: rmcp_model::ResourceUpdatedNotificationParam,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.send_notification(
            "notifications/resources/updated",
            Some(json!({ "uri": params.uri })),
        );
    }

    async fn on_resource_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.send_notification("notifications/resources/list_changed", Some(json!({})));
    }

    async fn on_tool_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.send_notification("notifications/tools/list_changed", Some(json!({})));
    }

    async fn on_prompt_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.send_notification("notifications/prompts/list_changed", Some(json!({})));
    }

    fn get_info(&self) -> rmcp_model::ClientInfo {
        self.client_info.clone()
    }
}

fn build_stdio_command(command: &str, config: &claurst_core::config::McpServerConfig) -> Command {
    let program = which::which(command).unwrap_or_else(|_| command.into());
    Command::new(program).configure(|cmd| {
        cmd.args(&config.args);
        cmd.envs(&config.env);
    })
}

pub struct RmcpClientBackend {
    snapshot: McpClientSnapshot,
    peer: rmcp::Peer<RoleClient>,
    #[allow(dead_code)]
    running: Mutex<RunningService<RoleClient, RmcpNotificationClient>>,
    notifications_rx: Arc<Mutex<mpsc::UnboundedReceiver<Value>>>,
}

impl RmcpClientBackend {
    pub async fn connect_stdio(
        config: &claurst_core::config::McpServerConfig,
    ) -> anyhow::Result<Self> {
        let command = config
            .command
            .clone()
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' has no command configured", config.name))?;

        let transport = TokioChildProcess::new(build_stdio_command(&command, config))
        .map_err(|e| anyhow::anyhow!("failed to spawn rmcp stdio child for '{}': {}", config.name, e))?;

        let client_info = build_client_info(rmcp_model::ProtocolVersion::default());
        Self::connect_with_transport(config, transport, client_info, "stdio").await
    }

    pub async fn connect_http(
        config: &claurst_core::config::McpServerConfig,
        auth_token: Option<String>,
        protocol_version: rmcp_model::ProtocolVersion,
    ) -> anyhow::Result<Self> {
        let endpoint = config
            .url
            .clone()
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' has no URL configured", config.name))?;

        let mut transport_config = StreamableHttpClientTransportConfig::with_uri(endpoint);
        if let Some(token) = auth_token {
            transport_config = transport_config.auth_header(token);
        }

        let transport = StreamableHttpClientTransport::from_config(transport_config);
        let client_info = build_client_info(protocol_version);
        Self::connect_with_transport(config, transport, client_info, "http").await
    }

    pub async fn connect_legacy_sse(
        config: &claurst_core::config::McpServerConfig,
        auth_token: Option<String>,
    ) -> anyhow::Result<Self> {
        // Legacy SSE servers still expose the real POST endpoint via
        // `event: endpoint`; keep a thin compatibility shim here and hand the
        // session and capability flow back to rmcp once connected.
        let transport = LegacySseRmcpTransport::connect(config, auth_token).await?;
        let client_info = build_client_info(rmcp_model::ProtocolVersion::V_2024_11_05);
        Self::connect_with_transport(config, transport, client_info, "sse").await
    }

    async fn connect_with_transport<T, E, A>(
        config: &claurst_core::config::McpServerConfig,
        transport: T,
        client_info: rmcp_model::ClientInfo,
        transport_label: &str,
    ) -> anyhow::Result<Self>
    where
        T: rmcp::transport::IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();
        let handler = RmcpNotificationClient::new(notifications_tx, client_info);
        let running = handler.serve(transport).await.map_err(|e| {
            anyhow::anyhow!(
                "rmcp {} client '{}' failed to initialize: {}",
                transport_label,
                config.name,
                e
            )
        })?;

        let peer = running.peer().clone();
        let snapshot = build_snapshot(config, &peer).await?;

        Ok(Self {
            snapshot,
            peer,
            running: Mutex::new(running),
            notifications_rx: Arc::new(Mutex::new(notifications_rx)),
        })
    }
}

fn build_client_info(protocol_version: rmcp_model::ProtocolVersion) -> rmcp_model::ClientInfo {
    let mut client_info = rmcp_model::ClientInfo::default();
    client_info.protocol_version = protocol_version;
    client_info.capabilities.roots = Some(rmcp_model::RootsCapabilities {
        list_changed: Some(false),
    });
    client_info.client_info = rmcp_model::Implementation::new(
        claurst_core::constants::APP_NAME,
        claurst_core::constants::APP_VERSION,
    );
    client_info
}

#[derive(Debug, thiserror::Error)]
enum LegacySseTransportError {
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

struct LegacySseRmcpTransport {
    server_name: String,
    sse_url: String,
    client: reqwest::Client,
    auth_token: Option<String>,
    post_endpoint: Arc<StdMutex<Option<String>>>,
    incoming_tx: mpsc::UnboundedSender<rmcp::service::RxJsonRpcMessage<RoleClient>>,
    incoming_rx: Arc<Mutex<mpsc::UnboundedReceiver<rmcp::service::RxJsonRpcMessage<RoleClient>>>>,
    background_tasks: Arc<StdMutex<Vec<JoinHandle<()>>>>,
}

impl LegacySseRmcpTransport {
    async fn connect(
        config: &claurst_core::config::McpServerConfig,
        auth_token: Option<String>,
    ) -> anyhow::Result<Self> {
        let sse_url = config
            .url
            .clone()
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' has no URL configured", config.name))?;
        let client = reqwest::Client::new();
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
        let transport = Self {
            server_name: config.name.clone(),
            sse_url,
            client,
            auth_token,
            post_endpoint: Arc::new(StdMutex::new(None)),
            incoming_tx,
            incoming_rx: Arc::new(Mutex::new(incoming_rx)),
            background_tasks: Arc::new(StdMutex::new(Vec::new())),
        };
        transport.start_sse_listener().await?;
        Ok(transport)
    }

    async fn start_sse_listener(&self) -> anyhow::Result<()> {
        let (endpoint_tx, endpoint_rx) = oneshot::channel::<anyhow::Result<String>>();
        let endpoint_tx = Arc::new(StdMutex::new(Some(endpoint_tx)));

        let mut request = self
            .client
            .get(&self.sse_url)
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(token) = &self.auth_token {
            request = request.header(
                reqwest::header::AUTHORIZATION,
                transport::bearer_header_value(token)?,
            );
        }
        let response = request.send().await.map_err(|e| {
            anyhow::anyhow!(
                "MCP server '{}': failed to open legacy SSE stream '{}': {}",
                self.server_name,
                self.sse_url,
                e
            )
        })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "MCP server '{}': legacy SSE stream returned HTTP {}: {}",
                self.server_name,
                status,
                body
            );
        }

        let server_name = self.server_name.clone();
        let sse_url = self.sse_url.clone();
        let post_endpoint = Arc::clone(&self.post_endpoint);
        let incoming_tx = self.incoming_tx.clone();
        let endpoint_tx_for_task = Arc::clone(&endpoint_tx);
        let task = tokio::spawn(async move {
            let result = transport::process_sse_response(response, |event, data| {
                if matches!(event, Some("endpoint")) {
                    let endpoint = transport::resolve_legacy_endpoint(&sse_url, data)?;
                    *post_endpoint.lock().expect("endpoint mutex poisoned") = Some(endpoint.clone());
                    if let Some(tx) = endpoint_tx_for_task
                        .lock()
                        .expect("endpoint sender mutex poisoned")
                        .take()
                    {
                        let _ = tx.send(Ok(endpoint));
                    }
                    return Ok(());
                }

                if data.trim().is_empty() {
                    return Ok(());
                }

                let message = parse_server_message(&server_name, data)?;
                let _ = incoming_tx.send(message);
                Ok(())
            })
            .await;

            if let Err(e) = result {
                tracing::warn!(server = %server_name, error = %e, "Legacy SSE stream closed with error");
                if let Some(tx) = endpoint_tx_for_task
                    .lock()
                    .expect("endpoint sender mutex poisoned")
                    .take()
                {
                    let _ = tx.send(Err(anyhow::anyhow!(e.to_string())));
                }
            } else if let Some(tx) = endpoint_tx_for_task
                .lock()
                .expect("endpoint sender mutex poisoned")
                .take()
            {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "legacy SSE stream closed before announcing endpoint"
                )));
            }
        });
        self.background_tasks.lock().expect("task mutex poisoned").push(task);

        let endpoint = tokio::time::timeout(std::time::Duration::from_secs(10), endpoint_rx)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "MCP server '{}': timed out waiting for legacy SSE endpoint event",
                    self.server_name
                )
            })?
            .map_err(|_| {
                anyhow::anyhow!(
                    "MCP server '{}': legacy SSE stream ended before endpoint discovery",
                    self.server_name
                )
            })??;
        *self.post_endpoint.lock().expect("endpoint mutex poisoned") = Some(endpoint);
        Ok(())
    }
}

impl Drop for LegacySseRmcpTransport {
    fn drop(&mut self) {
        // Some upper-layer paths drop the backend instead of calling close()
        // explicitly. Abort the listener tasks again here so legacy SSE does
        // not outlive the connection teardown.
        let mut tasks = self.background_tasks.lock().expect("task mutex poisoned");
        for handle in tasks.drain(..) {
            handle.abort();
        }
    }
}

impl rmcp::transport::Transport<RoleClient> for LegacySseRmcpTransport {
    type Error = LegacySseTransportError;

    fn send(
        &mut self,
        item: rmcp::service::TxJsonRpcMessage<RoleClient>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        let endpoint = self
            .post_endpoint
            .lock()
            .expect("endpoint mutex poisoned")
            .clone();
        let client = self.client.clone();
        let auth_token = self.auth_token.clone();
        let server_name = self.server_name.clone();
        let incoming_tx = self.incoming_tx.clone();
        let background_tasks = Arc::clone(&self.background_tasks);
        async move {
            let endpoint = endpoint.ok_or_else(|| {
                anyhow::anyhow!(
                    "MCP server '{}': legacy SSE POST endpoint has not been discovered",
                    server_name
                )
            })?;

            let mut request = client
                .post(endpoint)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .json(&item);
            if let Some(token) = &auth_token {
                request = request.header(
                    reqwest::header::AUTHORIZATION,
                    transport::bearer_header_value(token)?,
                );
            }

            let response = request.send().await.map_err(|e| {
                anyhow::anyhow!(
                    "MCP server '{}': legacy SSE POST request failed: {}",
                    server_name,
                    e
                )
            })?;

            handle_legacy_sse_http_response(server_name, response, incoming_tx, background_tasks)
                .await
                .map_err(Into::into)
        }
    }

    fn receive(
        &mut self,
    ) -> impl std::future::Future<Output = Option<rmcp::service::RxJsonRpcMessage<RoleClient>>> + Send {
        let incoming_rx = Arc::clone(&self.incoming_rx);
        async move {
            let mut rx = incoming_rx.lock().await;
            rx.recv().await
        }
    }

    fn close(&mut self) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        let background_tasks = Arc::clone(&self.background_tasks);
        async move {
            let mut tasks = background_tasks.lock().expect("task mutex poisoned");
            for handle in tasks.drain(..) {
                handle.abort();
            }
            Ok(())
        }
    }
}

fn parse_server_message(
    server_name: &str,
    data: &str,
) -> anyhow::Result<rmcp::service::RxJsonRpcMessage<RoleClient>> {
    serde_json::from_str(data).map_err(|e| {
        anyhow::anyhow!(
            "MCP server '{}': failed to parse legacy SSE JSON payload: {}",
            server_name,
            e
        )
    })
}

async fn handle_legacy_sse_http_response(
    server_name: String,
    response: reqwest::Response,
    incoming_tx: mpsc::UnboundedSender<rmcp::service::RxJsonRpcMessage<RoleClient>>,
    background_tasks: Arc<StdMutex<Vec<JoinHandle<()>>>>,
) -> anyhow::Result<()> {
    let status = response.status();
    if !status.is_success() && status != reqwest::StatusCode::ACCEPTED {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "MCP server '{}': HTTP {} from legacy SSE transport: {}",
            server_name,
            status,
            body
        );
    }

    if status == reqwest::StatusCode::ACCEPTED {
        return Ok(());
    }

    if transport::is_event_stream_response(&response) {
        let server_name_for_task = server_name.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = transport::process_sse_response(response, |_, data| {
                if data.trim().is_empty() {
                    return Ok(());
                }
                let message = parse_server_message(&server_name_for_task, data)?;
                let _ = incoming_tx.send(message);
                Ok(())
            })
            .await
            {
                tracing::warn!(server = %server_name_for_task, error = %e, "legacy SSE POST stream closed with error");
            }
        });
        background_tasks.lock().expect("task mutex poisoned").push(task);
        return Ok(());
    }

    let text = response.text().await.map_err(|e| {
        anyhow::anyhow!(
            "MCP server '{}': failed to read legacy SSE HTTP response body: {}",
            server_name,
            e
        )
    })?;
    if text.trim().is_empty() {
        return Ok(());
    }

    let message = parse_server_message(&server_name, &text)?;
    let _ = incoming_tx.send(message);
    Ok(())
}

#[async_trait]
impl McpClientBackend for RmcpClientBackend {
    fn kind(&self) -> McpBackendKind {
        McpBackendKind::Rmcp
    }

    fn snapshot(&self) -> McpClientSnapshot {
        self.snapshot.clone()
    }

    async fn list_tools(&self) -> anyhow::Result<Vec<McpTool>> {
        let tools = self
            .peer
            .list_all_tools()
            .await
            .map_err(|e| anyhow::anyhow!("rmcp list_tools failed: {}", e))?;
        Ok(tools.into_iter().map(convert_tool).collect())
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Option<Value>,
    ) -> anyhow::Result<CallToolResult> {
        let mut params = rmcp_model::CallToolRequestParams::new(name.to_string());
        if let Some(arguments) = arguments {
            params = params.with_arguments(json_value_to_object(arguments)?);
        }
        let result = self
            .peer
            .call_tool(params)
            .await
            .map_err(|e| anyhow::anyhow!("rmcp call_tool '{}' failed: {}", name, e))?;
        Ok(convert_call_tool_result(result))
    }

    async fn list_resources(&self) -> anyhow::Result<Vec<McpResource>> {
        let resources = self
            .peer
            .list_all_resources()
            .await
            .map_err(|e| anyhow::anyhow!("rmcp list_resources failed: {}", e))?;
        Ok(resources.into_iter().map(convert_resource).collect())
    }

    async fn read_resource(&self, uri: &str) -> anyhow::Result<ResourceContents> {
        let result = self
            .peer
            .read_resource(rmcp_model::ReadResourceRequestParams::new(uri.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("rmcp read_resource '{}' failed: {}", uri, e))?;
        let first = result
            .contents
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("rmcp read_resource '{}' returned no contents", uri))?;
        Ok(convert_resource_contents(first))
    }

    async fn list_prompts(&self) -> anyhow::Result<Vec<McpPrompt>> {
        let prompts = self
            .peer
            .list_all_prompts()
            .await
            .map_err(|e| anyhow::anyhow!("rmcp list_prompts failed: {}", e))?;
        Ok(prompts.into_iter().map(convert_prompt).collect())
    }

    async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<HashMap<String, String>>,
    ) -> anyhow::Result<GetPromptResult> {
        let mut params = rmcp_model::GetPromptRequestParams::new(name.to_string());
        if let Some(arguments) = arguments {
            let args = arguments
                .into_iter()
                .map(|(key, value)| (key, Value::String(value)))
                .collect();
            params = params.with_arguments(args);
        }
        let result = self
            .peer
            .get_prompt(params)
            .await
            .map_err(|e| anyhow::anyhow!("rmcp get_prompt '{}' failed: {}", name, e))?;
        Ok(convert_get_prompt_result(result))
    }

    async fn subscribe_resource(&self, uri: &str) -> anyhow::Result<()> {
        self.peer
            .subscribe(rmcp_model::SubscribeRequestParams::new(uri.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("rmcp subscribe '{}' failed: {}", uri, e))
    }

    async fn unsubscribe_resource(&self, uri: &str) -> anyhow::Result<()> {
        self.peer
            .unsubscribe(rmcp_model::UnsubscribeRequestParams::new(uri.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("rmcp unsubscribe '{}' failed: {}", uri, e))
    }

    fn subscribe_to_notifications(&self) -> BoxStream<'static, anyhow::Result<Value>> {
        let notifications_rx = Arc::clone(&self.notifications_rx);
        let (tx, rx) = mpsc::channel::<anyhow::Result<Value>>(100);
        tokio::spawn(async move {
            loop {
                let next = {
                    let mut receiver = notifications_rx.lock().await;
                    receiver.recv().await
                };
                match next {
                    Some(value) => {
                        if tx.send(Ok(value)).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        });
        Box::pin(ReceiverStream::new(rx))
    }
}

async fn build_snapshot(
    config: &claurst_core::config::McpServerConfig,
    peer: &rmcp::Peer<RoleClient>,
) -> anyhow::Result<McpClientSnapshot> {
    let server_info = peer.peer_info().cloned();

    let (server_info_value, capabilities, instructions) = match server_info {
        Some(info) => (
            Some(ServerInfo {
                name: info.server_info.name,
                version: info.server_info.version,
            }),
            convert_server_capabilities(info.capabilities),
            info.instructions,
        ),
        None => (None, ServerCapabilities::default(), None),
    };

    let tools = if capabilities.tools.is_some() {
        peer.list_all_tools()
            .await
            .map_err(|e| anyhow::anyhow!("rmcp list_all_tools failed: {}", e))?
            .into_iter()
            .map(convert_tool)
            .collect()
    } else {
        vec![]
    };

    let resources = if capabilities.resources.is_some() {
        peer.list_all_resources()
            .await
            .map_err(|e| anyhow::anyhow!("rmcp list_all_resources failed: {}", e))?
            .into_iter()
            .map(convert_resource)
            .collect()
    } else {
        vec![]
    };

    let prompts = if capabilities.prompts.is_some() {
        peer.list_all_prompts()
            .await
            .map_err(|e| anyhow::anyhow!("rmcp list_all_prompts failed: {}", e))?
            .into_iter()
            .map(convert_prompt)
            .collect()
    } else {
        vec![]
    };

    Ok(McpClientSnapshot {
        server_name: config.name.clone(),
        server_info: server_info_value,
        capabilities,
        tools,
        resources,
        prompts,
        instructions,
    })
}

fn convert_server_capabilities(capabilities: rmcp_model::ServerCapabilities) -> ServerCapabilities {
    ServerCapabilities {
        tools: capabilities.tools.map(|tools| ToolsCapability {
            list_changed: tools.list_changed.unwrap_or(false),
        }),
        resources: capabilities.resources.map(|resources| ResourcesCapability {
            subscribe: resources.subscribe.unwrap_or(false),
            list_changed: resources.list_changed.unwrap_or(false),
        }),
        prompts: capabilities.prompts.map(|prompts| PromptsCapability {
            list_changed: prompts.list_changed.unwrap_or(false),
        }),
        logging: capabilities.logging.map(Value::Object),
    }
}

fn convert_tool(tool: rmcp_model::Tool) -> McpTool {
    McpTool {
        name: tool.name.into_owned(),
        description: tool.description.map(|description| description.into_owned()),
        input_schema: Value::Object((*tool.input_schema).clone()),
    }
}

fn convert_resource(resource: rmcp_model::Resource) -> McpResource {
    McpResource {
        uri: resource.uri.clone(),
        name: resource.name.clone(),
        description: resource.description.clone(),
        mime_type: resource.mime_type.clone(),
        annotations: resource
            .annotations
            .as_ref()
            .and_then(|annotations| serde_json::to_value(annotations).ok()),
    }
}

fn convert_prompt(prompt: rmcp_model::Prompt) -> McpPrompt {
    McpPrompt {
        name: prompt.name,
        description: prompt.description,
        arguments: prompt
            .arguments
            .unwrap_or_default()
            .into_iter()
            .map(|argument| McpPromptArgument {
                name: argument.name,
                description: argument.description,
                required: argument.required.unwrap_or(false),
            })
            .collect(),
    }
}

fn convert_resource_contents(resource: rmcp_model::ResourceContents) -> ResourceContents {
    match resource {
        rmcp_model::ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            ..
        } => ResourceContents {
            uri,
            mime_type,
            text: Some(text),
            blob: None,
        },
        rmcp_model::ResourceContents::BlobResourceContents {
            uri,
            mime_type,
            blob,
            ..
        } => ResourceContents {
            uri,
            mime_type,
            text: None,
            blob: Some(blob),
        },
    }
}

fn convert_prompt_message(message: rmcp_model::PromptMessage) -> PromptMessage {
    PromptMessage {
        role: match message.role {
            rmcp_model::PromptMessageRole::User => "user".to_string(),
            rmcp_model::PromptMessageRole::Assistant => "assistant".to_string(),
        },
        content: match message.content {
            rmcp_model::PromptMessageContent::Text { text } => PromptMessageContent::Text { text },
            rmcp_model::PromptMessageContent::Image { image } => PromptMessageContent::Image {
                data: image.data.clone(),
                mime_type: image.mime_type.clone(),
            },
            rmcp_model::PromptMessageContent::Resource { resource } => {
                PromptMessageContent::Resource {
                    resource: serde_json::to_value(resource).unwrap_or(Value::Null),
                }
            }
            rmcp_model::PromptMessageContent::ResourceLink { link } => {
                PromptMessageContent::Resource {
                    resource: serde_json::to_value(link).unwrap_or(Value::Null),
                }
            }
        },
    }
}

fn convert_get_prompt_result(result: rmcp_model::GetPromptResult) -> GetPromptResult {
    GetPromptResult {
        description: result.description,
        messages: result
            .messages
            .into_iter()
            .map(convert_prompt_message)
            .collect(),
    }
}

fn convert_call_tool_result(result: rmcp_model::CallToolResult) -> CallToolResult {
    CallToolResult {
        content: result.content.into_iter().map(convert_content).collect(),
        is_error: result.is_error.unwrap_or(false),
    }
}

fn convert_content(content: rmcp_model::Content) -> McpContent {
    match content.raw {
        rmcp_model::RawContent::Text(text) => McpContent::Text { text: text.text },
        rmcp_model::RawContent::Image(image) => McpContent::Image {
            data: image.data.clone(),
            mime_type: image.mime_type.clone(),
        },
        rmcp_model::RawContent::Resource(resource) => McpContent::Resource {
            resource: convert_resource_contents(resource.resource),
        },
        rmcp_model::RawContent::ResourceLink(link) => McpContent::Resource {
            resource: ResourceContents {
                uri: link.uri,
                mime_type: link.mime_type,
                text: None,
                blob: None,
            },
        },
        rmcp_model::RawContent::Audio(audio) => McpContent::Text {
            text: format!("[Audio: {} | base64 bytes omitted]", audio.mime_type),
        },
    }
}


fn json_value_to_object(value: Value) -> anyhow::Result<rmcp_model::JsonObject> {
    match value {
        Value::Object(map) => Ok(map),
        other => Err(anyhow::anyhow!(
            "rmcp tool arguments must be a JSON object, got {}",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claurst_core::config::McpServerConfig;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::{Duration, timeout};

    fn test_sse_config(url: String) -> McpServerConfig {
        McpServerConfig {
            name: "test-sse".to_string(),
            command: None,
            args: vec![],
            env: HashMap::new(),
            url: Some(url),
            server_type: "sse".to_string(),
        }
    }

    async fn serve_single_response(
        status_line: &str,
        content_type: &str,
        body: &str,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");
        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept test connection");
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf).await;
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write test response");
            let _ = stream.shutdown().await;
        });
        format!("http://{addr}")
    }

    async fn fetch_response(status_line: &str, content_type: &str, body: &str) -> reqwest::Response {
        let url = serve_single_response(status_line, content_type, body).await;
        reqwest::Client::new()
            .get(url)
            .send()
            .await
            .expect("fetch test response")
    }

    #[test]
    fn build_stdio_command_includes_configured_env() {
        let config = McpServerConfig {
            name: "mcp-router".to_string(),
            command: Some("pnpx".to_string()),
            args: vec!["@mcp_router/cli@latest".to_string(), "connect".to_string()],
            env: HashMap::from([("MCPR_TOKEN".to_string(), "test-token".to_string())]),
            url: None,
            server_type: "stdio".to_string(),
        };

        let command = build_stdio_command("definitely-missing-mcp-command", &config);
        let envs = command.as_std().get_envs().collect::<Vec<_>>();

        assert_eq!(command.as_std().get_program().to_string_lossy(), "definitely-missing-mcp-command");
        assert!(envs.iter().any(|(key, value)| {
            key.to_string_lossy() == "MCPR_TOKEN"
                && value
                    .as_ref()
                    .is_some_and(|value| value.to_string_lossy() == "test-token")
        }));
    }

    #[test]
    fn build_client_info_sets_expected_protocol_and_identity() {
        let info = build_client_info(rmcp_model::ProtocolVersion::V_2024_11_05);
        assert_eq!(info.protocol_version, rmcp_model::ProtocolVersion::V_2024_11_05);
        assert_eq!(info.client_info.name, claurst_core::constants::APP_NAME);
        assert_eq!(info.client_info.version, claurst_core::constants::APP_VERSION);
        assert_eq!(
            info.capabilities
                .roots
                .as_ref()
                .and_then(|roots| roots.list_changed),
            Some(false)
        );
    }

    #[tokio::test]
    async fn legacy_sse_transport_connect_discovers_endpoint_event() {
        let body = "event: endpoint\ndata: /messages\n\n";
        let url = serve_single_response("200 OK", "text/event-stream", body).await;
        let config = test_sse_config(url.clone());

        let transport = LegacySseRmcpTransport::connect(&config, None)
            .await
            .expect("connect legacy sse transport");

        let endpoint = transport
            .post_endpoint
            .lock()
            .expect("endpoint mutex poisoned")
            .clone();
        let expected = format!("{url}/messages");
        assert_eq!(endpoint.as_deref(), Some(expected.as_str()));
    }

    #[tokio::test]
    async fn handle_legacy_sse_http_response_queues_json_message() {
        let response = fetch_response(
            "200 OK",
            "application/json",
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
        )
        .await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let tasks = Arc::new(StdMutex::new(Vec::new()));

        handle_legacy_sse_http_response("test".to_string(), response, tx, Arc::clone(&tasks))
            .await
            .expect("handle json response");

        let message = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("json response timeout")
            .expect("json response message");
        assert!(matches!(message, rmcp::service::RxJsonRpcMessage::<RoleClient>::Response(_)));
    }

    #[tokio::test]
    async fn handle_legacy_sse_http_response_queues_sse_message() {
        let response = fetch_response(
            "200 OK",
            "text/event-stream",
            "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n",
        )
        .await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let tasks = Arc::new(StdMutex::new(Vec::new()));

        handle_legacy_sse_http_response("test".to_string(), response, tx, Arc::clone(&tasks))
            .await
            .expect("handle sse response");

        let message = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("sse response timeout")
            .expect("sse response message");
        assert!(matches!(message, rmcp::service::RxJsonRpcMessage::<RoleClient>::Response(_)));
    }

    #[tokio::test]
    async fn legacy_sse_transport_drop_aborts_background_tasks() {
        let task = tokio::spawn(async move {
            loop {
                tokio::task::yield_now().await;
            }
        });

        let transport = LegacySseRmcpTransport {
            server_name: "test".to_string(),
            sse_url: "http://localhost/sse".to_string(),
            client: reqwest::Client::new(),
            auth_token: None,
            post_endpoint: Arc::new(StdMutex::new(None)),
            incoming_tx: mpsc::unbounded_channel().0,
            incoming_rx: Arc::new(Mutex::new(mpsc::unbounded_channel().1)),
            background_tasks: Arc::new(StdMutex::new(vec![task])),
        };

        let background_tasks = Arc::clone(&transport.background_tasks);
        drop(transport);

        let is_empty = background_tasks
            .lock()
            .expect("task mutex poisoned")
            .is_empty();
        assert!(is_empty);
    }
}
