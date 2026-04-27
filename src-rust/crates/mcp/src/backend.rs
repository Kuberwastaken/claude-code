use crate::types::{
    CallToolResult, GetPromptResult, McpPrompt, McpResource, McpTool,
    ResourceContents, ServerCapabilities, ServerInfo,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;

/// Backend implementation used by an `McpClient` instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpBackendKind {
    /// rmcp-backed implementation.
    Rmcp,
}

/// Immutable view of the server state that upper layers depend on.
#[derive(Debug, Clone)]
pub struct McpClientSnapshot {
    pub server_name: String,
    pub server_info: Option<ServerInfo>,
    pub capabilities: ServerCapabilities,
    pub tools: Vec<McpTool>,
    pub resources: Vec<McpResource>,
    pub prompts: Vec<McpPrompt>,
    pub instructions: Option<String>,
}

impl McpClientSnapshot {
    /// Create an empty snapshot for a newly constructed client.
    pub fn empty(server_name: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            server_info: None,
            capabilities: ServerCapabilities::default(),
            tools: vec![],
            resources: vec![],
            prompts: vec![],
            instructions: None,
        }
    }
}

/// Protocol/session backend used by `McpClient`.
///
/// The goal is to keep upper layers stable (`McpManager`, CLI, tools, TUI)
/// while allowing transport implementations to migrate independently.
#[async_trait]
pub trait McpClientBackend: Send + Sync {
    fn kind(&self) -> McpBackendKind;

    /// Return the latest server snapshot for populating `McpClient` fields.
    fn snapshot(&self) -> McpClientSnapshot;

    async fn list_tools(&self) -> anyhow::Result<Vec<McpTool>>;

    async fn call_tool(
        &self,
        name: &str,
        arguments: Option<Value>,
    ) -> anyhow::Result<CallToolResult>;

    async fn list_resources(&self) -> anyhow::Result<Vec<McpResource>>;

    async fn read_resource(&self, uri: &str) -> anyhow::Result<ResourceContents>;

    async fn list_prompts(&self) -> anyhow::Result<Vec<McpPrompt>>;

    async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<HashMap<String, String>>,
    ) -> anyhow::Result<GetPromptResult>;

    async fn subscribe_resource(&self, uri: &str) -> anyhow::Result<()>;

    async fn unsubscribe_resource(&self, uri: &str) -> anyhow::Result<()>;

    /// Subscribe to raw notification payloads emitted by the backend.
    fn subscribe_to_notifications(&self) -> BoxStream<'static, anyhow::Result<Value>>;
}

/// Pick the preferred backend for a configured transport type.
pub fn preferred_backend_kind(_server_type: &str) -> McpBackendKind {
    McpBackendKind::Rmcp
}
