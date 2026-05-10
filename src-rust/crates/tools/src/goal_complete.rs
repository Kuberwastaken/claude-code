// GoalCompleteTool — marks the active goal as complete.
//
// This is the tool the model calls after passing a self-audit that verifies
// the goal objective has been fully achieved.  Calling it without a thorough
// audit_summary + evidence is considered a violation of the goal contract.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

pub struct GoalCompleteTool;

#[derive(Debug, Deserialize)]
struct GoalCompleteInput {
    /// A concise summary of what was accomplished (the audit).
    audit_summary: String,
    /// Concrete evidence: test output, file diffs, command results, etc.
    evidence: String,
}

#[async_trait]
impl Tool for GoalCompleteTool {
    fn name(&self) -> &str { "GoalComplete" }

    fn description(&self) -> &str {
        "Mark the active goal as complete. ONLY call this after a genuine completion audit:\n\
         1. Restate the goal as concrete deliverables.\n\
         2. Check each deliverable against real output, test results, or file diffs.\n\
         3. Confirm all deliverables are satisfied.\n\
         Calling this without a real audit is a goal contract violation."
    }

    fn permission_level(&self) -> PermissionLevel { PermissionLevel::None }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "audit_summary": {
                    "type": "string",
                    "description": "Concise summary of what was accomplished and verified"
                },
                "evidence": {
                    "type": "string",
                    "description": "Concrete evidence of completion: test output, diffs, command results"
                }
            },
            "required": ["audit_summary", "evidence"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: GoalCompleteInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        if params.audit_summary.trim().is_empty() {
            return ToolResult::error(
                "audit_summary cannot be empty. Provide a concise description of what was completed."
                    .to_string(),
            );
        }
        if params.evidence.trim().is_empty() {
            return ToolResult::error(
                "evidence cannot be empty. Provide test output, diffs, or command results."
                    .to_string(),
            );
        }

        let session_id = &ctx.session_id;

        match claurst_core::GoalStore::open_default() {
            None => ToolResult::error("Could not open goal store.".to_string()),
            Some(store) => match store.set_status(session_id, claurst_core::GoalStatus::Complete) {
                Ok(()) => ToolResult::success(format!(
                    "Goal marked complete.\n\nAudit summary: {}\n\nEvidence: {}",
                    params.audit_summary, params.evidence,
                )),
                Err(e) => ToolResult::error(format!(
                    "Failed to mark goal complete: {}. \
                     There may be no active goal for this session.",
                    e
                )),
            },
        }
    }
}
