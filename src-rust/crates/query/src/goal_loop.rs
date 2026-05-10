// goal_loop.rs — Goal continuation engine for the /goal feature.
//
// `check_and_continue_goal` is called by the CLI REPL after each query loop
// turn completes.  When an active goal exists it:
//   1. Checks runaway / budget guards
//   2. Records the turn in the GoalStore
//   3. Returns `GoalContinuation::Continue { message }` with the continuation
//      user message to inject, signalling the caller to dispatch another turn.
//
// The caller (cli/src/main.rs) is responsible for the actual dispatch so that
// TUI event handling and cancellation tokens stay in the right place.

use claurst_core::{GoalStatus, GoalStore, MAX_GOAL_TURNS, goal_continuation_message};

/// Result returned to the caller after a completed query loop turn.
pub enum GoalContinuation {
    /// Inject this user message and run another turn.
    Continue { message: String },
    /// Goal is done (complete, paused, cleared, budget hit, runaway).
    Stop { reason: StopReason },
    /// No goal is set for this session.
    NoGoal,
}

#[derive(Debug, Clone)]
pub enum StopReason {
    GoalComplete,
    Paused,
    BudgetLimited,
    RunawayGuard { turns_used: u32 },
    Error(String),
}

impl StopReason {
    pub fn user_message(&self) -> Option<String> {
        match self {
            StopReason::GoalComplete => {
                Some("Goal marked complete by the model.".to_string())
            }
            StopReason::Paused => None, // user-initiated, no extra message needed
            StopReason::BudgetLimited => Some(
                "Soft token budget reached — goal paused. Use /goal resume to continue.".to_string(),
            ),
            StopReason::RunawayGuard { turns_used } => Some(format!(
                "Goal paused after {} turns (runaway guard). Use /goal resume to continue.",
                turns_used
            )),
            StopReason::Error(msg) => Some(format!("Goal error: {}", msg)),
        }
    }
}

/// Inspect the current goal for `session_id` after a completed turn and decide
/// whether to continue.
///
/// `total_tokens_used` is the session-wide cumulative token count from the
/// cost tracker (used to enforce soft budgets).
/// `turn_elapsed_secs` is how long this turn took (for time accounting).
pub fn check_and_continue_goal(
    session_id: &str,
    total_tokens_used: u64,
    turn_elapsed_secs: u64,
) -> GoalContinuation {
    let store = match GoalStore::open_default() {
        Some(s) => s,
        None => return GoalContinuation::NoGoal,
    };

    let goal = match store.get_goal(session_id) {
        Some(g) => g,
        None => return GoalContinuation::NoGoal,
    };

    // If model (or user) already marked complete/paused, stop.
    match goal.status {
        GoalStatus::Complete => {
            return GoalContinuation::Stop { reason: StopReason::GoalComplete };
        }
        GoalStatus::Paused => {
            return GoalContinuation::Stop { reason: StopReason::Paused };
        }
        GoalStatus::BudgetLimited => {
            return GoalContinuation::Stop { reason: StopReason::BudgetLimited };
        }
        GoalStatus::Active => {}
    }

    // Runaway guard: check before incrementing so first fire is at MAX_GOAL_TURNS.
    if goal.turns_used >= MAX_GOAL_TURNS {
        let _ = store.set_status(session_id, GoalStatus::Paused);
        return GoalContinuation::Stop {
            reason: StopReason::RunawayGuard { turns_used: goal.turns_used },
        };
    }

    // Soft token budget check.
    if goal.is_over_budget(total_tokens_used) {
        let _ = store.set_status(session_id, GoalStatus::BudgetLimited);
        return GoalContinuation::Stop { reason: StopReason::BudgetLimited };
    }

    // Record this turn.
    if let Err(e) = store.record_turn(session_id, turn_elapsed_secs) {
        return GoalContinuation::Stop {
            reason: StopReason::Error(e.to_string()),
        };
    }

    // Reload after the update so turns_used is current.
    let goal = match store.get_goal(session_id) {
        Some(g) => g,
        None => return GoalContinuation::NoGoal,
    };

    // Build the continuation message.
    let message = goal_continuation_message(&goal);
    GoalContinuation::Continue { message }
}

/// Called by GoalCompleteTool to mark the goal complete.
pub fn mark_goal_complete(session_id: &str) -> Result<(), String> {
    let store = GoalStore::open_default()
        .ok_or_else(|| "Could not open goal store".to_string())?;
    store
        .set_status(session_id, GoalStatus::Complete)
        .map_err(|e| e.to_string())
}
