// managed_orchestrator.rs — System-prompt injection and helpers for
// the manager-executor managed agent architecture.

use claurst_core::ManagedAgentConfig;

/// Build the managed-agent section appended to the system prompt when managed mode is active.
pub fn managed_agent_system_prompt(config: &ManagedAgentConfig) -> String {
    let isolation_note = if config.executor_isolation {
        "Each executor runs in an isolated git worktree."
    } else {
        "Executors share the working directory."
    };

    let budget_note = match config.total_budget_usd {
        Some(b) => format!(
            "Total session budget: ${:.2}. Monitor your spend carefully.",
            b
        ),
        None => "No hard budget cap set. Be cost-conscious.".to_string(),
    };

    format!(
        r#"
## Managed Agent Mode

You are the MANAGER in a manager-executor architecture.

### Your Role
- You are the **planning and reasoning layer**. You coordinate work but do NOT execute tasks directly yourself using file/bash tools.
- Delegate all implementation work to executor agents using the **Agent tool**.
- Each executor uses model `{executor_model}` and has up to {max_turns} turns.
- You may run up to {max_concurrent} executors in parallel by setting `run_in_background: true` on the Agent tool call.

### Workflow
1. Analyze the user's request and break it into well-scoped sub-tasks.
2. Spawn an executor agent for each sub-task using the Agent tool.
3. Review executor results. If a result is insufficient, spawn a follow-up executor with clarified instructions.
4. Synthesize all results into a coherent response.

### Writing Good Executor Prompts
- Prompts must be **fully self-contained** — executors cannot see your conversation history.
- Include all relevant context: file paths, constraints, what has already been done.
- Be specific about the expected output format.
- Prefer fewer, larger tasks over many tiny ones to save cost.

### Executor Configuration
- Model: `{executor_model}`
- Max turns per executor: {max_turns}
- Max concurrent: {max_concurrent}
- {isolation_note}

### Budget
- {budget_note}
- Prefer batching work into fewer, well-scoped executors over spawning many small ones.
"#,
        executor_model = config.executor_model,
        max_turns = config.executor_max_turns,
        max_concurrent = config.max_concurrent_executors,
        isolation_note = isolation_note,
        budget_note = budget_note,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use claurst_core::ManagedAgentConfig;

    fn test_config() -> ManagedAgentConfig {
        ManagedAgentConfig {
            enabled: true,
            manager_model: "anthropic/claude-opus-4-6".to_string(),
            executor_model: "anthropic/claude-sonnet-4-6".to_string(),
            executor_max_turns: 8,
            max_concurrent_executors: 3,
            budget_split: claurst_core::BudgetSplitPolicy::SharedPool,
            total_budget_usd: Some(10.0),
            preset_name: None,
            executor_isolation: true,
        }
    }

    #[test]
    fn system_prompt_contains_executor_model() {
        let prompt = managed_agent_system_prompt(&test_config());
        assert!(prompt.contains("anthropic/claude-sonnet-4-6"));
        assert!(prompt.contains("8")); // max turns
        assert!(prompt.contains("3")); // max concurrent
    }

    #[test]
    fn system_prompt_mentions_isolation_when_enabled() {
        let prompt = managed_agent_system_prompt(&test_config());
        assert!(prompt.contains("worktree"));
    }

    #[test]
    fn system_prompt_mentions_budget() {
        let prompt = managed_agent_system_prompt(&test_config());
        assert!(prompt.contains("10.00"));
    }
}
