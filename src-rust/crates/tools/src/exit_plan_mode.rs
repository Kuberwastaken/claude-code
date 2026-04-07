// ExitPlanMode tool: leave planning mode and return to normal execution.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::debug;

pub struct ExitPlanModeTool;

#[derive(Debug, Deserialize)]
struct ExitPlanModeInput {
    #[serde(default)]
    summary: Option<String>,
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str {
        claurst_core::constants::TOOL_NAME_EXIT_PLAN_MODE
    }

    fn description(&self) -> &str {
        "Exit plan mode and return to normal execution mode where all tools \
         are available. Optionally provide a summary of the plan."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Summary of the plan you developed"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let params: ExitPlanModeInput =
            serde_json::from_value(input).unwrap_or(ExitPlanModeInput { summary: None });

        debug!(summary = ?params.summary, "Exiting plan mode");

        let msg = if let Some(summary) = &params.summary {
            format!("Exited plan mode. Plan summary: {}", summary)
        } else {
            "Exited plan mode. All tools are now available.".to_string()
        };

        ToolResult::success(msg).with_metadata(json!({
            "type": "exit_plan_mode",
            "summary": params.summary,
        }))
    }
}
