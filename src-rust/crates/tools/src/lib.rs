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
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// Sub-modules – each contains a full tool implementation.
pub mod ask_user;
pub mod bash;
pub mod pty_bash;
pub mod brief;
pub mod config_tool;
pub mod cron;
pub mod enter_plan_mode;
pub mod exit_plan_mode;
pub mod apply_patch;
pub mod batch_edit;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod glob_tool;
pub mod grep_tool;
pub mod lsp_tool;
pub mod mcp_resources;
pub mod todo_write;
pub mod notebook_edit;
pub mod powershell;
pub mod send_message;
pub mod bundled_skills;
pub mod skill_tool;
pub mod sleep;
pub mod tasks;
pub mod tool_search;
pub mod web_fetch;
pub mod web_search;
pub mod worktree;
pub mod computer_use;
pub mod mcp_auth_tool;
pub mod repl_tool;
pub mod synthetic_output;
pub mod team_tool;
pub mod remote_trigger;
pub mod formatter;
pub mod monitor_tool;
pub mod goal_complete;

// Re-exports for convenience.
pub use formatter::try_format_file;
pub use ask_user::AskUserQuestionTool;
pub use bash::BashTool;
pub use pty_bash::PtyBashTool;
pub use brief::BriefTool;
pub use config_tool::ConfigTool;
pub use cron::{CronCreateTool, CronDeleteTool, CronListTool};
pub use enter_plan_mode::EnterPlanModeTool;
pub use exit_plan_mode::ExitPlanModeTool;
pub use apply_patch::ApplyPatchTool;
pub use batch_edit::BatchEditTool;
pub use file_edit::FileEditTool;
pub use file_read::FileReadTool;
pub use file_write::FileWriteTool;
pub use glob_tool::GlobTool;
pub use grep_tool::GrepTool;
pub use lsp_tool::LspTool;
pub use mcp_resources::{ListMcpResourcesTool, ReadMcpResourceTool};
pub use todo_write::TodoWriteTool;
pub use notebook_edit::NotebookEditTool;
pub use powershell::PowerShellTool;
pub use send_message::{SendMessageTool, drain_inbox, peek_inbox};
pub use skill_tool::SkillTool;
pub use sleep::SleepTool;
pub use tasks::{TaskCreateTool, TaskGetTool, TaskListTool, TaskOutputTool, TaskStopTool, TaskUpdateTool, Task, TaskStatus, TASK_STORE};
pub use tool_search::ToolSearchTool;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
pub use worktree::{EnterWorktreeTool, ExitWorktreeTool};
pub use computer_use::ComputerUseTool;
pub use mcp_auth_tool::McpAuthTool;
pub use repl_tool::ReplTool;
pub use synthetic_output::SyntheticOutputTool;
pub use team_tool::{TeamCreateTool, TeamDeleteTool, register_agent_runner, AgentRunFn};
pub use remote_trigger::RemoteTriggerTool;
pub use monitor_tool::MonitorTool;
pub use goal_complete::GoalCompleteTool;

// ---------------------------------------------------------------------------
// AskUser question channel
// ---------------------------------------------------------------------------

/// Event sent through the TUI side-channel when the `AskUserQuestion` tool
/// needs to pause the query loop and collect a response from the user.
pub struct UserQuestionEvent {
    /// The question text to display.
    pub question: String,
    /// Optional predefined choices (for multiple-choice questions).
    pub options: Option<Vec<String>>,
    /// Send the user's answer back through this channel to resume execution.
    pub reply_tx: tokio::sync::oneshot::Sender<String>,
}

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

#[derive(Debug)]
pub struct PendingPermissionRequest {
    pub tool_use_id: String,
    pub request: claurst_core::permissions::PermissionRequest,
    pub reason: String,
    pub decision_tx: Option<tokio::sync::oneshot::Sender<PermissionDecision>>,
}

#[derive(Default)]
pub struct PendingPermissionStore {
    pub queue: VecDeque<PendingPermissionRequest>,
    pub waiting: HashMap<String, PendingPermissionRequest>,
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
static SHELL_STATE_REGISTRY: once_cell::sync::Lazy<dashmap::DashMap<String, Arc<parking_lot::Mutex<ShellState>>>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);

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

/// Return the `ShadowSnapshot` for `working_dir`, creating it on first call.
/// Returns `None` when git is unavailable or the directory is not in a git repo.
pub fn session_shadow(working_dir: &std::path::Path) -> Option<Arc<claurst_core::snapshot::ShadowSnapshot>> {
    claurst_core::snapshot::get_or_create(working_dir)
}

/// Drop the cached shadow snapshot for `working_dir` (e.g. when a session ends).
pub fn clear_session_shadow(working_dir: &std::path::Path) {
    claurst_core::snapshot::remove(working_dir);
}


/// A cloneable handle for injecting notification messages into the next agent turn.
/// Used by background tasks with `notify_on_complete` to signal completion without polling.
#[derive(Clone)]
pub struct CompletionNotifier(Arc<dyn Fn(String) + Send + Sync>);

impl CompletionNotifier {
    pub fn new(f: impl Fn(String) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
    pub fn notify(&self, msg: String) {
        (self.0)(msg);
    }
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
    /// Managed agent (manager-executor) configuration, if active.
    pub managed_agent_config: Option<claurst_core::config::ManagedAgentConfig>,
    /// Optional notifier for injecting completion messages into the next agent turn.
    /// Set when the query loop has a command queue wired up.
    pub completion_notifier: Option<CompletionNotifier>,
    /// Queue used by interactive mode to surface permission dialogs to the TUI.
    pub pending_permissions: Option<Arc<parking_lot::Mutex<PendingPermissionStore>>>,
    /// Shared permission manager so the interactive loop can record session/persistent approvals.
    pub permission_manager: Option<Arc<std::sync::Mutex<claurst_core::permissions::PermissionManager>>>,
    /// Channel for the `AskUserQuestion` tool to send questions to the TUI and
    /// receive the user's typed answer.  `None` in headless / non-interactive mode.
    pub user_question_tx: Option<tokio::sync::mpsc::UnboundedSender<UserQuestionEvent>>,
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

    fn permission_allowed_roots(&self) -> Vec<PathBuf> {
        let mut roots = self.config.workspace_paths.clone();
        roots.extend(self.config.additional_dirs.clone());
        roots
    }

    fn build_permission_request(
        &self,
        tool_name: &str,
        description: &str,
        details: Option<String>,
        is_read_only: bool,
        path: Option<PathBuf>,
    ) -> PermissionRequest {
        PermissionRequest {
            tool_name: tool_name.to_string(),
            description: description.to_string(),
            details,
            is_read_only,
            path: path.map(|p| p.display().to_string()),
            working_dir: Some(self.working_dir.clone()),
            allowed_roots: self.permission_allowed_roots(),
            context_description: None,
        }
    }

    fn request_permission_inner(
        &self,
        request: PermissionRequest,
    ) -> Result<(), claurst_core::error::ClaudeError> {
        let interactive_reason = request.details.clone();
        let decision = self.permission_handler.request_permission(&request);
        match decision {
            PermissionDecision::Allow | PermissionDecision::AllowPermanently => Ok(()),
            PermissionDecision::Ask { reason } if self.non_interactive => Err(
                claurst_core::error::ClaudeError::PermissionDenied(format!(
                    "Permission denied for tool '{}': {}",
                    request.tool_name,
                    interactive_reason.unwrap_or(reason)
                )),
            ),
            PermissionDecision::Ask { reason } => {
                let Some(queue) = &self.pending_permissions else {
                    return Err(claurst_core::error::ClaudeError::PermissionDenied(format!(
                        "Permission denied for tool '{}'",
                        request.tool_name
                    )));
                };

                let (tx, rx) = tokio::sync::oneshot::channel();
                queue.lock().queue.push_back(PendingPermissionRequest {
                    tool_use_id: format!(
                        "perm-{}-{}",
                        self.session_id,
                        self.current_turn.fetch_add(1, Ordering::Relaxed)
                    ),
                    request,
                    reason: interactive_reason.unwrap_or(reason),
                    decision_tx: Some(tx),
                });

                let decision = tokio::task::block_in_place(|| rx.blocking_recv());
                match decision {
                    Ok(PermissionDecision::Allow | PermissionDecision::AllowPermanently) => Ok(()),
                    _ => Err(claurst_core::error::ClaudeError::PermissionDenied(
                        "Permission denied by user".to_string(),
                    )),
                }
            }
            _ => Err(claurst_core::error::ClaudeError::PermissionDenied(format!(
                "Permission denied for tool '{}'",
                request.tool_name
            ))),
        }
    }

    /// Check permissions for a tool invocation.
    pub fn check_permission(
        &self,
        tool_name: &str,
        description: &str,
        is_read_only: bool,
    ) -> Result<(), claurst_core::error::ClaudeError> {
        let request = self.build_permission_request(tool_name, description, None, is_read_only, None);
        self.request_permission_inner(request)
    }

    pub fn check_permission_for_path(
        &self,
        tool_name: &str,
        description: &str,
        path: PathBuf,
        is_read_only: bool,
    ) -> Result<(), claurst_core::error::ClaudeError> {
        let request = self.build_permission_request(tool_name, description, None, is_read_only, Some(path));
        self.request_permission_inner(request)
    }

    /// Like `check_permission` but also passes structured `details` text
    /// (e.g. a risk explanation) that the TUI permission dialog can display.
    pub fn check_permission_with_details(
        &self,
        tool_name: &str,
        description: &str,
        details: &str,
        is_read_only: bool,
    ) -> Result<(), claurst_core::error::ClaudeError> {
        let request = self.build_permission_request(
            tool_name,
            description,
            Some(details.to_string()),
            is_read_only,
            None,
        );
        self.request_permission_inner(request).map_err(|_| {
            claurst_core::error::ClaudeError::PermissionDenied(format!(
                "Permission denied for tool '{}': {}",
                tool_name, details
            ))
        })
    }

    pub fn path_is_within_workspace(&self, path: &std::path::Path) -> bool {
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut roots = vec![
            std::fs::canonicalize(&self.working_dir).unwrap_or_else(|_| self.working_dir.clone()),
        ];
        roots.extend(
            self.permission_allowed_roots()
                .into_iter()
                .map(|root| std::fs::canonicalize(&root).unwrap_or(root)),
        );
        roots.iter().any(|root| resolved.starts_with(root))
    }

    pub fn check_permission_with_details_and_path(
        &self,
        tool_name: &str,
        description: &str,
        details: &str,
        path: PathBuf,
        is_read_only: bool,
    ) -> Result<(), claurst_core::error::ClaudeError> {
        let request = self.build_permission_request(
            tool_name,
            description,
            Some(details.to_string()),
            is_read_only,
            Some(path),
        );
        self.request_permission_inner(request).map_err(|_| {
            claurst_core::error::ClaudeError::PermissionDenied(format!(
                "Permission denied for tool '{}': {}",
                tool_name, details
            ))
        })
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
        Box::new(MonitorTool),
        Box::new(GoalCompleteTool),
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

    struct AskPermissionHandler {
        reason: String,
    }

    impl claurst_core::permissions::PermissionHandler for AskPermissionHandler {
        fn check_permission(
            &self,
            _request: &claurst_core::permissions::PermissionRequest,
        ) -> claurst_core::permissions::PermissionDecision {
            claurst_core::permissions::PermissionDecision::Ask {
                reason: self.reason.clone(),
            }
        }

        fn request_permission(
            &self,
            request: &claurst_core::permissions::PermissionRequest,
        ) -> claurst_core::permissions::PermissionDecision {
            self.check_permission(request)
        }
    }

    fn test_tool_context(
        handler: Arc<dyn claurst_core::permissions::PermissionHandler>,
    ) -> ToolContext {
        use claurst_core::config::Config;

        ToolContext {
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
            managed_agent_config: None,
            completion_notifier: None,
            pending_permissions: None,
            permission_manager: None,
            user_question_tx: None,
        }
    }

    // ---- Tool registry tests ------------------------------------------------

    #[test]
    fn test_all_tools_non_empty() {
        let tools = all_tools();
        assert!(!tools.is_empty(), "all_tools() must return at least one tool");
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
            "Bash", "Read", "Edit", "Write", "Glob", "Grep",
            "WebFetch", "WebSearch",
            "TodoWrite", "Skill",
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
        use claurst_core::permissions::AutoPermissionHandler;

        let handler = Arc::new(AutoPermissionHandler {
            mode: claurst_core::config::PermissionMode::Default,
        });
        let ctx = test_tool_context(handler);

        // Absolute paths pass through unchanged
        let resolved = ctx.resolve_path("/absolute/path/file.rs");
        assert_eq!(resolved, PathBuf::from("/absolute/path/file.rs"));
    }

    #[test]
    fn test_resolve_path_relative() {
        use claurst_core::permissions::AutoPermissionHandler;

        let handler = Arc::new(AutoPermissionHandler {
            mode: claurst_core::config::PermissionMode::Default,
        });
        let ctx = test_tool_context(handler);

        // Relative paths get joined with working_dir
        let resolved = ctx.resolve_path("src/main.rs");
        assert_eq!(resolved, PathBuf::from("/workspace/src/main.rs"));
    }

    #[test]
    fn test_request_permission_uses_details_for_non_interactive_errors() {
        let ctx = test_tool_context(Arc::new(AskPermissionHandler {
            reason: "generic reason".to_string(),
        }));
        let request = ctx.build_permission_request(
            "PowerShell",
            "[High risk] set execution policy",
            Some("[High risk] This may modify system-wide security policy.".to_string()),
            false,
            Some(PathBuf::from("Set-ExecutionPolicy RemoteSigned")),
        );

        let error = ctx.request_permission_inner(request).unwrap_err().to_string();
        assert!(error.contains("[High risk] This may modify system-wide security policy."));
        assert!(!error.contains("generic reason"));
    }

    #[test]
    fn test_request_permission_falls_back_to_handler_reason_without_details() {
        let ctx = test_tool_context(Arc::new(AskPermissionHandler {
            reason: "generic reason".to_string(),
        }));
        let request = ctx.build_permission_request(
            "Bash",
            "run ls",
            None,
            false,
            Some(PathBuf::from("ls -la")),
        );

        let error = ctx.request_permission_inner(request).unwrap_err().to_string();
        assert!(error.contains("generic reason"));
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
