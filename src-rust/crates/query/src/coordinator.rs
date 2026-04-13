//! Coordinator mode: multi-worker agent orchestration

pub const COORDINATOR_ENV_VAR: &str = "CLAURST_COORDINATOR_MODE";

/// Tools that belong exclusively to the coordinator — not exposed to workers.
/// Maps to INTERNAL_WORKER_TOOLS in coordinatorMode.ts.
pub const COORDINATOR_ONLY_TOOLS: &[&str] = &[
    "Agent",
    "SendMessage",
    "TaskStop",
    "TeamCreate",
    "TeamDelete",
    "SyntheticOutput",
];

/// Tools that workers are allowed to use in simple mode (CLAURST_SIMPLE=1).
pub const WORKER_SIMPLE_TOOLS: &[&str] = &["Bash", "Read", "Edit"];

/// Tools explicitly banned in coordinator mode (coordinator delegates these to workers).
pub const COORDINATOR_BANNED_TOOLS: &[&str] = &[
    // Coordinator should orchestrate, not execute directly.
    "Bash",
];

/// Agent mode for a session: either the coordinator itself or a worker spawned by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    Coordinator,
    Worker,
    Normal,
}

pub fn is_coordinator_mode() -> bool {
    std::env::var(COORDINATOR_ENV_VAR)
        .map(|v| !v.is_empty() && v != "0" && v != "false")
        .unwrap_or(false)
}

/// System prompt sections injected when coordinator mode is active
pub fn coordinator_system_prompt() -> &'static str {
    r#"
## Coordinator Mode

You are operating as an orchestrator for parallel worker agents.

### Your Role
- Orchestrate workers using the Agent tool to spawn parallel subagents
- Use SendMessage to continue communication with running workers
- Use TaskStop to cancel workers that are no longer needed
- Synthesize findings across workers before presenting to the user
- Answer directly when the question doesn't need delegation

### Task Workflow
1. **Research Phase**: Spawn workers to gather information in parallel
2. **Synthesis Phase**: Collect and merge worker findings
3. **Implementation Phase**: Delegate implementation tasks to specialized workers
4. **Verification Phase**: Spawn verification workers to validate results

### Worker Guidelines
- Worker prompts must be fully self-contained (workers cannot see your conversation)
- Always synthesize findings before spawning follow-up workers
- Workers have access to all standard tools + MCP + skills
- Use TaskCreate/TaskUpdate to track parallel work

### Internal Tools (do not delegate to workers)
- Agent, SendMessage, TaskStop (coordination only)
"#
}

/// Tools that should NOT be passed to worker agents (alias kept for
/// backwards-compatibility with existing callers in this file).
pub const INTERNAL_COORDINATOR_TOOLS: &[&str] = COORDINATOR_ONLY_TOOLS;

// ---------------------------------------------------------------------------
// Scratchpad gate
// ---------------------------------------------------------------------------

/// Guards scratchpad-gated tools.  When the unlock signal phrase appears in
/// model output, the gate opens and those tools become available.
///
/// Mirrors the scratchpad permission model in coordinatorMode.ts /
/// filesystem.ts (`isScratchpadEnabled` + `tengu_scratch` feature gate).
pub struct ScratchpadGate {
    unlocked: bool,
    /// The phrase that, when seen in content, opens the gate.
    unlock_signal: Option<String>,
}

impl ScratchpadGate {
    pub fn new() -> Self {
        Self {
            unlocked: false,
            unlock_signal: None,
        }
    }

    /// Create a gate with an explicit unlock phrase.
    pub fn with_signal(signal: impl Into<String>) -> Self {
        Self {
            unlocked: false,
            unlock_signal: Some(signal.into()),
        }
    }

    /// Returns `true` when `tool_name` is currently allowed through the gate.
    ///
    /// Tools NOT in the gated set are always allowed.  Gated tools are only
    /// allowed once `try_unlock` has been called with matching content.
    pub fn check(&self, tool_name: &str) -> bool {
        // File operations on the scratchpad dir are gated
        const GATED: &[&str] = &["Write", "FileWrite", "Edit", "FileEdit"];
        if GATED.contains(&tool_name) {
            return self.unlocked;
        }
        true
    }

    /// Returns `true` if `content` contains the unlock signal and the gate was
    /// opened as a result (or was already open).
    pub fn try_unlock(&mut self, content: &str) -> bool {
        if self.unlocked {
            return true;
        }
        if let Some(ref signal) = self.unlock_signal {
            if content.contains(signal.as_str()) {
                self.unlocked = true;
                return true;
            }
        }
        false
    }

    pub fn is_unlocked(&self) -> bool {
        self.unlocked
    }
}

impl Default for ScratchpadGate {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tool filtering
// ---------------------------------------------------------------------------

/// Filter a tool list so only tools appropriate for `mode` are included.
///
/// - `AgentMode::Coordinator`: all tools are available (coordinator has the
///   full set including Agent/SendMessage/TaskStop).
/// - `AgentMode::Worker`: COORDINATOR_ONLY_TOOLS are removed.
/// - `AgentMode::Normal`: no filtering.
pub fn filter_tools_for_mode<'a>(
    tools: &'a [Box<dyn claurst_tools::Tool>],
    mode: AgentMode,
) -> Vec<&'a Box<dyn claurst_tools::Tool>> {
    match mode {
        AgentMode::Coordinator | AgentMode::Normal => tools.iter().collect(),
        AgentMode::Worker => tools
            .iter()
            .filter(|t| !COORDINATOR_ONLY_TOOLS.contains(&t.name()))
            .collect(),
    }
}

/// Get the user context injected for coordinator sessions
pub fn coordinator_user_context(available_tools: &[String], mcp_servers: &[String]) -> String {
    let tool_list = available_tools
        .iter()
        .filter(|t| !INTERNAL_COORDINATOR_TOOLS.contains(&t.as_str()))
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");

    let mcp_section = if mcp_servers.is_empty() {
        String::new()
    } else {
        format!("\nConnected MCP servers: {}", mcp_servers.join(", "))
    };

    format!("Available worker tools: {}{}\n", tool_list, mcp_section)
}

/// Check if the current runtime coordinator flag matches `stored_coordinator`.
/// If mismatched, flips the env var to match the stored session and returns a
/// human-readable warning string.  Returns `None` when no switch was needed.
///
/// This is the lower-level variant used by session-resume code that stores the
/// mode as a plain bool.  For the typed `AgentMode` variant see
/// `match_session_mode_from_agent_mode`.
pub fn match_session_mode(stored_coordinator: bool) -> Option<String> {
    let current = is_coordinator_mode();
    if stored_coordinator == current {
        return None;
    }
    if stored_coordinator {
        // SAFETY: env-var mutation is inherently racy in multi-threaded
        // programs, but coordinator-mode toggling only happens at session
        // resume time before any worker threads are spawned.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(COORDINATOR_ENV_VAR, "1");
        }
        Some("Entered coordinator mode to match resumed session.".to_string())
    } else {
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(COORDINATOR_ENV_VAR);
        }
        Some("Exited coordinator mode to match resumed session.".to_string())
    }
}

/// Typed variant of `match_session_mode` — accepts an `AgentMode` directly.
/// Returns a warning string when the environment was changed, `None` otherwise.
pub fn match_session_mode_from_agent_mode(session_mode: AgentMode) -> Option<String> {
    match session_mode {
        AgentMode::Coordinator => match_session_mode(true),
        AgentMode::Normal | AgentMode::Worker => match_session_mode(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // All tests that mutate COORDINATOR_ENV_VAR must hold this guard to avoid
    // data races when cargo test runs them in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_is_coordinator_mode_unset() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var(COORDINATOR_ENV_VAR);
        assert!(!is_coordinator_mode());
    }

    #[test]
    fn test_is_coordinator_mode_set_to_one() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var(COORDINATOR_ENV_VAR, "1");
        assert!(is_coordinator_mode());
        std::env::remove_var(COORDINATOR_ENV_VAR);
    }

    #[test]
    fn test_is_coordinator_mode_set_to_false() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var(COORDINATOR_ENV_VAR, "false");
        assert!(!is_coordinator_mode());
        std::env::remove_var(COORDINATOR_ENV_VAR);
    }

    #[test]
    fn test_is_coordinator_mode_set_to_zero() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var(COORDINATOR_ENV_VAR, "0");
        assert!(!is_coordinator_mode());
        std::env::remove_var(COORDINATOR_ENV_VAR);
    }

    #[test]
    fn test_coordinator_user_context_filters_internal_tools() {
        let tools = vec![
            "Bash".to_string(),
            "Agent".to_string(),
            "SendMessage".to_string(),
            "TaskStop".to_string(),
            "Read".to_string(),
        ];
        let ctx = coordinator_user_context(&tools, &[]);
        assert!(ctx.contains("Bash"));
        assert!(ctx.contains("Read"));
        assert!(!ctx.contains("Agent"));
        assert!(!ctx.contains("SendMessage"));
        assert!(!ctx.contains("TaskStop"));
    }

    #[test]
    fn test_coordinator_user_context_mcp_servers() {
        let tools = vec!["Bash".to_string()];
        let mcps = vec!["filesystem".to_string(), "git".to_string()];
        let ctx = coordinator_user_context(&tools, &mcps);
        assert!(ctx.contains("filesystem"));
        assert!(ctx.contains("git"));
    }

    #[test]
    fn test_match_session_mode_no_change_needed() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var(COORDINATOR_ENV_VAR);
        // current = false, stored = false → no warning
        assert!(match_session_mode(false).is_none());
    }

    #[test]
    fn test_match_session_mode_switches_to_coordinator() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var(COORDINATOR_ENV_VAR);
        // current = false, stored = true → should flip and warn
        let msg = match_session_mode(true);
        assert!(msg.is_some());
        assert!(msg.unwrap().contains("coordinator"));
        // Clean up
        std::env::remove_var(COORDINATOR_ENV_VAR);
    }

    #[test]
    fn test_coordinator_system_prompt_content() {
        let prompt = coordinator_system_prompt();
        assert!(prompt.contains("Coordinator Mode"));
        assert!(prompt.contains("orchestrator"));
        assert!(prompt.contains("Research Phase"));
        assert!(prompt.contains("Synthesis Phase"));
    }

    #[test]
    fn test_scratchpad_gate_unlocked_by_default_for_non_gated_tools() {
        let gate = ScratchpadGate::new();
        assert!(gate.check("Bash"), "Bash should always pass the gate");
        assert!(gate.check("Read"), "Read should always pass the gate");
    }

    #[test]
    fn test_scratchpad_gate_blocks_write_until_unlocked() {
        let mut gate = ScratchpadGate::with_signal("SCRATCHPAD_READY");
        assert!(
            !gate.check("Write"),
            "Write should be blocked before unlock"
        );
        assert!(
            !gate.check("FileWrite"),
            "FileWrite should be blocked before unlock"
        );
        gate.try_unlock("Some content SCRATCHPAD_READY here");
        assert!(gate.check("Write"), "Write should be allowed after unlock");
        assert!(
            gate.check("FileWrite"),
            "FileWrite should be allowed after unlock"
        );
    }

    #[test]
    fn test_scratchpad_gate_try_unlock_wrong_signal() {
        let mut gate = ScratchpadGate::with_signal("SIGNAL_X");
        let result = gate.try_unlock("no signal here");
        assert!(!result);
        assert!(!gate.is_unlocked());
    }

    #[test]
    fn test_scratchpad_gate_already_unlocked() {
        let mut gate = ScratchpadGate::with_signal("SIG");
        gate.try_unlock("SIG");
        assert!(gate.try_unlock("nothing")); // already open
        assert!(gate.is_unlocked());
    }

    #[test]
    fn test_agent_mode_enum() {
        assert_ne!(AgentMode::Coordinator, AgentMode::Worker);
        assert_ne!(AgentMode::Worker, AgentMode::Normal);
        assert_eq!(AgentMode::Coordinator, AgentMode::Coordinator);
    }

    #[test]
    fn test_match_session_mode_from_agent_mode_coordinator() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var(COORDINATOR_ENV_VAR);
        let msg = match_session_mode_from_agent_mode(AgentMode::Coordinator);
        assert!(msg.is_some());
        assert!(msg.unwrap().contains("coordinator"));
        std::env::remove_var(COORDINATOR_ENV_VAR);
    }

    #[test]
    fn test_match_session_mode_from_agent_mode_normal_no_change() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var(COORDINATOR_ENV_VAR);
        let msg = match_session_mode_from_agent_mode(AgentMode::Normal);
        assert!(msg.is_none());
    }
}
