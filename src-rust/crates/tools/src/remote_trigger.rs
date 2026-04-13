//! RemoteTriggerTool — cross-session event dispatch.
//! Mirrors src/tools/RemoteTriggerTool/.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

/// Input schema for RemoteTriggerTool.
#[derive(Debug, Deserialize)]
struct RemoteTriggerInput {
    /// Target session ID to send the event to.
    session_id: String,
    /// Event name (arbitrary string).
    event_name: String,
    /// JSON payload to deliver.
    #[serde(default)]
    payload: Value,
}

/// Delivers cross-session trigger events via the Claude.ai API.
pub struct RemoteTriggerTool;

#[async_trait]
impl Tool for RemoteTriggerTool {
    fn name(&self) -> &str {
        "RemoteTrigger"
    }

    fn description(&self) -> &str {
        "Send a named event to another active Claurst session. \
         Use this to coordinate across parallel sessions or notify a parent session of results."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "The target session ID to trigger"
                },
                "event_name": {
                    "type": "string",
                    "description": "Name of the event to send (e.g., 'task_complete', 'result_ready')"
                },
                "payload": {
                    "type": "object",
                    "description": "Optional JSON payload to deliver with the event",
                    "additionalProperties": true
                }
            },
            "required": ["session_id", "event_name"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: RemoteTriggerInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {e}")),
        };

        // Auth token is not available via a sync helper; pass empty string.
        // A future implementation can wire in a proper OAuth token retrieval.
        let token = String::new();

        let client = reqwest::Client::new();
        let url = format!(
            "https://api.claude.ai/api/sessions/{}/trigger",
            params.session_id
        );

        let body = json!({
            "event_name": params.event_name,
            "payload": params.payload,
            "source_session_id": ctx.session_id,
        });

        let mut request = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body);

        if !token.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        let resp = match request.send().await {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("HTTP error: {e}")),
        };

        let target_prefix = &params.session_id[..params.session_id.len().min(8)];

        if resp.status().is_success() {
            match resp.json::<Value>().await {
                Ok(data) => {
                    let delivered = data["delivered"].as_bool().unwrap_or(false);
                    let status = data["session_status"].as_str().unwrap_or("unknown");
                    ToolResult::success(format!(
                        "Event '{}' {} to session {} (status: {})",
                        params.event_name,
                        if delivered { "delivered" } else { "queued" },
                        target_prefix,
                        status,
                    ))
                }
                Err(_) => ToolResult::success(format!(
                    "Event '{}' sent to session {}",
                    params.event_name, target_prefix,
                )),
            }
        } else {
            ToolResult::error(format!(
                "Trigger failed: HTTP {} — is session {} active?",
                resp.status(),
                target_prefix,
            ))
        }
    }
}
