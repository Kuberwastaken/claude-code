// Cron tools: schedule recurring and one-shot prompts.
//
// CronCreateTool  – create a new scheduled task (cron expression)
// CronDeleteTool  – remove an existing scheduled task
// CronListTool    – list all scheduled tasks
//
// Scheduled tasks are stored in a global in-memory store.
// Durable tasks are persisted to `~/.claurst/scheduled_tasks.json`.
//
// On first use the store is initialised from the JSON file; tasks older than
// 7 days are automatically purged on load (matching TypeScript behaviour).
//
// Cron expression format: "M H DoM Mon DoW" (standard 5-field cron in local
// time). For example:
//   "*/5 * * * *"   = every 5 minutes
//   "30 14 * * 1"   = every Monday at 14:30

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Local, Timelike};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// In-memory store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronTask {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub durable: bool,
    pub created_at: u64,
}

/// 7 days in seconds — tasks older than this are purged on load.
const MAX_TASK_AGE_SECS: u64 = 7 * 24 * 3600;

/// Whether the store has been initialised from disk for this process.
static STORE_INITIALISED: once_cell::sync::Lazy<tokio::sync::Mutex<bool>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(false));

static CRON_STORE: Lazy<Arc<RwLock<HashMap<String, CronTask>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));

// ---------------------------------------------------------------------------
// Disk path helpers
// ---------------------------------------------------------------------------

/// Path to `~/.claurst/scheduled_tasks.json`.
fn scheduled_tasks_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claurst").join("scheduled_tasks.json"))
}

/// Ensure the store has been loaded from disk (once per process).
async fn ensure_store_loaded() {
    let mut init = STORE_INITIALISED.lock().await;
    if *init {
        return;
    }
    *init = true;

    // Load from ~/.claurst/scheduled_tasks.json if it exists.
    let path = match scheduled_tasks_path() {
        Some(p) => p,
        None => return,
    };

    let data = match tokio::fs::read_to_string(&path).await {
        Ok(d) => d,
        Err(_) => return, // file doesn't exist yet — that's fine
    };

    let tasks: Vec<CronTask> = match serde_json::from_str(&data) {
        Ok(t) => t,
        Err(e) => {
            debug!("Failed to parse scheduled_tasks.json: {}", e);
            return;
        }
    };

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut store = CRON_STORE.write().await;
    for task in tasks {
        // Drop tasks older than MAX_TASK_AGE_SECS
        if now_secs.saturating_sub(task.created_at) > MAX_TASK_AGE_SECS {
            debug!("Cron task {} expired, skipping on load", task.id);
            continue;
        }
        store.insert(task.id.clone(), task);
    }
}

// ---------------------------------------------------------------------------
// Public scheduler API (used by cc-query cron_scheduler)
// ---------------------------------------------------------------------------

/// Check if a cron expression fires at the given minute-resolution datetime.
pub fn cron_matches(expr: &str, dt: &DateTime<Local>) -> bool {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }
    let minute = dt.minute();
    let hour = dt.hour();
    let day = dt.day();
    let month = dt.month();
    let dow = dt.weekday().num_days_from_sunday(); // 0=Sun .. 6=Sat

    cron_field_matches(fields[0], minute)
        && cron_field_matches(fields[1], hour)
        && cron_field_matches(fields[2], day)
        && cron_field_matches(fields[3], month)
        && cron_field_matches(fields[4], dow)
}

fn cron_field_matches(field: &str, value: u32) -> bool {
    if field == "*" {
        return true;
    }
    // */N step
    if let Some(step_str) = field.strip_prefix("*/") {
        if let Ok(step) = step_str.parse::<u32>() {
            return step > 0 && value % step == 0;
        }
    }
    // Comma-separated list of values or ranges
    for part in field.split(',') {
        if cron_range_matches(part, value) {
            return true;
        }
    }
    false
}

fn cron_range_matches(part: &str, value: u32) -> bool {
    if let Some(dash) = part.find('-') {
        let lo: u32 = part[..dash].parse().unwrap_or(u32::MAX);
        let hi: u32 = part[dash + 1..].parse().unwrap_or(0);
        value >= lo && value <= hi
    } else {
        part.parse::<u32>()
            .map_or(false, |n| n == value || (n == 7 && value == 0)) // 7 = Sunday alias
    }
}

/// Return all tasks whose cron expression fires at `dt`.
/// One-shot tasks (recurring=false) are removed from the store after being returned.
pub async fn pop_due_tasks(dt: &DateTime<Local>) -> Vec<CronTask> {
    ensure_store_loaded().await;
    let mut store = CRON_STORE.write().await;
    let due: Vec<CronTask> = store
        .values()
        .filter(|t| cron_matches(&t.cron, dt))
        .cloned()
        .collect();
    for t in &due {
        if !t.recurring {
            store.remove(&t.id);
        }
    }
    due
}

// ---------------------------------------------------------------------------
// Simple cron expression parser (5-field)
// ---------------------------------------------------------------------------

/// Validate that a 5-field cron expression is syntactically correct.
fn validate_cron(expr: &str) -> bool {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }
    // Check each field: ranges for M(0-59), H(0-23), DoM(1-31), Mon(1-12), DoW(0-7)
    let ranges = [(0u32, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
    for (i, field) in fields.iter().enumerate() {
        if *field == "*" {
            continue;
        }
        // Handle */N (step)
        if let Some(step) = field.strip_prefix("*/") {
            if step.parse::<u32>().is_err() {
                return false;
            }
            continue;
        }
        // Handle N-M (range) or N
        let parts: Vec<&str> = field.split('-').collect();
        for part in &parts {
            match part.parse::<u32>() {
                Ok(n) => {
                    if n < ranges[i].0 || n > ranges[i].1 {
                        return false;
                    }
                }
                Err(_) => return false,
            }
        }
    }
    true
}

/// Convert a cron expression to a human-readable description.
fn cron_to_human(expr: &str) -> String {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return expr.to_string();
    }

    let (minute, hour, dom, month, dow) = (fields[0], fields[1], fields[2], fields[3], fields[4]);

    if expr == "* * * * *" {
        return "every minute".to_string();
    }
    if minute.starts_with("*/") {
        let n = &minute[2..];
        return format!("every {} minutes", n);
    }
    if hour == "*" && dom == "*" && month == "*" && dow == "*" {
        return format!("at minute {} of every hour", minute);
    }
    if dom == "*" && month == "*" && dow == "*" {
        return format!("daily at {:0>2}:{:0>2}", hour, minute);
    }
    // Fallback: return the raw expression
    format!("cron({})", expr)
}

// ---------------------------------------------------------------------------
// CronCreate
// ---------------------------------------------------------------------------

pub struct CronCreateTool;

#[derive(Debug, Deserialize)]
struct CronCreateInput {
    cron: String,
    prompt: String,
    #[serde(default = "default_true")]
    recurring: bool,
    #[serde(default)]
    durable: bool,
}

fn default_true() -> bool {
    true
}

#[async_trait]
impl Tool for CronCreateTool {
    fn name(&self) -> &str {
        "CronCreate"
    }

    fn description(&self) -> &str {
        "Schedule a recurring or one-shot prompt using a standard 5-field cron expression \
         in local time: \"M H DoM Mon DoW\". Examples:\n\
         - \"*/5 * * * *\" = every 5 minutes\n\
         - \"30 14 * * 1\" = every Monday at 14:30\n\
         - \"0 9 15 * *\" = 15th of each month at 09:00\n\
         Use recurring=false for one-shot (fires once then auto-deletes).\n\
         Use durable=true to persist across sessions."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cron": {
                    "type": "string",
                    "description": "5-field cron expression: M H DoM Mon DoW"
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt to run at each scheduled time"
                },
                "recurring": {
                    "type": "boolean",
                    "description": "true (default) = repeat on every match; false = fire once then delete"
                },
                "durable": {
                    "type": "boolean",
                    "description": "true = persist to .claurst/scheduled_tasks.json; false (default) = session only"
                }
            },
            "required": ["cron", "prompt"]
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let params: CronCreateInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        if !validate_cron(&params.cron) {
            return ToolResult::error(format!(
                "Invalid cron expression '{}'. Expected 5 fields: M H DoM Mon DoW.",
                params.cron
            ));
        }

        // Ensure persistent tasks are loaded from disk before we check the count.
        ensure_store_loaded().await;

        let mut store = CRON_STORE.write().await;
        if store.len() >= 50 {
            return ToolResult::error(
                "Too many scheduled jobs (max 50). Cancel one first.".to_string(),
            );
        }

        let id = Uuid::new_v4().to_string()[..8].to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let task = CronTask {
            id: id.clone(),
            cron: params.cron.clone(),
            prompt: params.prompt.clone(),
            recurring: params.recurring,
            durable: params.durable,
            created_at: now,
        };

        store.insert(id.clone(), task);

        // Persist to ~/.claurst/scheduled_tasks.json for durable tasks.
        if params.durable {
            if let Err(e) = persist_tasks_to_disk(&store).await {
                debug!("Failed to persist cron task to disk: {}", e);
            }
        }

        let human = cron_to_human(&params.cron);

        let where_note = if params.durable {
            "Persisted to ~/.claurst/scheduled_tasks.json"
        } else {
            "Session-only (dies when Claude exits)"
        };

        let msg = if params.recurring {
            format!("Scheduled recurring job {} ({}). {}", id, human, where_note)
        } else {
            format!(
                "Scheduled one-shot task {} ({}). {}. Will fire once then auto-delete.",
                id, human, where_note
            )
        };

        ToolResult::success(msg)
    }
}

// ---------------------------------------------------------------------------
// CronDelete
// ---------------------------------------------------------------------------

pub struct CronDeleteTool;

#[derive(Debug, Deserialize)]
struct CronDeleteInput {
    id: String,
}

#[async_trait]
impl Tool for CronDeleteTool {
    fn name(&self) -> &str {
        "CronDelete"
    }

    fn description(&self) -> &str {
        "Cancel a scheduled cron task by its ID. Use CronList to find the ID."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The cron task ID to delete"
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let params: CronDeleteInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        // Load disk state first so we don't accidentally drop persisted tasks.
        ensure_store_loaded().await;

        let mut store = CRON_STORE.write().await;
        if let Some(removed) = store.remove(&params.id) {
            // If it was durable, update the file.
            if removed.durable {
                if let Err(e) = persist_tasks_to_disk(&store).await {
                    debug!("Failed to update scheduled_tasks.json after delete: {}", e);
                }
            }
            ToolResult::success(format!("Deleted cron task '{}'.", params.id))
        } else {
            ToolResult::error(format!("Cron task '{}' not found.", params.id))
        }
    }
}

// ---------------------------------------------------------------------------
// CronList
// ---------------------------------------------------------------------------

pub struct CronListTool;

#[async_trait]
impl Tool for CronListTool {
    fn name(&self) -> &str {
        "CronList"
    }

    fn description(&self) -> &str {
        "List all currently scheduled cron tasks."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _input: Value, _ctx: &ToolContext) -> ToolResult {
        // Merge in-memory store with any persisted tasks from disk.
        ensure_store_loaded().await;

        let store = CRON_STORE.read().await;

        if store.is_empty() {
            return ToolResult::success("No scheduled cron tasks.".to_string());
        }

        let mut tasks: Vec<&CronTask> = store.values().collect();
        tasks.sort_by_key(|t| t.created_at);

        let lines: Vec<String> = tasks
            .iter()
            .map(|t| {
                format!(
                    "{} | {} | {} | recurring={} | durable={} | prompt: {}",
                    t.id,
                    t.cron,
                    cron_to_human(&t.cron),
                    t.recurring,
                    t.durable,
                    if t.prompt.len() > 60 {
                        format!("{}…", &t.prompt[..60])
                    } else {
                        t.prompt.clone()
                    }
                )
            })
            .collect();

        ToolResult::success(format!(
            "Scheduled tasks ({}):\n\n{}",
            tasks.len(),
            lines.join("\n")
        ))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Persist all durable tasks to `~/.claurst/scheduled_tasks.json`.
async fn persist_tasks_to_disk(store: &HashMap<String, CronTask>) -> Result<(), String> {
    let durable: Vec<&CronTask> = store.values().filter(|t| t.durable).collect();
    let json = serde_json::to_string_pretty(&durable).map_err(|e| e.to_string())?;

    let path =
        scheduled_tasks_path().ok_or_else(|| "Cannot determine home directory".to_string())?;
    let dir = path.parent().ok_or("No parent directory")?;

    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| e.to_string())?;

    tokio::fs::write(&path, json)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}
