// MCP resource tools: list and read resources from connected MCP servers.
//
// ListMcpResourcesTool – enumerate all resources available from MCP servers
// ReadMcpResourceTool  – read a specific resource by server name + URI
//
// These require an MCP manager to be configured in ToolContext.mcp_manager.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::debug;

// ---------------------------------------------------------------------------
// ListMcpResourcesTool
// ---------------------------------------------------------------------------

pub struct ListMcpResourcesTool;

#[derive(Debug, Deserialize)]
struct ListMcpResourcesInput {
    #[serde(default)]
    server: Option<String>,
}

#[async_trait]
impl Tool for ListMcpResourcesTool {
    fn name(&self) -> &str {
        "ListMcpResources"
    }

    fn description(&self) -> &str {
        "List all resources available from connected MCP servers. \
         Optionally filter by server name. \
         Resources represent data that MCP servers expose (files, database records, etc.)."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "Optional server name to filter resources by"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: ListMcpResourcesInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        let manager = match &ctx.mcp_manager {
            Some(m) => m,
            None => {
                return ToolResult::error(
                    "No MCP servers connected. Configure MCP servers in settings.".to_string(),
                );
            }
        };

        let resources = manager.list_all_resources(params.server.as_deref()).await;

        if resources.is_empty() {
            return ToolResult::success(
                "No resources found. MCP servers may still provide tools even if they have no resources."
                    .to_string(),
            );
        }

        let json_out = serde_json::to_string_pretty(&resources).unwrap_or_default();
        debug!(count = resources.len(), "Listed MCP resources");
        ToolResult::success(json_out)
    }
}

// ---------------------------------------------------------------------------
// ReadMcpResourceTool
// ---------------------------------------------------------------------------

pub struct ReadMcpResourceTool;

#[derive(Debug, Deserialize)]
struct ReadMcpResourceInput {
    server: String,
    uri: String,
}

#[async_trait]
impl Tool for ReadMcpResourceTool {
    fn name(&self) -> &str {
        "ReadMcpResource"
    }

    fn description(&self) -> &str {
        "Read a specific resource from an MCP server by URI. \
         Use ListMcpResources to discover available resource URIs."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "The MCP server name"
                },
                "uri": {
                    "type": "string",
                    "description": "The resource URI to read"
                }
            },
            "required": ["server", "uri"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: ReadMcpResourceInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        let manager = match &ctx.mcp_manager {
            Some(m) => m,
            None => {
                return ToolResult::error(
                    "No MCP servers connected. Configure MCP servers in settings.".to_string(),
                );
            }
        };

        debug!(server = %params.server, uri = %params.uri, "Reading MCP resource");

        match manager.read_resource(&params.server, &params.uri).await {
            Ok(contents) => {
                let json_out = serde_json::to_string_pretty(&contents).unwrap_or_default();
                ToolResult::success(json_out)
            }
            Err(e) => ToolResult::error(format!(
                "Failed to read resource '{}' from server '{}': {}",
                params.uri, params.server, e
            )),
        }
    }
}
