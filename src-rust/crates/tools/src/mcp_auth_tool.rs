// McpAuthTool: pseudo-tool surfaced for MCP servers that require OAuth.
//
// Tool name: "mcp__auth"
//
// When called by the LLM (or user) with a `server_name`, this tool:
//  1. Checks whether the server is already connected (or currently connecting).
//  2. If the server is a remote MCP server (`http` / `sse`), runs the browser-
//     based OAuth flow and stores the resulting token locally.
//  3. For stdio servers, explains env-var based authentication.
//
// This mirrors the TypeScript `mcp__<name>__authenticate` dynamic tool.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

pub struct McpAuthTool;

#[derive(Debug, Deserialize)]
struct McpAuthInput {
    server_name: String,
}

#[async_trait]
impl Tool for McpAuthTool {
    fn name(&self) -> &str {
        "mcp__auth"
    }

    fn description(&self) -> &str {
        "Start the OAuth 2.0 + PKCE authorization flow for an MCP server that \
         requires authentication. Completes the browser flow and stores the \
         resulting token locally. For stdio servers that use environment \
         variables for auth, returns setup instructions instead."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server_name": {
                    "type": "string",
                    "description": "The MCP server name that needs authentication."
                }
            },
            "required": ["server_name"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: McpAuthInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        if let Err(e) = ctx.check_permission(
            self.name(),
            &format!("Authenticate MCP server {}", params.server_name),
            false,
        ) {
            return ToolResult::error(e.to_string());
        }

        let manager = match &ctx.mcp_manager {
            Some(m) => m,
            None => {
                return ToolResult::error(
                    "No MCP manager configured. Cannot authenticate MCP servers.".to_string(),
                )
            }
        };

        use claurst_mcp::McpServerStatus;

        // 1. Check current connection status.
        match manager.server_status(&params.server_name) {
            McpServerStatus::Connected { tool_count } => {
                // Fall through to allow re-authentication even when already connected.
                tracing::debug!(
                    server = %params.server_name,
                    tool_count,
                    "McpAuthTool: server already connected; continuing with re-authentication"
                );
            }
            McpServerStatus::Connecting => {
                return ToolResult::success(format!(
                    "MCP server \"{}\" is currently connecting. Try again in a moment.",
                    params.server_name
                ));
            }
            McpServerStatus::Failed { error, .. } => {
                // Fall through to attempt auth; also report the failure.
                tracing::debug!(
                    server = %params.server_name,
                    error = %error,
                    "McpAuthTool: server failed; attempting to authenticate"
                );
            }
            McpServerStatus::Disconnected { .. } => {
                // Fall through to attempt auth.
            }
        }

        // 2. Run the full OAuth flow and persist the resulting token.
        match manager.authenticate(&params.server_name).await {
            Ok(result) => ToolResult::success(
                json!({
                    "status": "authenticated",
                    "server_name": result.server_name,
                    "auth_url": result.auth_url,
                    "redirect_uri": result.redirect_uri,
                    "token_path": result.token_path,
                    "message": format!(
                        "Completed OAuth authentication for \"{}\" and saved the token.\n\
                         Token path: {}\n\
                         You can now run /mcp connect {} or press r in the MCP panel to reconnect.",
                        result.server_name,
                        result.token_path.display(),
                        result.server_name
                    )
                })
                .to_string(),
            ),
            Err(e) => {
                // Return a descriptive error so the LLM can guide the user.
                ToolResult::error(format!(
                    "Could not complete OAuth for \"{}\": {}\n\n\
                     This may mean the server is a stdio server (uses env-var auth) \
                     or its URL is not configured. Run /mcp auth {} in the Claude \
                     interface for detailed instructions.",
                    params.server_name, e, params.server_name
                ))
            }
        }
    }
}
