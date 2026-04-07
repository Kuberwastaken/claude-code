// claurst-tools: All tool implementations for Claurst.
//
// Each tool maps to a capability the LLM can invoke: running shell commands,
// reading/writing/editing files, searching codebases, fetching web pages, etc.

use async_trait::async_trait;
use claurst_core::config::PermissionMode;
use claurst_core::cost::CostTracker;
use claurst_core::permissions::{PermissionDecision, PermissionHandler, PermissionRequest};
use claurst_core::types::ToolDefinition;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// Sub-modules – each contains a full tool implementation.
pub mod apply_patch;
pub mod ask_user;
pub mod bash;
pub mod batch_edit;
pub mod brief;
pub mod bundled_skills;
pub mod computer_use;
pub mod config_tool;
pub mod cron;
pub mod enter_plan_mode;
pub mod exit_plan_mode;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod formatter;
pub mod glob_tool;
pub mod grep_tool;
pub mod lsp_tool;
pub mod mcp_auth_tool;
pub mod mcp_resources;
pub mod notebook_edit;
pub mod powershell;
pub mod pty_bash;
pub mod remote_trigger;
pub mod repl_tool;
pub mod send_message;
pub mod skill_tool;
pub mod sleep;
pub mod synthetic_output;
pub mod tasks;
pub mod team_tool;
pub mod todo_write;
pub mod tool_search;
pub mod web_fetch;
pub mod web_search;
pub mod worktree;

// Re-exports for convenience.
pub use apply_patch::ApplyPatchTool;
pub use ask_user::AskUserQuestionTool;
pub use bash::BashTool;
pub use batch_edit::BatchEditTool;
pub use brief::BriefTool;
pub use computer_use::ComputerUseTool;
pub use config_tool::ConfigTool;
pub use cron::{CronCreateTool, CronDeleteTool, CronListTool};
pub use enter_plan_mode::EnterPlanModeTool;
pub use exit_plan_mode::ExitPlanModeTool;
pub use file_edit::FileEditTool;
pub use file_read::FileReadTool;
pub use file_write::FileWriteTool;
pub use formatter::try_format_file;
pub use glob_tool::GlobTool;
pub use grep_tool::GrepTool;
pub use lsp_tool::LspTool;
pub use mcp_auth_tool::McpAuthTool;
pub use mcp_resources::{ListMcpResourcesTool, ReadMcpResourceTool};
pub use notebook_edit::NotebookEditTool;
pub use powershell::PowerShellTool;
pub use pty_bash::PtyBashTool;
pub use remote_trigger::RemoteTriggerTool;
pub use repl_tool::ReplTool;
pub use send_message::{drain_inbox, peek_inbox, SendMessageTool};
pub use skill_tool::SkillTool;
pub use sleep::SleepTool;
pub use synthetic_output::SyntheticOutputTool;
pub use tasks::{
    Task, TaskCreateTool, TaskGetTool, TaskListTool, TaskOutputTool, TaskStatus, TaskStopTool,
    TaskUpdateTool, TASK_STORE,
};
pub use team_tool::{register_agent_runner, AgentRunFn, TeamCreateTool, TeamDeleteTool};
pub use todo_write::TodoWriteTool;
pub use tool_search::ToolSearchTool;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
pub use worktree::{EnterWorktreeTool, ExitWorktreeTool};

// ---------------------------------------------------------------------------
// Core trait & types
// ---------------------------------------------------------------------------

/// The result of executing a tool.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Content to send back to the model as the tool result.
    pub content: String,
    /// Whether this invocation was an error.
    pub is_error: bool,
    /// Optional structured metadata (for the TUI to render diffs, etc.).
    pub metadata: Option<Value>,
}

impl ToolResult {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            metadata: None,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            metadata: None,
        }
    }

    pub fn with_metadata(mut self, meta: Value) -> Self {
        self.metadata = Some(meta);
        self
    }
}

/// Permission level required by a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionLevel {
    /// No permission needed (read-only, purely informational).
    None,
    /// Read-only access to the filesystem or network.
    ReadOnly,
    /// Write access to the filesystem.
    Write,
    /// Arbitrary command execution.
    Execute,
    /// Potentially dangerous (e.g., bypass sandbox).
    Dangerous,
    /// Unconditionally forbidden — the action must never be executed regardless
    /// of permission mode.  Used by BashTool when the classifier identifies a
    /// `Critical`-risk command (e.g. `rm -rf /`, fork-bomb, `dd if=…`).
    Forbidden,
}

/// Persistent shell state shared across Bash tool invocations within one session.
///
/// The `BashTool` reads and writes this state on every call so that `cd` and
/// `export` commands persist across separate tool invocations, matching the
/// mental model described in the tool description ("the working directory
/// persists between commands").
#[derive(Debug, Clone, Default)]
pub struct ShellState {
    /// Current working directory as tracked by the shell state.
    /// Starts as the session's `working_dir`; updated after each `cd` command.
    pub cwd: Option<PathBuf>,
    /// Environment variable overrides exported by previous commands.
    pub env_vars: HashMap<String, String>,
}

impl ShellState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Process-global registry of shell states keyed by session_id.
/// This lets us persist cwd/env across Bash invocations without changing
/// the `ToolContext` struct (which is constructed in places we cannot modify).
static SHELL_STATE_REGISTRY: once_cell::sync::Lazy<
    dashmap::DashMap<String, Arc<parking_lot::Mutex<ShellState>>>,
> = once_cell::sync::Lazy::new(dashmap::DashMap::new);

/// Process-global registry of `SnapshotManager` instances keyed by session_id.
/// Used by tools to record pre-write snapshots and by `/undo` to revert them.
static SNAPSHOT_REGISTRY: once_cell::sync::Lazy<
    dashmap::DashMap<String, Arc<parking_lot::Mutex<claurst_core::SnapshotManager>>>,
> = once_cell::sync::Lazy::new(dashmap::DashMap::new);

/// Return the persistent `ShellState` for the given session, creating one if needed.
pub fn session_shell_state(session_id: &str) -> Arc<parking_lot::Mutex<ShellState>> {
    SHELL_STATE_REGISTRY
        .entry(session_id.to_string())
        .or_insert_with(|| Arc::new(parking_lot::Mutex::new(ShellState::new())))
        .clone()
}

/// Remove the shell state for a session (e.g. when the session ends).
pub fn clear_session_shell_state(session_id: &str) {
    SHELL_STATE_REGISTRY.remove(session_id);
}

/// Return the persistent `SnapshotManager` for the given session, creating one if needed.
pub fn session_snapshot(
    session_id: &str,
) -> Arc<parking_lot::Mutex<claurst_core::SnapshotManager>> {
    SNAPSHOT_REGISTRY
        .entry(session_id.to_string())
        .or_insert_with(|| Arc::new(parking_lot::Mutex::new(claurst_core::SnapshotManager::new())))
        .clone()
}

/// Remove the snapshot manager for a session (e.g. when the session ends).
pub fn clear_session_snapshot(session_id: &str) {
    SNAPSHOT_REGISTRY.remove(session_id);
}

/// Shared context passed to every tool invocation.
#[derive(Clone)]
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub permission_mode: PermissionMode,
    pub permission_handler: Arc<dyn PermissionHandler>,
    pub cost_tracker: Arc<CostTracker>,
    pub session_id: String,
    pub file_history: Arc<parking_lot::Mutex<claurst_core::file_history::FileHistory>>,
    pub current_turn: Arc<AtomicUsize>,
    /// If true, suppress interactive prompts (batch / CI mode).
    pub non_interactive: bool,
    /// Optional MCP manager for ListMcpResources / ReadMcpResource tools.
    pub mcp_manager: Option<Arc<claurst_mcp::McpManager>>,
    /// Configured event hooks (PreToolUse, PostToolUse, etc.).
    pub config: claurst_core::config::Config,
}

impl ToolContext {
    /// Resolve a potentially relative path against the working directory.
    pub fn resolve_path(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            self.working_dir.join(p)
        }
    }

    /// Check permissions for a tool invocation.
    pub fn check_permission(
        &self,
        tool_name: &str,
        description: &str,
        is_read_only: bool,
    ) -> Result<(), claurst_core::error::ClaudeError> {
        let request = PermissionRequest {
            tool_name: tool_name.to_string(),
            description: description.to_string(),
            details: None,
            is_read_only,
            context_description: None,
        };
        let decision = self.permission_handler.request_permission(&request);
        match decision {
            PermissionDecision::Allow | PermissionDecision::AllowPermanently => Ok(()),
            _ => Err(claurst_core::error::ClaudeError::PermissionDenied(format!(
                "Permission denied for tool '{}'",
                tool_name
            ))),
        }
    }

    /// Like `check_permission` but also passes structured `details` text
    /// (e.g. a risk explanation) that the TUI permission dialog can display.
    ///
    /// Used by PowerShellTool (and any future tool) when it needs to show
    /// the user *why* a command is considered risky before they approve it.
    pub fn check_permission_with_details(
        &self,
        tool_name: &str,
        description: &str,
        details: &str,
        is_read_only: bool,
    ) -> Result<(), claurst_core::error::ClaudeError> {
        let request = PermissionRequest {
            tool_name: tool_name.to_string(),
            description: description.to_string(),
            details: Some(details.to_string()),
            is_read_only,
            context_description: None,
        };
        let decision = self.permission_handler.request_permission(&request);
        match decision {
            PermissionDecision::Allow | PermissionDecision::AllowPermanently => Ok(()),
            _ => Err(claurst_core::error::ClaudeError::PermissionDenied(format!(
                "Permission denied for tool '{}': {}",
                tool_name, details
            ))),
        }
    }

    pub fn current_turn_index(&self) -> usize {
        self.current_turn.load(Ordering::Relaxed)
    }

    pub fn record_file_change(
        &self,
        path: PathBuf,
        before_content: &[u8],
        after_content: &[u8],
        tool_name: &str,
    ) {
        self.file_history.lock().record_modification(
            path,
            before_content,
            after_content,
            self.current_turn_index(),
            tool_name,
        );
    }
}

/// The trait every tool must implement.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Human-readable name (matches the constant in claurst_core::constants).
    fn name(&self) -> &str;

    /// One-line description shown to the LLM.
    fn description(&self) -> &str;

    /// The permission level the tool requires.
    fn permission_level(&self) -> PermissionLevel;

    /// JSON Schema describing the tool's input parameters.
    fn input_schema(&self) -> Value;

    /// Execute the tool with the given JSON input.
    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult;

    /// Produce a `ToolDefinition` suitable for sending to the API.
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: self.input_schema(),
        }
    }
}

/// Return all built-in tools (excluding AgentTool, which lives in cc-query).
pub fn all_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(PtyBashTool),
        Box::new(FileReadTool),
        Box::new(FileEditTool),
        Box::new(FileWriteTool),
        Box::new(BatchEditTool),
        Box::new(ApplyPatchTool),
        Box::new(GlobTool),
        Box::new(GrepTool),
        Box::new(WebFetchTool),
        Box::new(WebSearchTool),
        Box::new(NotebookEditTool),
        Box::new(TaskCreateTool),
        Box::new(TaskGetTool),
        Box::new(TaskUpdateTool),
        Box::new(TaskListTool),
        Box::new(TaskStopTool),
        Box::new(TaskOutputTool),
        Box::new(TodoWriteTool),
        Box::new(AskUserQuestionTool),
        Box::new(EnterPlanModeTool),
        Box::new(ExitPlanModeTool),
        Box::new(PowerShellTool),
        Box::new(SleepTool),
        Box::new(CronCreateTool),
        Box::new(CronDeleteTool),
        Box::new(CronListTool),
        Box::new(EnterWorktreeTool),
        Box::new(ExitWorktreeTool),
        Box::new(ListMcpResourcesTool),
        Box::new(ReadMcpResourceTool),
        Box::new(ToolSearchTool),
        Box::new(BriefTool),
        Box::new(ConfigTool),
        Box::new(SendMessageTool),
        Box::new(SkillTool),
        Box::new(LspTool),
        Box::new(ReplTool),
        Box::new(TeamCreateTool),
        Box::new(TeamDeleteTool),
        Box::new(SyntheticOutputTool),
        Box::new(McpAuthTool),
        Box::new(RemoteTriggerTool),
        // Computer Use is only available when compiled with the feature flag.
        #[cfg(feature = "computer-use")]
        Box::new(computer_use::ComputerUseTool),
    ]
}

/// Find a tool by name (case-sensitive).
pub fn find_tool(name: &str) -> Option<Box<dyn Tool>> {
    all_tools().into_iter().find(|t| t.name() == name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Tool registry tests ------------------------------------------------

    #[test]
    fn test_all_tools_non_empty() {
        let tools = all_tools();
        assert!(
            !tools.is_empty(),
            "all_tools() must return at least one tool"
        );
    }

    #[test]
    fn test_all_tools_have_unique_names() {
        let tools = all_tools();
        let mut names = std::collections::HashSet::new();
        for tool in &tools {
            assert!(
                names.insert(tool.name().to_string()),
                "Duplicate tool name: {}",
                tool.name()
            );
        }
    }

    #[test]
    fn test_all_tools_have_non_empty_descriptions() {
        for tool in all_tools() {
            assert!(
                !tool.description().is_empty(),
                "Tool '{}' has empty description",
                tool.name()
            );
        }
    }

    #[test]
    fn test_all_tools_have_valid_input_schema() {
        for tool in all_tools() {
            let schema = tool.input_schema();
            assert!(
                schema.is_object(),
                "Tool '{}' input_schema must be a JSON object",
                tool.name()
            );
            assert!(
                schema.get("type").is_some() || schema.get("properties").is_some(),
                "Tool '{}' schema missing type or properties",
                tool.name()
            );
        }
    }

    #[test]
    fn test_find_tool_found() {
        let tool = find_tool("Bash");
        assert!(tool.is_some(), "Should find the Bash tool");
        assert_eq!(tool.unwrap().name(), "Bash");
    }

    #[test]
    fn test_find_tool_not_found() {
        assert!(find_tool("NonExistentTool12345").is_none());
    }

    #[test]
    fn test_find_tool_case_sensitive() {
        // Tool names are case-sensitive — "bash" should not match "Bash"
        assert!(find_tool("bash").is_none());
        assert!(find_tool("Bash").is_some());
    }

    #[test]
    fn test_core_tools_present() {
        let expected = [
            "Bash",
            "Read",
            "Edit",
            "Write",
            "Glob",
            "Grep",
            "WebFetch",
            "WebSearch",
            "TodoWrite",
            "Skill",
        ];
        for name in &expected {
            assert!(
                find_tool(name).is_some(),
                "Expected tool '{}' not found in all_tools()",
                name
            );
        }
    }

    // ---- ToolResult tests ---------------------------------------------------

    #[test]
    fn test_tool_result_success() {
        let r = ToolResult::success("done");
        assert!(!r.is_error);
        assert_eq!(r.content, "done");
        assert!(r.metadata.is_none());
    }

    #[test]
    fn test_tool_result_error() {
        let r = ToolResult::error("something went wrong");
        assert!(r.is_error);
        assert_eq!(r.content, "something went wrong");
    }

    #[test]
    fn test_tool_result_with_metadata() {
        let r = ToolResult::success("ok")
            .with_metadata(serde_json::json!({"file": "foo.rs", "lines": 10}));
        assert!(r.metadata.is_some());
        let meta = r.metadata.unwrap();
        assert_eq!(meta["file"], "foo.rs");
    }

    // ---- ToolContext::resolve_path tests ------------------------------------

    #[test]
    fn test_resolve_path_absolute() {
        use claurst_core::config::Config;
        use claurst_core::permissions::AutoPermissionHandler;

        let handler = Arc::new(AutoPermissionHandler {
            mode: claurst_core::config::PermissionMode::Default,
        });
        let ctx = ToolContext {
            working_dir: PathBuf::from("/workspace"),
            permission_mode: claurst_core::config::PermissionMode::Default,
            permission_handler: handler,
            cost_tracker: claurst_core::cost::CostTracker::new(),
            session_id: "test".to_string(),
            file_history: Arc::new(parking_lot::Mutex::new(
                claurst_core::file_history::FileHistory::new(),
            )),
            current_turn: Arc::new(AtomicUsize::new(0)),
            non_interactive: true,
            mcp_manager: None,
            config: Config::default(),
        };

        // Absolute paths pass through unchanged
        let resolved = ctx.resolve_path("/absolute/path/file.rs");
        assert_eq!(resolved, PathBuf::from("/absolute/path/file.rs"));
    }

    #[test]
    fn test_resolve_path_relative() {
        use claurst_core::config::Config;
        use claurst_core::permissions::AutoPermissionHandler;

        let handler = Arc::new(AutoPermissionHandler {
            mode: claurst_core::config::PermissionMode::Default,
        });
        let ctx = ToolContext {
            working_dir: PathBuf::from("/workspace"),
            permission_mode: claurst_core::config::PermissionMode::Default,
            permission_handler: handler,
            cost_tracker: claurst_core::cost::CostTracker::new(),
            session_id: "test".to_string(),
            file_history: Arc::new(parking_lot::Mutex::new(
                claurst_core::file_history::FileHistory::new(),
            )),
            current_turn: Arc::new(AtomicUsize::new(0)),
            non_interactive: true,
            mcp_manager: None,
            config: Config::default(),
        };

        // Relative paths get joined with working_dir
        let resolved = ctx.resolve_path("src/main.rs");
        assert_eq!(resolved, PathBuf::from("/workspace/src/main.rs"));
    }

    // ---- PermissionLevel tests ---------------------------------------------

    #[test]
    fn test_permission_level_order() {
        // Just verify the variants exist and are distinct
        assert_ne!(PermissionLevel::None, PermissionLevel::ReadOnly);
        assert_ne!(PermissionLevel::Write, PermissionLevel::Execute);
        assert_ne!(PermissionLevel::Execute, PermissionLevel::Dangerous);
    }

    #[test]
    fn test_bash_tool_permission_level() {
        assert_eq!(PtyBashTool.permission_level(), PermissionLevel::Execute);
    }

    #[test]
    fn test_file_read_permission_level() {
        assert_eq!(FileReadTool.permission_level(), PermissionLevel::ReadOnly);
    }

    #[test]
    fn test_file_edit_permission_level() {
        assert_eq!(FileEditTool.permission_level(), PermissionLevel::Write);
    }

    #[test]
    fn test_file_write_permission_level() {
        assert_eq!(FileWriteTool.permission_level(), PermissionLevel::Write);
    }

    // ---- Tool to_definition tests ------------------------------------------

    #[test]
    fn test_tool_to_definition() {
        let def = PtyBashTool.to_definition();
        assert_eq!(def.name, "Bash");
        assert!(!def.description.is_empty());
        assert!(def.input_schema.is_object());
    }
}
