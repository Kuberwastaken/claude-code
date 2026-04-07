// cron_scheduler: background task that fires cron-scheduled prompts.
//
// Runs as a long-lived tokio task. Every minute it checks the global CRON_STORE
// (in cc-tools) for tasks whose cron expression matches the current wall-clock
// minute. Matching tasks are fired by spawning a sub-query loop, exactly like
// the AgentTool does for sub-agents.
//
// One-shot tasks (recurring=false) are automatically removed from the store
// by `pop_due_tasks` after they are returned.

use crate::{run_query_loop, QueryConfig, QueryOutcome};
use chrono::Timelike;
use claurst_core::types::Message;
use claurst_tools::Tool;
use claurst_tools::ToolContext;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

/// Start the background cron scheduler.
///
/// Returns immediately; the scheduler runs as a detached tokio task.
/// Call `cancel.cancel()` to stop it gracefully.
pub fn start_cron_scheduler(
    client: Arc<claurst_api::AnthropicClient>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    tool_ctx: ToolContext,
    query_config: QueryConfig,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        run_scheduler_loop(client, tools, tool_ctx, query_config, cancel).await;
    });
}

async fn run_scheduler_loop(
    client: Arc<claurst_api::AnthropicClient>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    tool_ctx: ToolContext,
    query_config: QueryConfig,
    cancel: CancellationToken,
) {
    info!("Cron scheduler started");

    loop {
        // Sleep until the next whole-minute boundary (±1s tolerance).
        let now = chrono::Local::now();
        let secs_into_minute = now.second() as u64;
        let nanos_ms = now.nanosecond() as u64 / 1_000_000;
        // How many ms until the next minute starts? Use saturating sub to avoid underflow.
        let ms_to_next_minute = (60u64.saturating_sub(secs_into_minute))
            .saturating_mul(1_000)
            .saturating_sub(nanos_ms)
            .max(1); // always sleep at least 1ms

        tokio::select! {
            _ = sleep(Duration::from_millis(ms_to_next_minute)) => {}
            _ = cancel.cancelled() => {
                info!("Cron scheduler stopped");
                return;
            }
        }

        let tick_time = chrono::Local::now();
        debug!(time = %tick_time.format("%H:%M"), "Cron scheduler tick");

        // Find tasks due at this minute.
        let due = claurst_tools::cron::pop_due_tasks(&tick_time).await;

        for task in due {
            info!(id = %task.id, cron = %task.cron, "Firing cron task");

            let client = client.clone();
            let tools = tools.clone();
            let tool_ctx = tool_ctx.clone();
            let query_config = query_config.clone();
            let cost_tracker = tool_ctx.cost_tracker.clone();
            let cancel_child = cancel.clone();
            let task_id = task.id.clone();

            tokio::spawn(async move {
                let mut messages = vec![Message::user(task.prompt.clone())];

                let outcome = run_query_loop(
                    client.as_ref(),
                    &mut messages,
                    &tools,
                    &tool_ctx,
                    &query_config,
                    cost_tracker,
                    None, // background — no UI event channel
                    cancel_child,
                    None, // no pending message queue for cron tasks
                )
                .await;

                match outcome {
                    QueryOutcome::EndTurn { .. } => {
                        info!(id = %task_id, "Cron task completed");
                    }
                    QueryOutcome::Error(e) => {
                        error!(id = %task_id, error = %e, "Cron task failed");
                    }
                    QueryOutcome::MaxTokens { .. } => {
                        info!(id = %task_id, "Cron task hit max tokens");
                    }
                    QueryOutcome::Cancelled => {
                        debug!(id = %task_id, "Cron task cancelled");
                    }
                    QueryOutcome::BudgetExceeded {
                        cost_usd,
                        limit_usd,
                    } => {
                        eprintln!(
                            "[cron] task {} budget exceeded: spent ${:.4} of ${:.4}",
                            task_id, cost_usd, limit_usd
                        );
                    }
                }
            });
        }
    }
}
