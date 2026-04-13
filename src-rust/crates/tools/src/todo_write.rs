// TodoWrite tool: task / todo list management.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use tracing::debug;

// ---------------------------------------------------------------------------
// Session-aware persistence helpers
// ---------------------------------------------------------------------------

/// Returns the path to the persisted todo list for `session_id`.
pub fn todos_path(session_id: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".claurst")
        .join("todos")
        .join(format!("{}.json", session_id))
}

/// Load the persisted todo list for `session_id`. Returns an empty vec if the
/// file does not exist or cannot be parsed.
pub fn load_todos(session_id: &str) -> Vec<Value> {
    let path = todos_path(session_id);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
        .unwrap_or_default()
}

/// Persist `todos` to `~/.claurst/todos/<session_id>.json`.
pub fn save_todos(session_id: &str, todos: &[Value]) {
    let path = todos_path(session_id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(serialized) = serde_json::to_string_pretty(todos) {
        let _ = std::fs::write(&path, serialized);
    }
}

// ---------------------------------------------------------------------------
// Status enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    /// Parse case-insensitively from a string.
    fn from_str_ci(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(TodoStatus::Pending),
            "in_progress" => Ok(TodoStatus::InProgress),
            "completed" => Ok(TodoStatus::Completed),
            other => Err(format!(
                "Invalid status {:?}: must be one of \"pending\", \"in_progress\", or \"completed\".",
                other
            )),
        }
    }
}

impl<'de> serde::Deserialize<'de> for TodoStatus {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        TodoStatus::from_str_ci(&s).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
        }
    }
}

// ---------------------------------------------------------------------------
// Input types
// ---------------------------------------------------------------------------

pub struct TodoWriteTool;

#[derive(Debug, Deserialize)]
struct TodoWriteInput {
    todos: Vec<TodoItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct TodoItem {
    id: String,
    content: String,
    status: TodoStatus,
    #[serde(default)]
    #[allow(dead_code)]
    priority: Option<String>,
}

// ---------------------------------------------------------------------------
// Transition validation
// ---------------------------------------------------------------------------

/// Check that a status transition from `old` to `new` is permitted.
///
/// Allowed:
///   pending     → in_progress   ✓
///   pending     → completed     ✓  (direct completion)
///   in_progress → completed     ✓
///
/// Forbidden:
///   completed   → anything      ✗  (completed tasks cannot be reopened)
///   in_progress → pending       ✗  (cannot move backwards)
fn validate_transition(id: &str, old: &TodoStatus, new: &TodoStatus) -> Result<(), String> {
    if old == new {
        return Ok(());
    }
    match (old, new) {
        // Completed tasks are immutable.
        (TodoStatus::Completed, _) => Err(format!(
            "Task {:?}: cannot change status of a completed task (currently \"completed\" → \"{}\").",
            id, new
        )),
        // Cannot move in_progress backwards to pending.
        (TodoStatus::InProgress, TodoStatus::Pending) => Err(format!(
            "Task {:?}: cannot move status backwards (\"in_progress\" → \"pending\").",
            id
        )),
        // All other transitions (pending→in_progress, pending→completed,
        // in_progress→completed) are valid.
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        claurst_core::constants::TOOL_NAME_TODO_WRITE
    }

    fn description(&self) -> &str {
        "Write and manage a todo/task list. Provide the complete list of todos \
         each time (this replaces the entire list). Use this to track progress \
         on multi-step tasks."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "content": { "type": "string" },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"]
                            },
                            "priority": { "type": "string" }
                        },
                        "required": ["id", "content", "status"]
                    },
                    "description": "The complete list of todo items"
                }
            },
            "required": ["todos"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        // --- 1. Deserialize & validate statuses (case-insensitive) ----------
        let params: TodoWriteInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        debug!(count = params.todos.len(), "Writing todo list");

        // --- 2. Task ID uniqueness check ------------------------------------
        // IDs must be unique within the incoming list itself.
        {
            let mut seen_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for item in &params.todos {
                if !seen_ids.insert(item.id.as_str()) {
                    return ToolResult::error(format!(
                        "Duplicate task ID {:?} in the provided list. IDs must be unique.",
                        item.id
                    ));
                }
            }
        }

        // --- 3. Load persisted state & enforce status-transition rules -------
        let persisted = load_todos(&ctx.session_id);

        // Build a map of existing id → status from the persisted list.
        let existing: std::collections::HashMap<&str, TodoStatus> = persisted
            .iter()
            .filter_map(|v| {
                let id = v.get("id")?.as_str()?;
                let raw = v.get("status")?.as_str()?;
                TodoStatus::from_str_ci(raw).ok().map(|s| (id, s))
            })
            .collect();

        // Collect the set of IDs that were newly completed in *this* call,
        // so we can craft accurate nudge messaging.
        let mut newly_completed_ids: std::collections::HashSet<&str> =
            std::collections::HashSet::new();

        for item in &params.todos {
            match existing.get(item.id.as_str()) {
                Some(old_status) => {
                    // Existing task — validate the transition.
                    if let Err(e) = validate_transition(&item.id, old_status, &item.status) {
                        return ToolResult::error(e);
                    }
                    if old_status != &TodoStatus::Completed && item.status == TodoStatus::Completed
                    {
                        newly_completed_ids.insert(&item.id);
                    }
                }
                None => {
                    // New task — IDs must not collide with persisted ones.
                    // (They aren't in the map, so no collision; nothing extra to check.)
                }
            }
        }

        // --- 4. Counts -------------------------------------------------------
        let total = params.todos.len();
        let completed = params
            .todos
            .iter()
            .filter(|t| t.status == TodoStatus::Completed)
            .count();
        let in_progress = params
            .todos
            .iter()
            .filter(|t| t.status == TodoStatus::InProgress)
            .count();
        let pending = total - completed - in_progress;

        // --- 5. Build human-readable output ----------------------------------
        let mut output = format!(
            "Todo list updated ({} total: {} pending, {} in progress, {} completed)\n\n",
            total, pending, in_progress, completed
        );

        for item in &params.todos {
            let icon = match item.status {
                TodoStatus::Pending => "[ ]",
                TodoStatus::InProgress => "[~]",
                TodoStatus::Completed => "[x]",
            };
            output.push_str(&format!("{} {} ({})\n", icon, item.content, item.id));
        }

        // --- 6. Persist to disk ----------------------------------------------
        let todos_json: Vec<Value> = params
            .todos
            .iter()
            .map(|t| {
                let mut obj = json!({
                    "id": t.id,
                    "content": t.content,
                    "status": t.status.to_string(),
                });
                if let Some(ref p) = t.priority {
                    obj["priority"] = json!(p);
                }
                obj
            })
            .collect();
        save_todos(&ctx.session_id, &todos_json);

        // --- 7. Session-end verification message / completion nudge ----------
        if total == 0 || (pending == 0 && in_progress == 0) {
            // All tasks completed (or the list was cleared).
            if total > 0 {
                output.push_str(
                    "\n\nAll tasks completed! Great work — the session todo list is fully done.",
                );
            }
        } else {
            // Some tasks remain.

            // In-progress nudge: fire only when there are in_progress tasks
            // that were NOT just completed in this very call.
            if in_progress > 0 {
                output.push_str(&format!(
                    "\n\nReminder: {} task{} are in_progress — complete them before marking the session done.",
                    in_progress,
                    if in_progress == 1 { "" } else { "s" }
                ));
            }

            // General incomplete warning.
            let incomplete = pending + in_progress;
            output.push_str(&format!(
                "\n\nWARNING: {} task{} still incomplete. Continue working on them.",
                incomplete,
                if incomplete == 1 { " is" } else { "s are" }
            ));
        }

        ToolResult::success(output).with_metadata(json!({
            "total": total,
            "completed": completed,
            "in_progress": in_progress,
            "pending": pending,
        }))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_todos_path_contains_session_id() {
        let path = todos_path("my-session-123");
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains("my-session-123"),
            "todos_path should embed the session id"
        );
        assert!(
            path_str.contains(".claurst"),
            "todos_path should be under ~/.claurst"
        );
        assert!(
            path_str.ends_with(".json"),
            "todos_path should end with .json"
        );
    }

    #[test]
    fn test_load_todos_missing_file_returns_empty() {
        let todos = load_todos("nonexistent-session-zzzzzz-99999");
        assert!(todos.is_empty(), "Missing file should yield empty vec");
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let session_id = format!(
            "test-session-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let todos = vec![
            json!({"id": "1", "content": "Task one", "status": "pending"}),
            json!({"id": "2", "content": "Task two", "status": "completed"}),
        ];
        save_todos(&session_id, &todos);
        let loaded = load_todos(&session_id);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0]["id"].as_str(), Some("1"));
        assert_eq!(loaded[1]["status"].as_str(), Some("completed"));
        // Clean up.
        let _ = std::fs::remove_file(todos_path(&session_id));
    }

    // --- Status parsing ------------------------------------------------------

    #[test]
    fn test_status_parsing_case_insensitive() {
        assert_eq!(
            TodoStatus::from_str_ci("PENDING").unwrap(),
            TodoStatus::Pending
        );
        assert_eq!(
            TodoStatus::from_str_ci("In_Progress").unwrap(),
            TodoStatus::InProgress
        );
        assert_eq!(
            TodoStatus::from_str_ci("COMPLETED").unwrap(),
            TodoStatus::Completed
        );
        assert!(TodoStatus::from_str_ci("done").is_err());
        assert!(TodoStatus::from_str_ci("").is_err());
    }

    #[test]
    fn test_status_display() {
        assert_eq!(TodoStatus::Pending.to_string(), "pending");
        assert_eq!(TodoStatus::InProgress.to_string(), "in_progress");
        assert_eq!(TodoStatus::Completed.to_string(), "completed");
    }

    // --- Transition rules ----------------------------------------------------

    #[test]
    fn test_valid_transitions() {
        // pending → in_progress
        assert!(validate_transition("t1", &TodoStatus::Pending, &TodoStatus::InProgress).is_ok());
        // pending → completed
        assert!(validate_transition("t2", &TodoStatus::Pending, &TodoStatus::Completed).is_ok());
        // in_progress → completed
        assert!(validate_transition("t3", &TodoStatus::InProgress, &TodoStatus::Completed).is_ok());
        // no-op transitions are always fine
        assert!(validate_transition("t4", &TodoStatus::Pending, &TodoStatus::Pending).is_ok());
        assert!(
            validate_transition("t5", &TodoStatus::InProgress, &TodoStatus::InProgress).is_ok()
        );
        assert!(validate_transition("t6", &TodoStatus::Completed, &TodoStatus::Completed).is_ok());
    }

    #[test]
    fn test_invalid_transition_completed_to_anything() {
        assert!(validate_transition("t1", &TodoStatus::Completed, &TodoStatus::Pending).is_err());
        assert!(
            validate_transition("t2", &TodoStatus::Completed, &TodoStatus::InProgress).is_err()
        );
    }

    #[test]
    fn test_invalid_transition_in_progress_to_pending() {
        assert!(validate_transition("t1", &TodoStatus::InProgress, &TodoStatus::Pending).is_err());
    }

    // --- ID uniqueness -------------------------------------------------------

    #[test]
    fn test_status_from_str_invalid() {
        let err = TodoStatus::from_str_ci("banana").unwrap_err();
        assert!(
            err.contains("Invalid status"),
            "error should mention invalid status"
        );
        assert!(err.contains("banana"), "error should echo the bad value");
    }
}
