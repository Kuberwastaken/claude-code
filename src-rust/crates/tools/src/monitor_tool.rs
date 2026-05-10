// monitor_tool.rs — Monitor background tasks
//
// Provides a "monitor" tool that lets the agent inspect background tasks
// started via BashTool with run_in_background=true.  Supports listing all
// tasks, checking the status or output of a specific task, and cancelling a
// running task.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use claurst_core::tasks::{global_registry, TaskStatus};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct MonitorTool;

#[derive(Deserialize)]
struct MonitorInput {
    #[serde(default)]
    action: MonitorAction,
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum MonitorAction {
    #[default]
    List,
    Status,
    Output,
    Cancel,
}

#[async_trait]
impl Tool for MonitorTool {
    fn name(&self) -> &str {
        "monitor"
    }

    fn description(&self) -> &str {
        "Monitor background tasks started with run_in_background=true. \
        List all tasks, check status, retrieve output, or cancel a running task."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "status", "output", "cancel"],
                    "description": "Action to perform. 'list' shows all tasks, 'status'/'output' inspect a specific task, 'cancel' terminates a running task.",
                    "default": "list"
                },
                "task_id": {
                    "type": "string",
                    "description": "Task ID to inspect or cancel. Required for status, output, cancel actions."
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let parsed: MonitorInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        match parsed.action {
            MonitorAction::List => {
                let tasks = global_registry().list();
                if tasks.is_empty() {
                    return ToolResult::success("No background tasks.");
                }
                let mut lines = vec!["Background tasks:".to_string()];
                for t in &tasks {
                    let status = match &t.status {
                        TaskStatus::Running => "running".to_string(),
                        TaskStatus::Completed => "completed".to_string(),
                        TaskStatus::Failed(msg) => format!("failed: {}", msg),
                        TaskStatus::Cancelled => "cancelled".to_string(),
                    };
                    lines.push(format!("  {} [{}] {}", t.id, status, t.name));
                }
                ToolResult::success(lines.join("\n"))
            }

            MonitorAction::Status => {
                let id = match parsed.task_id {
                    Some(id) => id,
                    None => return ToolResult::error("task_id required for status action"),
                };
                match global_registry().get(&id) {
                    None => ToolResult::error(format!("Task {} not found", id)),
                    Some(t) => {
                        let status = match &t.status {
                            TaskStatus::Running => "running".to_string(),
                            TaskStatus::Completed => "completed (exit 0)".to_string(),
                            TaskStatus::Failed(msg) => format!("failed: {}", msg),
                            TaskStatus::Cancelled => "cancelled".to_string(),
                        };
                        ToolResult::success(format!(
                            "Task: {}\nStatus: {}\nCommand: {}\nOutput lines: {}",
                            t.id,
                            status,
                            t.name,
                            t.output.len()
                        ))
                    }
                }
            }

            MonitorAction::Output => {
                let id = match parsed.task_id {
                    Some(id) => id,
                    None => return ToolResult::error("task_id required for output action"),
                };
                match global_registry().get(&id) {
                    None => ToolResult::error(format!("Task {} not found", id)),
                    Some(t) => {
                        let output = t.output.join("\n");
                        if output.is_empty() {
                            ToolResult::success("(no output yet)")
                        } else {
                            ToolResult::success(output)
                        }
                    }
                }
            }

            MonitorAction::Cancel => {
                let id = match parsed.task_id {
                    Some(id) => id,
                    None => return ToolResult::error("task_id required for cancel action"),
                };
                match global_registry().get(&id) {
                    None => ToolResult::error(format!("Task {} not found", id)),
                    Some(t) => {
                        if let TaskStatus::Running = t.status {
                            // Kill by PID if available.
                            if let Some(pid) = t.pid {
                                // On Windows use taskkill; on Unix send SIGTERM.
                                #[cfg(windows)]
                                {
                                    let _ = std::process::Command::new("taskkill")
                                        .args(["/PID", &pid.to_string(), "/F"])
                                        .output();
                                }
                                #[cfg(unix)]
                                {
                                    use std::process::Command;
                                    let _ = Command::new("kill")
                                        .args(["-TERM", &pid.to_string()])
                                        .output();
                                }
                            }
                            global_registry().update_status(&id, TaskStatus::Cancelled);
                            ToolResult::success(format!("Task {} cancelled.", id))
                        } else {
                            ToolResult::error(format!(
                                "Task {} is not running (status: {})",
                                id, t.status
                            ))
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_tool_name() {
        assert_eq!(MonitorTool.name(), "monitor");
    }

    #[test]
    fn monitor_schema_has_action_and_task_id() {
        let schema = MonitorTool.input_schema();
        let props = &schema["properties"];
        assert!(props["action"].is_object(), "schema should have 'action' property");
        assert!(props["task_id"].is_object(), "schema should have 'task_id' property");
    }

    #[test]
    fn monitor_schema_is_object() {
        let schema = MonitorTool.input_schema();
        assert!(schema.is_object());
        assert!(schema.get("properties").is_some());
    }

    #[tokio::test]
    async fn monitor_list_empty() {
        // The global registry is shared across tests, so we just verify the
        // tool runs without panicking and returns a success result.
        let tool = MonitorTool;
        let input = json!({"action": "list"});
        let ctx = make_test_ctx();
        let result = tool.execute(input, &ctx).await;
        // Either "No background tasks." or a list — both are successes.
        assert!(!result.is_error, "list action should not return an error: {}", result.content);
    }

    #[tokio::test]
    async fn monitor_status_missing_task_id() {
        let tool = MonitorTool;
        let input = json!({"action": "status"});
        let ctx = make_test_ctx();
        let result = tool.execute(input, &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("task_id required"));
    }

    #[tokio::test]
    async fn monitor_output_unknown_task() {
        let tool = MonitorTool;
        let input = json!({"action": "output", "task_id": "nonexistent-uuid-1234"});
        let ctx = make_test_ctx();
        let result = tool.execute(input, &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn monitor_cancel_unknown_task() {
        let tool = MonitorTool;
        let input = json!({"action": "cancel", "task_id": "nonexistent-uuid-5678"});
        let ctx = make_test_ctx();
        let result = tool.execute(input, &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    fn make_test_ctx() -> ToolContext {
        use claurst_core::config::Config;
        use claurst_core::permissions::AutoPermissionHandler;
        use std::path::PathBuf;
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;

        let handler = Arc::new(AutoPermissionHandler {
            mode: claurst_core::config::PermissionMode::Default,
        });
        ToolContext {
            working_dir: PathBuf::from("."),
            permission_mode: claurst_core::config::PermissionMode::Default,
            permission_handler: handler,
            cost_tracker: claurst_core::cost::CostTracker::new(),
            session_id: "test-monitor".to_string(),
            file_history: Arc::new(parking_lot::Mutex::new(
                claurst_core::file_history::FileHistory::new(),
            )),
            current_turn: Arc::new(AtomicUsize::new(0)),
            non_interactive: true,
            mcp_manager: None,
            config: Config::default(),
            managed_agent_config: None,
            completion_notifier: None,
            pending_permissions: None,
            permission_manager: None,
            user_question_tx: None,
        }
    }
}
