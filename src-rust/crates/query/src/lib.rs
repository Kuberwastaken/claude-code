// cc-query: The core agentic query loop.
//
// This crate implements the main conversation loop that:
// 1. Sends messages to the Anthropic API
// 2. Processes streaming responses
// 3. Detects tool-use requests and dispatches them
// 4. Feeds tool results back to the model
// 5. Handles auto-compact when the context window fills up
// 6. Manages stop conditions (end_turn, max_turns, cancellation)

pub mod agent_tool;
pub mod auto_dream;
pub mod away_summary;
pub mod command_queue;
pub mod compact;
pub mod context_analyzer;
pub mod coordinator;
pub mod cron_scheduler;
pub mod session_memory;
pub mod skill_prefetch;
pub use agent_tool::{init_team_swarm_runner, AgentTool};
pub use command_queue::{drain_command_queue, CommandPriority, CommandQueue, QueuedCommand};
pub use compact::{
    auto_compact_if_needed, calculate_messages_to_keep_index, calculate_token_warning_state,
    compact_conversation, context_collapse, context_window_for_model, format_compact_summary,
    get_compact_prompt, group_messages_for_compact, micro_compact_if_needed, reactive_compact,
    should_auto_compact, should_compact, should_context_collapse, snip_compact, AutoCompactState,
    CompactResult, CompactTrigger, MessageGroup, MicroCompactConfig, TokenWarningState,
};
pub use cron_scheduler::start_cron_scheduler;
pub use session_memory::{
    ExtractedMemory, MemoryCategory, SessionMemoryExtractor, SessionMemoryState,
};
pub use skill_prefetch::{
    format_skill_listing, prefetch_skills, SharedSkillIndex, SkillDefinition, SkillIndex,
};

use claurst_api::{
    AnthropicStreamEvent, ApiMessage, ApiToolDefinition, CreateMessageRequest, StreamAccumulator,
    StreamHandler, SystemPrompt, ThinkingConfig,
};
use claurst_core::config::Config;
use claurst_core::cost::CostTracker;
use claurst_core::error::ClaudeError;
use claurst_core::types::{ContentBlock, Message, ToolResultContent, UsageInfo};
use claurst_tools::{Tool, ToolContext, ToolResult};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Outcome of a single query-loop run.
#[derive(Debug)]
pub enum QueryOutcome {
    /// The model finished its turn (end_turn stop reason).
    EndTurn { message: Message, usage: UsageInfo },
    /// The model hit max_tokens.
    MaxTokens {
        partial_message: Message,
        usage: UsageInfo,
    },
    /// The conversation was cancelled by the user.
    Cancelled,
    /// An unrecoverable error occurred.
    Error(ClaudeError),
    /// The configured USD budget was exceeded.
    BudgetExceeded { cost_usd: f64, limit_usd: f64 },
}

/// Configuration for a single query-loop invocation.
#[derive(Clone)]
pub struct QueryConfig {
    pub model: String,
    pub max_tokens: u32,
    pub max_turns: u32,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub output_style: claurst_core::system_prompt::OutputStyle,
    pub output_style_prompt: Option<String>,
    pub working_directory: Option<String>,
    pub thinking_budget: Option<u32>,
    pub temperature: Option<f32>,
    /// Maximum cumulative character count of all tool results in the message
    /// history before older results are replaced with a truncation notice.
    /// Mirrors the TS `applyToolResultBudget` mechanism.  Default: 50_000.
    pub tool_result_budget: usize,
    /// Optional effort level.  When set and `thinking_budget` is `None`,
    /// the effort level's `thinking_budget_tokens()` is used as the
    /// thinking budget.  Also provides a temperature override when the
    /// level specifies one.
    pub effort_level: Option<claurst_core::effort::EffortLevel>,
    /// T1-4: Optional shared command queue.
    ///
    /// When set, the query loop drains this queue before each API call and
    /// injects any resulting messages into the conversation.  The queue is
    /// shared (Arc-backed) so the TUI input thread can push commands while the
    /// loop is waiting for a model response.
    pub command_queue: Option<CommandQueue>,
    /// T1-5: Optional shared skill index.
    ///
    /// When set, `prefetch_skills` is spawned once before the loop begins and
    /// the resulting index is used to inject a skill listing attachment into
    /// the conversation context.
    pub skill_index: Option<SharedSkillIndex>,
    /// Optional USD spend cap. The query loop checks accumulated cost after
    /// each turn and aborts with `QueryOutcome::BudgetExceeded` when exceeded.
    pub max_budget_usd: Option<f64>,
    /// Fallback model name. Used when the primary model returns overloaded /
    /// rate-limit errors (mirrors TS `--fallback-model`).
    pub fallback_model: Option<String>,
    /// Optional ProviderRegistry for dispatching to non-Anthropic providers.
    /// When `config.provider` is set to something other than "anthropic" and
    /// this registry contains that provider, the registry's provider is used
    /// instead of `AnthropicClient`.
    pub provider_registry: Option<std::sync::Arc<claurst_api::ProviderRegistry>>,
    /// Active agent name (e.g., "build", "plan", "explore", or None for default).
    pub agent_name: Option<String>,
    /// Resolved agent definition for the current session.
    pub agent_definition: Option<claurst_core::AgentDefinition>,
    /// Optional shared model registry for dynamic provider and model resolution.
    /// When set, the query loop uses this instead of constructing a fresh registry.
    pub model_registry: Option<std::sync::Arc<claurst_api::ModelRegistry>>,
}

impl Default for QueryConfig {
    fn default() -> Self {
        Self {
            model: claurst_core::constants::DEFAULT_MODEL.to_string(),
            max_tokens: claurst_core::constants::DEFAULT_MAX_TOKENS,
            max_turns: claurst_core::constants::MAX_TURNS_DEFAULT,
            system_prompt: None,
            append_system_prompt: None,
            output_style: claurst_core::system_prompt::OutputStyle::Default,
            output_style_prompt: None,
            working_directory: None,
            thinking_budget: None,
            temperature: None,
            tool_result_budget: 50_000,
            effort_level: None,
            command_queue: None,
            skill_index: None,
            max_budget_usd: None,
            fallback_model: None,
            provider_registry: None,
            agent_name: None,
            agent_definition: None,
            model_registry: None,
        }
    }
}

impl QueryConfig {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            model: cfg.effective_model().to_string(),
            max_tokens: cfg.effective_max_tokens(),
            output_style: cfg.effective_output_style(),
            output_style_prompt: cfg.resolve_output_style_prompt(),
            working_directory: cfg.project_dir.as_ref().map(|p| p.display().to_string()),
            ..Default::default()
        }
    }

    /// Build a QueryConfig using dynamic model resolution from the model registry.
    ///
    /// Prefers the best model for the configured provider (from models.dev data)
    /// over the hardcoded defaults.
    pub fn from_config_with_registry(cfg: &Config, registry: &claurst_api::ModelRegistry) -> Self {
        // We can't move the Arc here, but we need a clone for the query loop.
        // Callers typically wrap the registry in an Arc already.
        Self {
            model: claurst_api::effective_model_for_config(cfg, registry),
            max_tokens: cfg.effective_max_tokens(),
            output_style: cfg.effective_output_style(),
            output_style_prompt: cfg.resolve_output_style_prompt(),
            working_directory: cfg.project_dir.as_ref().map(|p| p.display().to_string()),
            ..Default::default()
        }
    }
}

fn reasoning_effort_for_level(effort_level: claurst_core::effort::EffortLevel) -> &'static str {
    match effort_level {
        claurst_core::effort::EffortLevel::Low => "low",
        claurst_core::effort::EffortLevel::Medium => "medium",
        claurst_core::effort::EffortLevel::High | claurst_core::effort::EffortLevel::Max => "high",
    }
}

fn google_thinking_level_for_effort(
    effort_level: Option<claurst_core::effort::EffortLevel>,
) -> &'static str {
    match effort_level.unwrap_or(claurst_core::effort::EffortLevel::High) {
        claurst_core::effort::EffortLevel::Low => "low",
        claurst_core::effort::EffortLevel::Medium => "medium",
        claurst_core::effort::EffortLevel::High | claurst_core::effort::EffortLevel::Max => "high",
    }
}

fn is_openai_reasoning_model(model_id: &str) -> bool {
    let model_id = model_id.to_ascii_lowercase();
    model_id.starts_with("gpt-5")
        || model_id.starts_with("o1")
        || model_id.starts_with("o3")
        || model_id.starts_with("o4")
}

fn is_openaiish_provider(provider_id: &str) -> bool {
    matches!(
        provider_id,
        "openai"
            | "azure"
            | "groq"
            | "mistral"
            | "deepseek"
            | "xai"
            | "openrouter"
            | "togetherai"
            | "together-ai"
            | "perplexity"
            | "cerebras"
            | "deepinfra"
            | "venice"
            | "huggingface"
            | "nvidia"
            | "siliconflow"
            | "sambanova"
            | "moonshot"
            | "zhipu"
            | "qwen"
            | "nebius"
            | "novita"
            | "ovhcloud"
            | "scaleway"
            | "vultr"
            | "vultr-ai"
            | "baseten"
            | "friendli"
            | "upstage"
            | "stepfun"
            | "fireworks"
            | "ollama"
            | "lmstudio"
            | "lm-studio"
            | "llamacpp"
            | "llama-cpp"
    )
}

fn build_provider_options(
    provider_id: &str,
    model_id: &str,
    effort_level: Option<claurst_core::effort::EffortLevel>,
    thinking_budget: Option<u32>,
) -> Value {
    let mut options = serde_json::Map::new();
    let model_id = model_id.to_ascii_lowercase();

    if provider_id == "github-copilot" {
        if model_id.contains("claude") {
            options.insert(
                "thinking_budget".to_string(),
                serde_json::json!(thinking_budget.unwrap_or(4_000)),
            );
        } else if model_id.starts_with("gpt-5") && !model_id.contains("gpt-5-pro") {
            let reasoning_effort = effort_level
                .map(reasoning_effort_for_level)
                .unwrap_or("medium");
            options.insert(
                "reasoningEffort".to_string(),
                serde_json::json!(reasoning_effort),
            );
            options.insert("reasoningSummary".to_string(), serde_json::json!("auto"));
            options.insert(
                "include".to_string(),
                serde_json::json!(["reasoning.encrypted_content"]),
            );

            if model_id.contains("gpt-5.")
                && !model_id.contains("codex")
                && !model_id.contains("-chat")
            {
                options.insert("textVerbosity".to_string(), serde_json::json!("low"));
            }
        }
    }

    if provider_id == "google" && model_id.contains("gemini") {
        if model_id.contains("2.5") {
            if let Some(budget) = thinking_budget {
                options.insert(
                    "thinkingConfig".to_string(),
                    serde_json::json!({
                        "includeThoughts": true,
                        "thinkingBudget": budget,
                    }),
                );
            }
        } else if model_id.contains("3.") || model_id.contains("gemini-3") {
            options.insert(
                "thinkingConfig".to_string(),
                serde_json::json!({
                    "includeThoughts": true,
                    "thinkingLevel": google_thinking_level_for_effort(effort_level),
                }),
            );
        }
    }

    if provider_id == "amazon-bedrock" {
        if model_id.contains("anthropic") || model_id.contains("claude") {
            if let Some(budget) = thinking_budget {
                options.insert(
                    "reasoningConfig".to_string(),
                    serde_json::json!({
                        "type": "enabled",
                        "budgetTokens": budget.min(31_999),
                    }),
                );
            }
        } else if let Some(level) = effort_level {
            options.insert(
                "reasoningConfig".to_string(),
                serde_json::json!({
                    "type": "enabled",
                    "maxReasoningEffort": reasoning_effort_for_level(level),
                }),
            );
        }
    }

    if is_openaiish_provider(provider_id) && is_openai_reasoning_model(&model_id) {
        let reasoning_effort = effort_level
            .map(reasoning_effort_for_level)
            .unwrap_or("medium");
        options.insert(
            "reasoningEffort".to_string(),
            serde_json::json!(reasoning_effort),
        );

        if model_id.starts_with("gpt-5")
            && model_id.contains("gpt-5.")
            && !model_id.contains("codex")
            && !model_id.contains("-chat")
            && provider_id != "azure"
        {
            options.insert("textVerbosity".to_string(), serde_json::json!("low"));
        }
    }

    if provider_id == "openrouter" {
        options.insert("usage".to_string(), serde_json::json!({ "include": true }));
        if model_id.contains("gemini-3") {
            options.insert(
                "reasoning".to_string(),
                serde_json::json!({ "effort": "high" }),
            );
        }
    }

    if provider_id == "qwen" && thinking_budget.is_some() && !model_id.contains("kimi-k2-thinking")
    {
        options.insert("enable_thinking".to_string(), serde_json::json!(true));
    }

    if provider_id == "zhipu" && thinking_budget.is_some() {
        options.insert(
            "thinking".to_string(),
            serde_json::json!({
                "type": "enabled",
                "clear_thinking": false,
            }),
        );
    }

    if options.is_empty() {
        Value::Null
    } else {
        Value::Object(options)
    }
}

/// Events emitted by the query loop for the TUI to render.
#[derive(Debug, Clone)]
pub enum QueryEvent {
    /// A stream event from the API.
    Stream(AnthropicStreamEvent),
    /// A tool is about to be executed.
    ToolStart {
        tool_name: String,
        tool_id: String,
        input_json: String,
    },
    /// A tool has finished executing.
    ToolEnd {
        tool_name: String,
        tool_id: String,
        result: String,
        is_error: bool,
    },
    /// The model finished a turn.
    TurnComplete {
        turn: u32,
        stop_reason: String,
        usage: Option<UsageInfo>,
    },
    /// An informational status message.
    Status(String),
    /// An error.
    Error(String),
    /// Token usage has crossed a warning threshold.
    /// `state` is Warning (≥ 80 %) or Critical (≥ 95 %).
    /// `pct_used` is the fraction of the context window consumed (0.0–1.0).
    TokenWarning {
        state: TokenWarningState,
        pct_used: f64,
    },
}

// ---------------------------------------------------------------------------
// T1-3: Post-sampling hooks
// ---------------------------------------------------------------------------

/// Result returned by `fire_post_sampling_hooks`.
#[derive(Debug, Default)]
pub struct PostSamplingHookResult {
    /// Error messages produced by hooks with non-zero exit codes.
    /// These are injected into the conversation as user messages before the
    /// next model turn so the model can react to them.
    pub blocking_errors: Vec<claurst_core::types::Message>,
    /// When `true` the query loop must not continue and should surface the
    /// error messages to the caller.  Set when any hook exits with code > 1.
    pub prevent_continuation: bool,
}

/// Execute all `PostModelTurn` hooks defined in `config.hooks`.
///
/// Each hook is run synchronously (blocking via `std::process::Command`).
/// On a non-zero exit code, the hook's stderr (falling back to stdout) is
/// wrapped in a user `Message` and appended to `blocking_errors`.
/// If the exit code is **strictly greater than 1** `prevent_continuation` is
/// set so the query loop can return early.
pub fn fire_post_sampling_hooks(
    _turn_result: &claurst_core::types::Message,
    config: &claurst_core::config::Config,
) -> PostSamplingHookResult {
    use claurst_core::config::HookEvent;
    use claurst_core::types::Message;

    let mut result = PostSamplingHookResult::default();

    let entries = match config.hooks.get(&HookEvent::PostModelTurn) {
        Some(e) => e,
        None => return result,
    };

    for entry in entries {
        let sh = if cfg!(windows) { "cmd" } else { "sh" };
        let flag = if cfg!(windows) { "/C" } else { "-c" };

        let output = match std::process::Command::new(sh)
            .args([flag, &entry.command])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(command = %entry.command, error = %e, "PostModelTurn hook spawn failed");
                continue;
            }
        };

        if output.status.success() {
            continue;
        }

        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let body = if !stderr.trim().is_empty() {
            stderr
        } else {
            stdout
        };

        tracing::warn!(
            command = %entry.command,
            exit_code = ?output.status.code(),
            "PostModelTurn hook returned non-zero exit"
        );

        result.blocking_errors.push(Message::user(format!(
            "[Hook '{}' error]:\n{}",
            entry.command,
            body.trim()
        )));

        // Exit code > 1 → hard veto of continuation.
        if output.status.code().unwrap_or(1) > 1 {
            result.prevent_continuation = true;
        }
    }

    result
}

/// Spawn all `Stop` hooks in fire-and-forget background tasks.
///
/// Stop hooks are non-blocking by design: the caller does not wait for them.
/// Returns an empty `Vec` immediately; results (if any) are lost.
pub fn stop_hooks_with_full_behavior(
    turn_result: &claurst_core::types::Message,
    config: &claurst_core::config::Config,
    working_dir: std::path::PathBuf,
) -> Vec<claurst_core::types::Message> {
    use claurst_core::config::HookEvent;

    let entries = match config.hooks.get(&HookEvent::Stop) {
        Some(e) if !e.is_empty() => e.clone(),
        _ => return Vec::new(),
    };

    let output_text = turn_result.get_all_text();

    for entry in entries {
        let cmd = entry.command.clone();
        let dir = working_dir.clone();
        let text = output_text.clone();

        tokio::task::spawn_blocking(move || {
            let sh = if cfg!(windows) { "cmd" } else { "sh" };
            let flag = if cfg!(windows) { "/C" } else { "-c" };

            let _ = std::process::Command::new(sh)
                .args([flag, &cmd])
                .current_dir(&dir)
                .env("CLAUDE_HOOK_OUTPUT", &text)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        });
    }

    Vec::new()
}

// ---------------------------------------------------------------------------
// Tool-result budgeting
// ---------------------------------------------------------------------------

/// Return the combined character count of all tool-result content blocks found
/// in `messages`.  Only user messages are examined (tool results always live
/// in user turns).
fn total_tool_result_chars(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|m| m.role == claurst_core::types::Role::User)
        .flat_map(|m| match &m.content {
            claurst_core::types::MessageContent::Blocks(blocks) => blocks.as_slice(),
            _ => &[],
        })
        .filter_map(|b| {
            if let ContentBlock::ToolResult { content, .. } = b {
                Some(match content {
                    ToolResultContent::Text(t) => t.len(),
                    ToolResultContent::Blocks(blocks) => blocks
                        .iter()
                        .map(|b| {
                            if let ContentBlock::Text { text } = b {
                                text.len()
                            } else {
                                0
                            }
                        })
                        .sum(),
                })
            } else {
                None
            }
        })
        .sum()
}

/// When the cumulative tool-result content exceeds `budget` characters, walk
/// the message list from oldest to newest and replace individual
/// `ToolResult` content with a placeholder until the running total is back
/// under budget.  Returns the (possibly modified) message list and the
/// number of results that were truncated.
///
/// Mirrors the spirit of the TypeScript `applyToolResultBudget` /
/// `enforceToolResultBudget` logic, simplified to a straightforward
/// oldest-first eviction without the session-persistence layer.
fn apply_tool_result_budget(messages: Vec<Message>, budget: usize) -> (Vec<Message>, usize) {
    let total = total_tool_result_chars(&messages);
    if total <= budget {
        return (messages, 0);
    }

    let mut to_shed = total - budget;
    let mut truncated = 0usize;
    let mut result = messages;

    'outer: for msg in result.iter_mut() {
        if msg.role != claurst_core::types::Role::User {
            continue;
        }
        let blocks = match &mut msg.content {
            claurst_core::types::MessageContent::Blocks(b) => b,
            _ => continue,
        };
        for block in blocks.iter_mut() {
            if let ContentBlock::ToolResult { content, .. } = block {
                let size = match &*content {
                    ToolResultContent::Text(t) => t.len(),
                    ToolResultContent::Blocks(inner) => inner
                        .iter()
                        .map(|b| {
                            if let ContentBlock::Text { text } = b {
                                text.len()
                            } else {
                                0
                            }
                        })
                        .sum(),
                };
                if size == 0 {
                    continue;
                }
                *content =
                    ToolResultContent::Text("[tool result truncated to save context]".to_string());
                truncated += 1;
                if size > to_shed {
                    break 'outer;
                }
                to_shed -= size;
            }
        }
    }

    (result, truncated)
}

// ---------------------------------------------------------------------------
// Query loop
// ---------------------------------------------------------------------------

/// Maximum number of max_tokens continuation attempts before surfacing the
/// partial response.  Mirrors `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT` in query.ts.
const MAX_TOKENS_RECOVERY_LIMIT: u32 = 3;

/// Message injected when the model hits its output-token limit.
/// Mirrors the TS recovery message in query.ts lines 1224-1228.
const MAX_TOKENS_RECOVERY_MSG: &str =
    "Output token limit hit. Resume directly — no apology, no recap of what \
     you were doing. Pick up mid-thought if that is where the cut happened. \
     Break remaining work into smaller pieces.";

/// Run the agentic query loop.
///
/// This sends the conversation to the API, handles tool calls in a loop, and
/// returns when the model issues an end_turn or an error/limit is hit.
///
/// `pending_messages` is an optional queue of user messages that were enqueued
/// during tool execution (e.g. by the UI or a command queue).  Each string is
/// appended as a plain user message between turns.  Callers that do not need
/// command queuing may pass `None` or an empty `Vec`.
pub async fn run_query_loop(
    client: &claurst_api::AnthropicClient,
    messages: &mut Vec<Message>,
    tools: &[Box<dyn Tool>],
    tool_ctx: &ToolContext,
    config: &QueryConfig,
    cost_tracker: Arc<CostTracker>,
    event_tx: Option<mpsc::UnboundedSender<QueryEvent>>,
    cancel_token: tokio_util::sync::CancellationToken,
    mut pending_messages: Option<&mut Vec<String>>,
) -> QueryOutcome {
    let mut turn = 0u32;
    let mut compact_state = compact::AutoCompactState::default();
    // Tracks how many consecutive max_tokens recoveries we've attempted so
    // we don't loop forever on a model that can't finish within any budget.
    let mut max_tokens_recovery_count: u32 = 0;
    // Active model — may switch to fallback on overloaded errors.
    // Agent model override takes priority over the session model when set.
    let mut effective_model = if let Some(ref agent) = config.agent_definition {
        agent.model.clone().unwrap_or_else(|| config.model.clone())
    } else {
        config.model.clone()
    };
    let mut used_fallback = false;
    // How many automatic retries remain when a stream stalls (no data for 45s).
    let mut retries_left: u32 = 2;

    // If an agent defines a max_turns override, respect it (agent wins over config).
    let effective_max_turns = config
        .agent_definition
        .as_ref()
        .and_then(|a| a.max_turns)
        .unwrap_or(config.max_turns);

    loop {
        turn += 1;
        tool_ctx
            .current_turn
            .store(turn as usize, std::sync::atomic::Ordering::Relaxed);
        if turn > effective_max_turns {
            info!(turns = turn, "Max turns reached");
            if let Some(ref tx) = event_tx {
                let _ = tx.send(QueryEvent::Status(format!(
                    "Reached maximum turn limit ({})",
                    effective_max_turns
                )));
            }
            // Return the last assistant message if any
            let last_msg = messages
                .last()
                .cloned()
                .unwrap_or_else(|| Message::assistant("Max turns reached."));
            return QueryOutcome::EndTurn {
                message: last_msg,
                usage: UsageInfo::default(),
            };
        }

        // Check for cancellation
        if cancel_token.is_cancelled() {
            return QueryOutcome::Cancelled;
        }

        // Drain any pending user messages that were queued during the previous
        // tool-execution phase (e.g. commands entered while tools ran).
        // Mirrors the TS `messageQueueManager` drain between turns.
        if let Some(queue) = pending_messages.as_deref_mut() {
            for text in queue.drain(..) {
                debug!("Injecting pending message: {}", &text);
                messages.push(Message::user(text));
            }
        }

        // T1-4: Drain the priority command queue (if wired up) and prepend any
        // resulting messages to the conversation before the API call.
        // Mirrors the TS `messageQueueManager` priority-queue drain.
        if let Some(ref cq) = config.command_queue {
            if !cq.is_empty() {
                let injected = drain_command_queue(cq);
                if !injected.is_empty() {
                    debug!(count = injected.len(), "Injecting command-queue messages");
                    // Prepend so that higher-priority commands appear first.
                    let tail = std::mem::take(messages);
                    messages.extend(injected);
                    messages.extend(tail);
                }
            }
        }

        // Apply tool-result budget: if the cumulative size of all tool results
        // in the conversation exceeds the configured threshold, replace the
        // oldest results with a placeholder until we're back under budget.
        // This mirrors the TS `applyToolResultBudget` call in query.ts.
        if config.tool_result_budget > 0 {
            let (budgeted, truncated) =
                apply_tool_result_budget(std::mem::take(messages), config.tool_result_budget);
            *messages = budgeted;
            if truncated > 0 {
                info!(
                    truncated,
                    budget = config.tool_result_budget,
                    "Tool-result budget exceeded: truncated {} result(s)",
                    truncated
                );
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(QueryEvent::Status(format!(
                        "[{} older tool result(s) truncated to save context]",
                        truncated
                    )));
                }
            }
        }

        // Build API request
        let api_messages: Vec<ApiMessage> = messages.iter().map(ApiMessage::from).collect();
        let api_tools: Vec<ApiToolDefinition> = tools
            .iter()
            .map(|t| ApiToolDefinition::from(&t.to_definition()))
            .collect();

        // Verification nudge: if there are incomplete todos for this session
        // and the conversation has more than 2 turns, append a reminder.
        let system = {
            // Build a (possibly patched) config for system-prompt assembly.
            // Agent prompt prefix and todo nudge are both applied here.
            let mut patched = config.clone();

            // Apply agent system-prompt prefix: prepend before the main system prompt.
            if let Some(ref agent) = config.agent_definition {
                if let Some(ref agent_prompt) = agent.prompt {
                    patched.system_prompt = Some(match &config.system_prompt {
                        Some(existing) => format!("{}\n\n{}", agent_prompt, existing),
                        None => agent_prompt.clone(),
                    });
                }
            }

            // Apply todo nudge on turns > 2.
            if turn > 2 {
                let nudge = build_todo_nudge(&tool_ctx.session_id);
                if !nudge.is_empty() {
                    patched.append_system_prompt = Some(match &config.append_system_prompt {
                        Some(existing) => format!("{}\n\n{}", existing, nudge),
                        None => nudge,
                    });
                }
            }

            build_system_prompt(&patched)
        };

        let system_for_provider = system.clone(); // used by non-Anthropic dispatch below
        let mut req_builder = CreateMessageRequest::builder(&effective_model, config.max_tokens)
            .messages(api_messages)
            .system(system)
            .tools(api_tools);

        // Resolve effective thinking budget:
        //   1. Explicit `thinking_budget` in config takes precedence.
        //   2. Fall back to the effort level's budget when no explicit budget is set.
        let effective_thinking_budget = config.thinking_budget.or_else(|| {
            config
                .effort_level
                .and_then(|el| el.thinking_budget_tokens())
        });

        if let Some(budget) = effective_thinking_budget {
            req_builder = req_builder.thinking(ThinkingConfig::enabled(budget));
        }

        // Apply temperature: explicit config value takes precedence, then agent override,
        // then effort-level override.
        let effective_temperature = config
            .temperature
            .or_else(|| {
                config
                    .agent_definition
                    .as_ref()
                    .and_then(|a| a.temperature)
                    .map(|t| t as f32)
            })
            .or_else(|| config.effort_level.and_then(|el| el.temperature()));
        if let Some(t) = effective_temperature {
            req_builder = req_builder.temperature(t);
        }

        let request = req_builder.build();

        // Create a stream handler that forwards to the event channel
        let handler: Arc<dyn StreamHandler> = if let Some(ref tx) = event_tx {
            let tx = tx.clone();
            Arc::new(ChannelStreamHandler { tx })
        } else {
            Arc::new(claurst_api::streaming::NullStreamHandler)
        };

        // Non-Anthropic provider dispatch: if the model is "provider/model"
        // format and the registry has that provider, use it directly.
        //
        // Provider resolution priority:
        //   1. Explicit "provider/model" format in the model string
        //   2. config.provider setting (from --provider flag or settings.json)
        //   3. Model registry lookup (e.g. "gemini-3-flash-preview" → google)
        //   4. Default to "anthropic"
        if let Some(ref registry) = config.provider_registry {
            let (provider_id_str, model_id_str) = if let Some(p) = tool_ctx
                .config
                .provider
                .as_deref()
                .filter(|p| *p != "anthropic")
            {
                // Explicit non-Anthropic provider in config — use it.
                // If the stored model is in canonical "provider/model" form,
                // strip the top-level provider prefix before sending it to the
                // provider adapter. If it contains an additional slash
                // (e.g. "meta-llama/Llama-3.3..." on OpenRouter), preserve it.
                let provider_prefix = format!("{}/", p);
                let model_id = effective_model
                    .strip_prefix(&provider_prefix)
                    .unwrap_or(&effective_model)
                    .to_string();
                (p.to_string(), model_id)
            } else if let Some((p, m)) = effective_model.split_once('/') {
                // No explicit provider but model has "provider/model" format.
                // Check whether `p` is a known provider or just a model
                // namespace (e.g. "meta-llama/Llama-3" on OpenRouter).
                let known_providers = [
                    "anthropic",
                    "openai",
                    "openai-codex",
                    "google",
                    "groq",
                    "mistral",
                    "deepseek",
                    "xai",
                    "cohere",
                    "perplexity",
                    "cerebras",
                    "openrouter",
                    "togetherai",
                    "together-ai",
                    "deepinfra",
                    "venice",
                    "github-copilot",
                    "ollama",
                    "lmstudio",
                    "llamacpp",
                    "azure",
                    "amazon-bedrock",
                    "huggingface",
                    "nvidia",
                    "fireworks",
                    "sambanova",
                ];
                if known_providers.contains(&p) {
                    (p.to_string(), m.to_string())
                } else {
                    // Treat the whole string as the model ID, fall through
                    // to auto-detection below.
                    let fallback_provider =
                        tool_ctx.config.provider.as_deref().unwrap_or("anthropic");
                    (fallback_provider.to_string(), effective_model.clone())
                }
            } else {
                // No explicit provider set (or set to "anthropic"): try the
                // model registry to auto-detect provider from the model name.
                // Use the shared model registry from QueryConfig if available;
                // otherwise construct a temporary one.
                let temp_reg;
                let model_reg: &claurst_api::ModelRegistry =
                    if let Some(ref shared) = config.model_registry {
                        shared
                    } else {
                        temp_reg = {
                            let mut r = claurst_api::ModelRegistry::new();
                            if let Some(cache_dir) = dirs::cache_dir() {
                                let cache_path = cache_dir.join("claurst").join("models_dev.json");
                                r.load_cache(&cache_path);
                            }
                            r
                        };
                        &temp_reg
                    };
                if let Some(detected_pid) = model_reg.find_provider_for_model(&effective_model) {
                    let pid_str = detected_pid.to_string();
                    if pid_str != "anthropic" {
                        (pid_str, effective_model.clone())
                    } else {
                        ("anthropic".to_string(), effective_model.clone())
                    }
                } else {
                    // Fall back to config.provider (may be "anthropic" or None→"anthropic")
                    let p = tool_ctx.config.provider.as_deref().unwrap_or("anthropic");
                    (p.to_string(), effective_model.clone())
                }
            };

            // Dispatch through the provider path for non-Anthropic providers,
            // AND for Anthropic when the pre-built client has no API key
            // (user started without ANTHROPIC_API_KEY but added one via /connect).
            let use_provider_dispatch = if provider_id_str == "openai-codex" {
                false
            } else {
                provider_id_str != "anthropic" || client.api_key_is_empty()
            };

            if use_provider_dispatch {
                let pid = claurst_core::provider_id::ProviderId::new(&provider_id_str);

                // Always prefer a fresh provider built from the auth_store so
                // that keys added at runtime via /connect are picked up
                // immediately — even when the provider was pre-registered at
                // startup with a stale or missing key.
                let runtime_provider =
                    claurst_api::registry::runtime_provider_for(&provider_id_str);

                let mut registry_provider = if runtime_provider.is_some() {
                    // Fresh auth_store key available — use it instead of the
                    // (possibly stale) registry entry.
                    None
                } else {
                    registry.get(&pid).cloned()
                };

                // If the user supplied --api-base for a local provider (Ollama, LM Studio,
                // llama.cpp), rebuild the provider with the override URL.  These providers
                // are always pre-registered with a hardcoded default URL, so without this
                // the --api-base flag would be silently ignored.
                if let Some(override_base) = tool_ctx
                    .config
                    .provider_configs
                    .get(&provider_id_str)
                    .and_then(|pc| pc.api_base.as_deref())
                {
                    use claurst_api::providers::openai_compat_providers;
                    let base_url = format!("{}/v1", override_base.trim_end_matches('/'));
                    let overridden: Option<std::sync::Arc<dyn claurst_api::LlmProvider>> =
                        match provider_id_str.as_str() {
                            "ollama" => Some(std::sync::Arc::new(
                                openai_compat_providers::ollama().with_base_url(base_url),
                            )),
                            "lmstudio" | "lm-studio" => Some(std::sync::Arc::new(
                                openai_compat_providers::lm_studio().with_base_url(base_url),
                            )),
                            "llamacpp" | "llama-cpp" => Some(std::sync::Arc::new(
                                openai_compat_providers::llama_cpp().with_base_url(base_url),
                            )),
                            _ => None,
                        };
                    if overridden.is_some() {
                        registry_provider = overridden;
                    }
                }

                let provider = runtime_provider.or(registry_provider);
                if let Some(provider) = provider {
                    debug!(provider = %provider_id_str, model = %model_id_str, "Dispatching to non-Anthropic provider");

                    // Notify TUI that we're calling the provider
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(QueryEvent::Status(format!(
                            "Calling {} ({})…",
                            provider.name(),
                            model_id_str
                        )));
                    }

                    // Build ProviderRequest from the already-assembled request data.
                    // tools comes from the api_tools we already built above.
                    // Filter unsupported modalities: replace Image/Document blocks
                    // with placeholder text when the provider doesn't support them,
                    // preventing crashes on text-only models.
                    let mut caps = provider.capabilities();
                    if let Some(model_entry) =
                        config.model_registry.as_ref().and_then(|model_registry| {
                            model_registry.get(&provider_id_str, &model_id_str)
                        })
                    {
                        caps.image_input = model_entry.vision;
                        caps.tool_calling = model_entry.tool_calling;
                        caps.thinking = model_entry.reasoning;
                    }
                    let provider_tools: Vec<claurst_core::types::ToolDefinition> =
                        if caps.tool_calling {
                            tools.iter().map(|t| t.to_definition()).collect()
                        } else {
                            Vec::new()
                        };
                    let provider_messages: Vec<claurst_core::types::Message> = messages
                        .iter()
                        .map(|msg| {
                            let mut msg = msg.clone();
                            if let claurst_core::types::MessageContent::Blocks(ref mut blocks) =
                                msg.content
                            {
                                for block in blocks.iter_mut() {
                                    match block {
                                        claurst_core::types::ContentBlock::Image { .. }
                                            if !caps.image_input =>
                                        {
                                            *block = claurst_core::types::ContentBlock::Text {
                                                text: "[Image not supported by this model]"
                                                    .to_string(),
                                            };
                                        }
                                        claurst_core::types::ContentBlock::Document { .. }
                                            if !caps.pdf_input =>
                                        {
                                            *block = claurst_core::types::ContentBlock::Text {
                                                text: "[PDF not supported by this model]"
                                                    .to_string(),
                                            };
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            msg
                        })
                        .collect();

                    let provider_request = claurst_api::ProviderRequest {
                        model: model_id_str.to_owned(),
                        messages: provider_messages,
                        system_prompt: Some(system_for_provider.clone()),
                        tools: provider_tools,
                        max_tokens: config.max_tokens,
                        temperature: effective_temperature.map(|t| t as f64),
                        top_p: None,
                        top_k: None,
                        stop_sequences: vec![],
                        thinking: if caps.thinking {
                            effective_thinking_budget
                                .map(|b| claurst_api::ThinkingConfig::enabled(b))
                        } else {
                            None
                        },
                        provider_options: build_provider_options(
                            &provider_id_str,
                            &model_id_str,
                            config.effort_level,
                            effective_thinking_budget,
                        ),
                    };

                    // Use create_message_stream so the TUI receives real-time
                    // text deltas instead of waiting for the full response.
                    let mut stream = match provider.create_message_stream(provider_request).await {
                        Ok(s) => s,
                        Err(e) => {
                            error!(provider = %provider_id_str, error = %e, "Provider stream failed");
                            return QueryOutcome::Error(claurst_core::error::ClaudeError::Api(
                                e.to_string(),
                            ));
                        }
                    };

                    // Accumulators for building the final assistant message.
                    let mut text_chunks: Vec<String> = Vec::new();
                    // tool_call_blocks: index → (id, name, accumulated_json)
                    let mut tool_call_blocks: std::collections::HashMap<
                        usize,
                        (String, String, String),
                    > = std::collections::HashMap::new();
                    let mut usage = UsageInfo::default();
                    let mut stop_str = "end_turn".to_string();
                    let mut msg_id = uuid::Uuid::new_v4().to_string();

                    use futures::StreamExt as ProviderStreamExt;
                    let provider_stall_timeout = std::time::Duration::from_secs(45);
                    let provider_stall = tokio::time::sleep(provider_stall_timeout);
                    tokio::pin!(provider_stall);
                    let mut provider_stream_stalled = false;

                    loop {
                        tokio::select! {
                            _ = cancel_token.cancelled() => {
                                return QueryOutcome::Cancelled;
                            }
                            _ = &mut provider_stall => {
                                provider_stream_stalled = true;
                                break;
                            }
                            event = stream.next() => {
                                provider_stall.as_mut().reset(tokio::time::Instant::now() + provider_stall_timeout);
                                match event {
                                    None => break,
                                    Some(Err(e)) => {
                                        error!(provider = %provider_id_str, error = %e, "Provider stream error");
                                        break;
                                    }
                                    Some(Ok(evt)) => {
                                        // Forward to TUI via AnthropicStreamEvent mapping.
                                        if let Some(ref tx) = event_tx {
                                            if let Some(ae) = map_to_anthropic_event(&evt) {
                                                let _ = tx.send(QueryEvent::Stream(ae));
                                            }
                                        }

                                        // Accumulate response data.
                                        match &evt {
                                            claurst_api::StreamEvent::MessageStart { id, usage: u, .. } => {
                                                msg_id = id.clone();
                                                usage.input_tokens = u.input_tokens;
                                                usage.cache_read_input_tokens = u.cache_read_input_tokens;
                                                usage.cache_creation_input_tokens = u.cache_creation_input_tokens;
                                            }
                                            claurst_api::StreamEvent::ContentBlockStart { index, content_block } => {
                                                if let ContentBlock::ToolUse { id, name, .. } = content_block {
                                                    tool_call_blocks.insert(*index, (id.clone(), name.clone(), String::new()));
                                                }
                                            }
                                            claurst_api::StreamEvent::TextDelta { text, .. } => {
                                                text_chunks.push(text.clone());
                                            }
                                            claurst_api::StreamEvent::InputJsonDelta { index, partial_json } => {
                                                if let Some((_, _, buf)) = tool_call_blocks.get_mut(index) {
                                                    buf.push_str(partial_json);
                                                }
                                            }
                                            claurst_api::StreamEvent::MessageDelta { stop_reason, usage: u } => {
                                                stop_str = match stop_reason {
                                                    Some(claurst_api::provider_types::StopReason::ToolUse) => "tool_use",
                                                    Some(claurst_api::provider_types::StopReason::MaxTokens) => "max_tokens",
                                                    _ => "end_turn",
                                                }.to_string();
                                                if let Some(u) = u {
                                                    usage.output_tokens = u.output_tokens;
                                                }
                                            }
                                            claurst_api::StreamEvent::MessageStop => break,
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // If the stream stalled (no data for 45s), retry.
                    if provider_stream_stalled && retries_left > 0 {
                        retries_left -= 1;
                        warn!(provider = %provider_id_str, model = %model_id_str, retries_left, "Provider stream stalled — retrying");
                        if let Some(ref tx) = event_tx {
                            let _ = tx.send(QueryEvent::Status(format!(
                                "No response for 45s — retrying ({} left)…",
                                retries_left + 1
                            )));
                        }
                        turn -= 1;
                        continue;
                    }

                    // Build the content blocks from accumulated stream data.
                    let mut content_blocks: Vec<ContentBlock> = Vec::new();

                    let combined_text = text_chunks.join("");
                    if !combined_text.is_empty() {
                        content_blocks.push(ContentBlock::Text {
                            text: combined_text,
                        });
                    }

                    // Reconstruct tool-use blocks (sorted by index for determinism).
                    let mut tc_indices: Vec<usize> = tool_call_blocks.keys().cloned().collect();
                    tc_indices.sort();
                    for idx in tc_indices {
                        if let Some((id, name, json_str)) = tool_call_blocks.remove(&idx) {
                            let input: serde_json::Value =
                                serde_json::from_str(&json_str).unwrap_or(serde_json::json!({}));
                            content_blocks.push(ContentBlock::ToolUse { id, name, input });
                        }
                    }

                    let assistant_msg = Message {
                        role: claurst_core::types::Role::Assistant,
                        content: claurst_core::types::MessageContent::Blocks(
                            content_blocks.clone(),
                        ),
                        uuid: Some(msg_id),
                        cost: None,
                    };

                    cost_tracker.add_usage(
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.cache_creation_input_tokens,
                        usage.cache_read_input_tokens,
                    );

                    messages.push(assistant_msg.clone());

                    // Handle tool-use turn: execute tools and loop.
                    let tool_use_blocks: Vec<_> = content_blocks
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::ToolUse { id, name, input } = b {
                                Some((id.clone(), name.clone(), input.clone()))
                            } else {
                                None
                            }
                        })
                        .collect();

                    // Execute tools if any tool_use blocks were returned.
                    // Note: we check the blocks themselves rather than relying
                    // solely on stop_str == "tool_use" because many OpenAI-
                    // compatible providers (Ollama, LM Studio, etc.) return
                    // finish_reason "stop" even when tool calls are present.
                    if !tool_use_blocks.is_empty() {
                        let mut tool_results = Vec::new();
                        for (tool_id, tool_name, tool_input) in tool_use_blocks {
                            // Notify TUI that a tool is starting (matches Anthropic path).
                            if let Some(ref tx) = event_tx {
                                let _ = tx.send(QueryEvent::ToolStart {
                                    tool_name: tool_name.clone(),
                                    tool_id: tool_id.clone(),
                                    input_json: tool_input.to_string(),
                                });
                            }
                            let result =
                                execute_tool(&*tool_name, &tool_input, tools, &tool_ctx).await;
                            if let Some(ref tx) = event_tx {
                                let _ = tx.send(QueryEvent::ToolEnd {
                                    tool_name: tool_name.clone(),
                                    tool_id: tool_id.clone(),
                                    result: result.content.clone(),
                                    is_error: result.is_error,
                                });
                            }
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: tool_id,
                                content: claurst_core::types::ToolResultContent::Text(
                                    result.content,
                                ),
                                is_error: Some(result.is_error),
                            });
                        }
                        messages.push(Message {
                            role: claurst_core::types::Role::User,
                            content: claurst_core::types::MessageContent::Blocks(tool_results),
                            uuid: None,
                            cost: None,
                        });
                        continue; // loop for next turn
                    }

                    // End turn — notify TUI and return.
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(QueryEvent::TurnComplete {
                            stop_reason: stop_str.clone(),
                            turn,
                            usage: Some(usage.clone()),
                        });
                    }

                    return QueryOutcome::EndTurn {
                        message: assistant_msg,
                        usage,
                    };
                } else if provider_id_str != "anthropic" {
                    // Non-Anthropic provider detected but no API key / credentials
                    // available.  Return a clear error instead of silently falling
                    // through to the Anthropic client.
                    let hint = match provider_id_str.as_str() {
                        "google" => "Set GOOGLE_API_KEY or run `claurst auth login --provider google`.",
                        "openai" => "Set OPENAI_API_KEY or run `claurst auth login --provider openai`.",
                        "groq" => "Set GROQ_API_KEY.",
                        "mistral" => "Set MISTRAL_API_KEY.",
                        "deepseek" => "Set DEEPSEEK_API_KEY.",
                        "xai" => "Set XAI_API_KEY.",
                        "github-copilot" => "Reconnect GitHub Copilot via /connect, or set GITHUB_TOKEN.",
                        "cohere" => "Set COHERE_API_KEY.",
                        _ => "Set the appropriate API key environment variable or use `claurst auth login`.",
                    };
                    error!(
                        provider = %provider_id_str,
                        model = %model_id_str,
                        "No credentials found for provider"
                    );
                    return QueryOutcome::Error(ClaudeError::Api(format!(
                        "No API key for provider '{}' (model '{}'). {}",
                        provider_id_str, model_id_str, hint
                    )));
                }
                // Anthropic with no auth_store key: fall through to the raw
                // client path below (which has its own deferred key validation
                // with detailed model-specific hints).
            }
        }

        // Send to API
        debug!(turn, model = %effective_model, "Sending API request");
        let codex_selected = tool_ctx.config.provider.as_deref() == Some("openai-codex")
            || effective_model.starts_with("openai-codex/");
        let codex_client;
        let active_client = if codex_selected {
            codex_client = match client.with_provider(claurst_api::client::Provider::Codex) {
                Ok(client) => Some(client),
                Err(e) => {
                    error!(error = %e, "Failed to construct Codex client");
                    return QueryOutcome::Error(ClaudeError::Api(e.to_string()));
                }
            };
            codex_client.as_ref().unwrap()
        } else {
            client
        };

        let mut stream_rx = match active_client.create_message_stream(request, handler).await {
            Ok(rx) => rx,
            Err(e) => {
                // On overloaded/rate-limit errors, attempt one switch to the fallback model.
                let err_str = e.to_string().to_lowercase();
                if !used_fallback
                    && (err_str.contains("overloaded")
                        || err_str.contains("529")
                        || err_str.contains("rate_limit"))
                {
                    if let Some(ref fb) = config.fallback_model {
                        warn!(
                            primary = %effective_model,
                            fallback = %fb,
                            "Primary model unavailable — switching to fallback"
                        );
                        if let Some(ref tx) = event_tx {
                            let _ = tx.send(QueryEvent::Status(format!(
                                "Model unavailable — switching to fallback ({})",
                                fb
                            )));
                        }
                        effective_model = fb.clone();
                        used_fallback = true;
                        turn -= 1; // don't count this attempt against max_turns
                        continue;
                    }
                }
                error!(error = %e, "API request failed");
                return QueryOutcome::Error(e);
            }
        };

        // Accumulate the streamed response.
        // A stall timeout auto-retries the request if no data arrives for 45s
        // (some providers are slow; we don't want to give up too early).
        const STALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
        let mut accumulator = StreamAccumulator::new();
        let stall_deadline = tokio::time::sleep(STALL_TIMEOUT);
        tokio::pin!(stall_deadline);

        let stream_stalled = loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    return QueryOutcome::Cancelled;
                }
                _ = &mut stall_deadline => {
                    // No data for 45s — stall detected
                    break true;
                }
                event = stream_rx.recv() => {
                    // Reset stall timer on every received event.
                    stall_deadline.as_mut().reset(tokio::time::Instant::now() + STALL_TIMEOUT);
                    match event {
                        Some(evt) => {
                            accumulator.on_event(&evt);
                            match &evt {
                                AnthropicStreamEvent::Error { error_type, message } => {
                                    if error_type == "overloaded_error" {
                                        warn!(model = %effective_model, "API overloaded");
                                    }
                                    error!(error_type, message, "Stream error");
                                }
                                AnthropicStreamEvent::MessageStop => break false,
                                _ => {}
                            }
                        }
                        None => break false, // Stream ended
                    }
                }
            }
        };

        if stream_stalled && retries_left > 0 {
            retries_left -= 1;
            warn!(model = %effective_model, retries_left, "Stream stalled — retrying request");
            if let Some(ref tx) = event_tx {
                let _ = tx.send(QueryEvent::Status(format!(
                    "No response for 45s — retrying ({} left)…",
                    retries_left + 1
                )));
            }
            turn -= 1; // don't count this stalled attempt
            continue;
        }

        let (assistant_msg, usage, stop_reason) = accumulator.finish();

        // Track costs
        cost_tracker.add_usage(
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_creation_input_tokens,
            usage.cache_read_input_tokens,
        );

        // Budget guard: abort the loop if the configured USD cap is exceeded.
        if let Some(limit) = config.max_budget_usd {
            let spent = cost_tracker.total_cost_usd();
            if spent >= limit {
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(QueryEvent::Status(format!(
                        "Budget limit ${:.4} exceeded (spent ${:.4}) — stopping.",
                        limit, spent
                    )));
                }
                return QueryOutcome::BudgetExceeded {
                    cost_usd: spent,
                    limit_usd: limit,
                };
            }
        }

        // Append assistant message to conversation
        messages.push(assistant_msg.clone());

        let stop = stop_reason.as_deref().unwrap_or("end_turn");

        // T1-3: Fire PostModelTurn hooks after the model samples a response.
        // Hooks can inject blocking errors or veto continuation entirely.
        {
            let hook_result = fire_post_sampling_hooks(&assistant_msg, &tool_ctx.config);
            if !hook_result.blocking_errors.is_empty() {
                if hook_result.prevent_continuation {
                    // Hard veto: push the errors into the conversation and abort.
                    for err_msg in hook_result.blocking_errors {
                        messages.push(err_msg);
                    }
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(QueryEvent::Status(
                            "PostModelTurn hook vetoed continuation.".to_string(),
                        ));
                    }
                    let last = messages
                        .last()
                        .cloned()
                        .unwrap_or_else(|| Message::assistant("Hook blocked continuation."));
                    return QueryOutcome::EndTurn {
                        message: last,
                        usage,
                    };
                }
                // Soft errors: inject them so the model can react next turn.
                for err_msg in hook_result.blocking_errors {
                    debug!("PostModelTurn hook injecting error message");
                    messages.push(err_msg);
                }
            }
        }

        // Emit token warning events when approaching context limits.
        // Thresholds mirror TypeScript autoCompact.ts: 80% → Warning, 95% → Critical.
        {
            let warning_state =
                compact::calculate_token_warning_state(usage.input_tokens, &config.model);
            if warning_state != compact::TokenWarningState::Ok {
                if let Some(ref tx) = event_tx {
                    let window = compact::context_window_for_model(&config.model);
                    let pct_used = usage.input_tokens as f64 / window as f64;
                    let _ = tx.send(QueryEvent::TokenWarning {
                        state: warning_state,
                        pct_used,
                    });
                }
            }
        }

        // Auto-compact: if context is near-full, summarise older messages now
        // (before the next turn's API call would fail with prompt-too-long).
        //
        // Reactive compact (T1-1): when the CLAUDE_REACTIVE_COMPACT feature gate
        // is enabled, we replace the proactive auto-compact path with reactive
        // compact / context-collapse instead. This fires on every streaming turn
        // so it can act before a prompt-too-long error is returned by the API.
        //
        // Feature gate check: CLAURST_FEATURE_REACTIVE_COMPACT=1
        let reactive_compact_enabled =
            claurst_core::feature_gates::is_feature_enabled("reactive_compact");

        if reactive_compact_enabled {
            // Reactive path: emergency collapse takes priority over normal compact.
            let context_limit = compact::context_window_for_model(&config.model);
            if compact::should_context_collapse(usage.input_tokens, context_limit) {
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(QueryEvent::Status(
                        "Compacting context... (emergency collapse)".to_string(),
                    ));
                }
                match compact::context_collapse(std::mem::take(messages), client, config).await {
                    Ok(result) => {
                        *messages = result.messages;
                        info!(
                            tokens_freed = result.tokens_freed,
                            "Context-collapse complete"
                        );
                    }
                    Err(e) => {
                        warn!(error = %e, "Context-collapse failed");
                        // Put messages back on failure (mem::take drained them).
                        // We can't recover them here — re-run auto-compact as fallback.
                    }
                }
            } else if compact::should_compact(usage.input_tokens, context_limit) {
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(QueryEvent::Status("Compacting context...".to_string()));
                }
                match compact::reactive_compact(
                    std::mem::take(messages),
                    client,
                    config,
                    cancel_token.clone(),
                    &[],
                )
                .await
                {
                    Ok(result) => {
                        *messages = result.messages;
                        info!(
                            tokens_freed = result.tokens_freed,
                            "Reactive compact complete"
                        );
                    }
                    Err(claurst_core::error::ClaudeError::Cancelled) => {
                        warn!("Reactive compact was cancelled");
                    }
                    Err(e) => {
                        warn!(error = %e, "Reactive compact failed");
                    }
                }
            }
        } else if stop == "end_turn" || stop == "tool_use" {
            // Proactive auto-compact (original path, used when reactive compact is off).
            if let Some(new_msgs) = compact::auto_compact_if_needed(
                client,
                messages,
                usage.input_tokens,
                &config.model,
                &mut compact_state,
            )
            .await
            {
                *messages = new_msgs;
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(QueryEvent::Status(
                        "Context compacted to stay within limits.".to_string(),
                    ));
                }
            }
        }

        if let Some(ref tx) = event_tx {
            let _ = tx.send(QueryEvent::TurnComplete {
                turn,
                stop_reason: stop.to_string(),
                usage: Some(usage.clone()),
            });
        }

        // Helper closure for firing the Stop hook.
        macro_rules! fire_stop_hook {
            ($msg:expr) => {{
                let stop_ctx = claurst_core::hooks::HookContext {
                    event: "Stop".to_string(),
                    tool_name: None,
                    tool_input: None,
                    tool_output: Some($msg.get_all_text()),
                    is_error: None,
                    session_id: Some(tool_ctx.session_id.clone()),
                };
                claurst_core::hooks::run_hooks(
                    &tool_ctx.config.hooks,
                    claurst_core::config::HookEvent::Stop,
                    &stop_ctx,
                    &tool_ctx.working_dir,
                )
                .await;
            }};
        }

        match stop {
            "end_turn" => {
                fire_stop_hook!(assistant_msg);

                // T1-3: Fire Stop hooks in background (fire-and-forget).
                // `stop_hooks_with_full_behavior` spawns blocking tasks internally
                // and returns immediately with an empty Vec.
                let _bg = stop_hooks_with_full_behavior(
                    &assistant_msg,
                    &tool_ctx.config,
                    tool_ctx.working_dir.clone(),
                );

                // Asynchronously extract and persist session memories if warranted.
                // Runs in a detached Tokio task so it doesn't block the query loop.
                if session_memory::SessionMemoryExtractor::should_extract(messages) {
                    let model_clone = config.model.clone();
                    let messages_clone = messages.clone();
                    let working_dir_clone = tool_ctx.working_dir.clone();

                    // Build a fresh client using the same API key.  This avoids
                    // requiring an Arc in the existing run_query_loop signature.
                    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
                        if !api_key.is_empty() {
                            if let Ok(sm_client) = claurst_api::AnthropicClient::new(
                                claurst_api::client::ClientConfig {
                                    api_key,
                                    ..Default::default()
                                },
                            ) {
                                let sm_client = std::sync::Arc::new(sm_client);
                                tokio::spawn(async move {
                                    let extractor =
                                        session_memory::SessionMemoryExtractor::new(&model_clone);
                                    match extractor
                                        .extract(&messages_clone, &working_dir_clone, &sm_client)
                                        .await
                                    {
                                        Ok(memories) if !memories.is_empty() => {
                                            let target = working_dir_clone
                                                .join(".claurst")
                                                .join("AGENTS.md");
                                            if let Err(e) =
                                                session_memory::SessionMemoryExtractor::persist(
                                                    &memories, &target,
                                                )
                                                .await
                                            {
                                                tracing::warn!(
                                                    error = %e,
                                                    "Failed to persist session memories"
                                                );
                                            }
                                        }
                                        Ok(_) => {} // no memories extracted
                                        Err(e) => {
                                            tracing::debug!(
                                                error = %e,
                                                "Session memory extraction failed (non-fatal)"
                                            );
                                        }
                                    }
                                });
                            }
                        }
                    }
                }

                // Trigger AutoDream consolidation check (non-blocking, best-effort).
                // maybe_trigger() checks gates + acquires lock. If it returns
                // Some(task), we spawn a background subagent via AgentTool so
                // the spawn doesn't call run_query_loop recursively from within
                // its own future (which would make the future !Send).
                {
                    let memory_dir = dirs::home_dir().map(|h| h.join(".claurst").join("memory"));
                    let conversations_dir =
                        dirs::home_dir().map(|h| h.join(".claurst").join("conversations"));
                    if let (Some(mem), Some(conv)) = (memory_dir, conversations_dir) {
                        let dreamer = crate::auto_dream::AutoDream::new(mem, conv);
                        if let Ok(Some(task)) = dreamer.maybe_trigger().await {
                            // Run the consolidation subagent in a background Tokio
                            // task. We use the AgentTool execute path (via
                            // poll_background_agent / BACKGROUND_AGENTS) to avoid
                            // re-entering run_query_loop from within the same
                            // future graph.
                            let agent_input = serde_json::json!({
                                "description": "memory consolidation",
                                "prompt": task.prompt,
                                "max_turns": 20,
                                "system_prompt": "You are performing automatic memory consolidation. Complete the task and return a brief summary.",
                                "run_in_background": true,
                                "isolation": null
                            });
                            let ctx_for_dream = tool_ctx.clone();
                            tokio::spawn(async move {
                                let agent = crate::agent_tool::AgentTool;
                                let _result = claurst_tools::Tool::execute(
                                    &agent,
                                    agent_input,
                                    &ctx_for_dream,
                                )
                                .await;
                                crate::auto_dream::AutoDream::finish_consolidation(&task).await;
                            });
                        }
                    }
                }

                return QueryOutcome::EndTurn {
                    message: assistant_msg,
                    usage,
                };
            }
            "max_tokens" => {
                // Mirror the TS recovery loop: inject a continuation nudge and
                // retry up to MAX_TOKENS_RECOVERY_LIMIT times before surfacing
                // the partial response as QueryOutcome::MaxTokens.
                if max_tokens_recovery_count < MAX_TOKENS_RECOVERY_LIMIT {
                    max_tokens_recovery_count += 1;
                    warn!(
                        attempt = max_tokens_recovery_count,
                        limit = MAX_TOKENS_RECOVERY_LIMIT,
                        "max_tokens hit — injecting continuation message (attempt {}/{})",
                        max_tokens_recovery_count,
                        MAX_TOKENS_RECOVERY_LIMIT,
                    );
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(QueryEvent::Status(format!(
                            "Output token limit hit — continuing (attempt {}/{})",
                            max_tokens_recovery_count, MAX_TOKENS_RECOVERY_LIMIT
                        )));
                    }
                    // The partial assistant message must be in the history so
                    // the continuation makes sense to the model.
                    messages.push(Message::user(MAX_TOKENS_RECOVERY_MSG));
                    continue;
                }
                // Recovery exhausted — surface the partial response.
                warn!(
                    "max_tokens recovery exhausted after {} attempts",
                    MAX_TOKENS_RECOVERY_LIMIT
                );
                return QueryOutcome::MaxTokens {
                    partial_message: assistant_msg,
                    usage,
                };
            }
            "tool_use" => {
                // A completed tool-use turn counts as a successful recovery
                // boundary; reset the max_tokens retry counter.
                max_tokens_recovery_count = 0;
                // Extract tool calls and execute them
                let tool_blocks = assistant_msg.get_tool_use_blocks();
                if tool_blocks.is_empty() {
                    // Shouldn't happen but treat as end_turn
                    return QueryOutcome::EndTurn {
                        message: assistant_msg,
                        usage,
                    };
                }

                // ---------------------------------------------------------------------------
                // Streaming tool executor: parallel non-agent tool dispatch.
                //
                // Phase 1: Run PreToolUse hooks sequentially (they can block/deny execution
                //          and may display interactive permission dialogs).
                // Phase 2: Dispatch all non-blocked tool executions concurrently via
                //          futures::future::join_all, preserving original order.
                // Phase 3: Fire PostToolUse hooks + emit events, then collect results.
                //
                // This mirrors the TypeScript StreamingToolExecutor pattern.
                // ---------------------------------------------------------------------------

                // Intermediate record produced during Phase 1.
                struct PreparedTool {
                    id: String,
                    name: String,
                    input: Value,
                    /// None means the pre-hook blocked execution; the String is the error reason.
                    blocked_result: Option<ToolResult>,
                }

                // Phase 1: sequential pre-hook pass.
                let mut prepared: Vec<PreparedTool> = Vec::with_capacity(tool_blocks.len());
                for block in tool_blocks {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        // Clone from the references returned by get_tool_use_blocks()
                        let id = id.clone();
                        let name = name.clone();
                        let input = input.clone();

                        if let Some(ref tx) = event_tx {
                            let _ = tx.send(QueryEvent::ToolStart {
                                tool_name: name.clone(),
                                tool_id: id.clone(),
                                input_json: input.to_string(),
                            });
                        }

                        let hooks = &tool_ctx.config.hooks;
                        let hook_ctx = claurst_core::hooks::HookContext {
                            event: "PreToolUse".to_string(),
                            tool_name: Some(name.clone()),
                            tool_input: Some(input.clone()),
                            tool_output: None,
                            is_error: None,
                            session_id: Some(tool_ctx.session_id.clone()),
                        };
                        let pre_outcome = claurst_core::hooks::run_hooks(
                            hooks,
                            claurst_core::config::HookEvent::PreToolUse,
                            &hook_ctx,
                            &tool_ctx.working_dir,
                        )
                        .await;

                        let plugin_pre_outcome =
                            claurst_plugins::run_global_pre_tool_hook(&name, &input);

                        let blocked_result = if let claurst_core::hooks::HookOutcome::Blocked(
                            reason,
                        ) = pre_outcome
                        {
                            warn!(tool = %name, reason = %reason, "PreToolUse hook blocked execution");
                            Some(claurst_tools::ToolResult::error(format!(
                                "Blocked by hook: {}",
                                reason
                            )))
                        } else if let claurst_plugins::HookOutcome::Deny(reason) =
                            plugin_pre_outcome
                        {
                            warn!(tool = %name, reason = %reason, "Plugin PreToolUse hook blocked execution");
                            Some(claurst_tools::ToolResult::error(format!(
                                "Blocked by plugin hook: {}",
                                reason
                            )))
                        } else {
                            None
                        };

                        prepared.push(PreparedTool {
                            id,
                            name,
                            input,
                            blocked_result,
                        });
                    }
                }

                // Phase 2: build execution futures for non-blocked tools and join them.
                // Blocked tools yield a ready future with the pre-computed error result.
                // Non-blocked tools execute concurrently via join_all.
                // Each async block owns its cloned name/input so there are no lifetime issues.
                let exec_futures: Vec<_> = prepared
                    .iter()
                    .map(|p| {
                        if p.blocked_result.is_some() {
                            let r = p.blocked_result.clone().unwrap();
                            futures::future::Either::Left(async move { r })
                        } else {
                            let name = p.name.clone();
                            let input = p.input.clone();
                            futures::future::Either::Right(async move {
                                execute_tool(&name, &input, tools, tool_ctx).await
                            })
                        }
                    })
                    .collect();

                // Run all tool futures concurrently; join_all preserves order.
                let exec_results: Vec<ToolResult> = futures::future::join_all(exec_futures).await;

                // Phase 3: post-hooks, event emission, and result block assembly.
                let mut result_blocks: Vec<ContentBlock> = Vec::with_capacity(prepared.len());
                for (p, result) in prepared.iter().zip(exec_results.into_iter()) {
                    let hooks = &tool_ctx.config.hooks;
                    let post_ctx = claurst_core::hooks::HookContext {
                        event: "PostToolUse".to_string(),
                        tool_name: Some(p.name.clone()),
                        tool_input: Some(p.input.clone()),
                        tool_output: Some(result.content.clone()),
                        is_error: Some(result.is_error),
                        session_id: Some(tool_ctx.session_id.clone()),
                    };
                    claurst_core::hooks::run_hooks(
                        hooks,
                        claurst_core::config::HookEvent::PostToolUse,
                        &post_ctx,
                        &tool_ctx.working_dir,
                    )
                    .await;

                    claurst_plugins::run_global_post_tool_hook(
                        &p.name,
                        &p.input,
                        &result.content,
                        result.is_error,
                    );

                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(QueryEvent::ToolEnd {
                            tool_name: p.name.clone(),
                            tool_id: p.id.clone(),
                            result: result.content.clone(),
                            is_error: result.is_error,
                        });
                    }

                    result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: p.id.clone(),
                        content: ToolResultContent::Text(result.content),
                        is_error: if result.is_error { Some(true) } else { None },
                    });
                }

                // Append tool results as a user message
                messages.push(Message::user_blocks(result_blocks));

                // Continue the loop to send results back to the model
                continue;
            }
            "stop_sequence" => {
                fire_stop_hook!(assistant_msg);
                let _bg = stop_hooks_with_full_behavior(
                    &assistant_msg,
                    &tool_ctx.config,
                    tool_ctx.working_dir.clone(),
                );
                return QueryOutcome::EndTurn {
                    message: assistant_msg,
                    usage,
                };
            }
            other => {
                warn!(
                    stop_reason = other,
                    "Unknown stop reason, treating as end_turn"
                );
                fire_stop_hook!(assistant_msg);
                let _bg = stop_hooks_with_full_behavior(
                    &assistant_msg,
                    &tool_ctx.config,
                    tool_ctx.working_dir.clone(),
                );
                return QueryOutcome::EndTurn {
                    message: assistant_msg,
                    usage,
                };
            }
        }
    }
}

/// Execute a single tool invocation.
async fn execute_tool(
    name: &str,
    input: &Value,
    tools: &[Box<dyn Tool>],
    ctx: &ToolContext,
) -> ToolResult {
    let tool = tools.iter().find(|t| t.name() == name);

    match tool {
        Some(tool) => {
            debug!(tool = name, "Executing tool");
            tool.execute(input.clone(), ctx).await
        }
        None => {
            warn!(tool = name, "Unknown tool requested");
            ToolResult::error(format!("Unknown tool: {}", name))
        }
    }
}

/// Load persisted todos for `session_id` and return a nudge string if any are
/// incomplete (status != "completed"). Returns empty string otherwise.
fn build_todo_nudge(session_id: &str) -> String {
    let todos = claurst_tools::todo_write::load_todos(session_id);
    let incomplete_count = todos
        .iter()
        .filter(|t| t["status"].as_str().map_or(true, |s| s != "completed"))
        .count();
    if incomplete_count == 0 {
        String::new()
    } else {
        format!(
            "You have {} incomplete task{} in your TodoWrite list. \
             Make sure to complete all tasks before ending your response.",
            incomplete_count,
            if incomplete_count == 1 { "" } else { "s" }
        )
    }
}

/// Build the system prompt from config.
///
/// Delegates to `claurst_core::system_prompt::build_system_prompt` so that all
/// default content (capabilities, safety guidelines, dynamic-boundary marker,
/// etc.) is assembled in one place.  The `QueryConfig` fields map directly to
/// `SystemPromptOptions`:
///
/// - `system_prompt`        → `custom_system_prompt` (added to cacheable block)
/// - `append_system_prompt` → `append_system_prompt` (added after boundary)
fn build_system_prompt(config: &QueryConfig) -> SystemPrompt {
    use claurst_core::system_prompt::SystemPromptOptions;

    let opts = SystemPromptOptions {
        custom_system_prompt: config.system_prompt.clone(),
        append_system_prompt: config.append_system_prompt.clone(),
        // All other fields use sensible defaults:
        // - prefix:                auto-detect from env
        // - memory_content:        empty (callers inject via append if needed)
        // - replace_system_prompt: false (additive mode)
        // - coordinator_mode:      false
        output_style: config.output_style,
        custom_output_style_prompt: config.output_style_prompt.clone(),
        working_directory: config.working_directory.clone(),
        ..Default::default()
    };

    let text = claurst_core::system_prompt::build_system_prompt(&opts);
    SystemPrompt::Text(text)
}

// ---------------------------------------------------------------------------
// Provider stream event mapping
// ---------------------------------------------------------------------------

/// Map a unified `StreamEvent` (from a non-Anthropic provider) onto the
/// equivalent `AnthropicStreamEvent` so that the TUI stream consumer sees a
/// single, consistent event type regardless of which provider produced it.
fn map_to_anthropic_event(
    evt: &claurst_api::StreamEvent,
) -> Option<claurst_api::AnthropicStreamEvent> {
    use claurst_api::streaming::{AnthropicStreamEvent, ContentDelta};
    use claurst_api::StreamEvent;

    match evt {
        StreamEvent::MessageStart { id, model, usage } => {
            Some(AnthropicStreamEvent::MessageStart {
                id: id.clone(),
                model: model.clone(),
                usage: usage.clone(),
            })
        }
        StreamEvent::ContentBlockStart {
            index,
            content_block,
        } => Some(AnthropicStreamEvent::ContentBlockStart {
            index: *index,
            content_block: content_block.clone(),
        }),
        StreamEvent::TextDelta { index, text } => Some(AnthropicStreamEvent::ContentBlockDelta {
            index: *index,
            delta: ContentDelta::TextDelta { text: text.clone() },
        }),
        StreamEvent::ThinkingDelta { index, thinking } => {
            Some(AnthropicStreamEvent::ContentBlockDelta {
                index: *index,
                delta: ContentDelta::ThinkingDelta {
                    thinking: thinking.clone(),
                },
            })
        }
        StreamEvent::ReasoningDelta { index, reasoning } => {
            Some(AnthropicStreamEvent::ContentBlockDelta {
                index: *index,
                delta: ContentDelta::ThinkingDelta {
                    thinking: reasoning.clone(),
                },
            })
        }
        StreamEvent::InputJsonDelta {
            index,
            partial_json,
        } => Some(AnthropicStreamEvent::ContentBlockDelta {
            index: *index,
            delta: ContentDelta::InputJsonDelta {
                partial_json: partial_json.clone(),
            },
        }),
        StreamEvent::SignatureDelta { index, signature } => {
            Some(AnthropicStreamEvent::ContentBlockDelta {
                index: *index,
                delta: ContentDelta::SignatureDelta {
                    signature: signature.clone(),
                },
            })
        }
        StreamEvent::ContentBlockStop { index } => {
            Some(AnthropicStreamEvent::ContentBlockStop { index: *index })
        }
        StreamEvent::MessageDelta { stop_reason, usage } => {
            // Convert the unified StopReason to the string form used by
            // AnthropicStreamEvent::MessageDelta.
            let stop_reason_str = stop_reason.as_ref().map(|r| match r {
                claurst_api::provider_types::StopReason::ToolUse => "tool_use".to_string(),
                claurst_api::provider_types::StopReason::MaxTokens => "max_tokens".to_string(),
                claurst_api::provider_types::StopReason::StopSequence => {
                    "stop_sequence".to_string()
                }
                claurst_api::provider_types::StopReason::EndTurn => "end_turn".to_string(),
                claurst_api::provider_types::StopReason::ContentFiltered => {
                    "content_filtered".to_string()
                }
                claurst_api::provider_types::StopReason::Other(s) => s.clone(),
            });
            Some(AnthropicStreamEvent::MessageDelta {
                stop_reason: stop_reason_str,
                usage: usage.clone(),
            })
        }
        StreamEvent::MessageStop => Some(AnthropicStreamEvent::MessageStop),
        StreamEvent::Error {
            error_type,
            message,
        } => Some(AnthropicStreamEvent::Error {
            error_type: error_type.clone(),
            message: message.clone(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claurst_api::SystemPrompt;

    fn make_config(sys: Option<&str>, append: Option<&str>) -> QueryConfig {
        QueryConfig {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 4096,
            max_turns: 10,
            system_prompt: sys.map(String::from),
            append_system_prompt: append.map(String::from),
            output_style: claurst_core::system_prompt::OutputStyle::Default,
            output_style_prompt: None,
            working_directory: None,
            thinking_budget: None,
            temperature: None,
            tool_result_budget: 50_000,
            effort_level: None,
            command_queue: None,
            skill_index: None,
            max_budget_usd: None,
            fallback_model: None,
            provider_registry: None,
            agent_name: None,
            agent_definition: None,
            model_registry: None,
        }
    }

    // ---- build_system_prompt tests ------------------------------------------

    #[test]
    fn test_system_prompt_default_when_empty() {
        // The default prompt (no custom system prompt set) should include the
        // Claurst attribution and standard sections.
        let cfg = make_config(None, None);
        let prompt = build_system_prompt(&cfg);
        if let SystemPrompt::Text(text) = prompt {
            assert!(
                text.contains("Claurst") || text.contains("Claude agent"),
                "Default prompt should contain attribution: {}",
                text
            );
            assert!(
                text.contains(claurst_core::system_prompt::SYSTEM_PROMPT_DYNAMIC_BOUNDARY),
                "Default prompt must contain the dynamic boundary marker"
            );
        } else {
            panic!("Expected SystemPrompt::Text");
        }
    }

    #[test]
    fn test_system_prompt_with_custom() {
        // A custom system prompt is injected into the cacheable section as
        // <custom_instructions>; the default sections are still present.
        let cfg = make_config(Some("You are a code reviewer."), None);
        let prompt = build_system_prompt(&cfg);
        if let SystemPrompt::Text(text) = prompt {
            assert!(
                text.contains("You are a code reviewer."),
                "Custom prompt text should appear in the output"
            );
            assert!(
                text.contains("Claurst") || text.contains("Claude agent"),
                "Default attribution should still be present"
            );
        } else {
            panic!("Expected SystemPrompt::Text");
        }
    }

    #[test]
    fn test_system_prompt_with_append() {
        // Appended text lands after the dynamic boundary.
        let cfg = make_config(Some("Base prompt."), Some("Additional context."));
        let prompt = build_system_prompt(&cfg);
        if let SystemPrompt::Text(text) = prompt {
            assert!(text.contains("Base prompt."));
            assert!(text.contains("Additional context."));
            // append_system_prompt appears after the boundary
            let boundary_pos = text
                .find(claurst_core::system_prompt::SYSTEM_PROMPT_DYNAMIC_BOUNDARY)
                .expect("boundary must exist");
            let append_pos = text.find("Additional context.").unwrap();
            assert!(
                append_pos > boundary_pos,
                "Appended text must appear after the dynamic boundary"
            );
        } else {
            panic!("Expected SystemPrompt::Text");
        }
    }

    #[test]
    fn test_system_prompt_append_only() {
        // When only append is set, default sections are present plus the
        // appended text after the dynamic boundary.
        let cfg = make_config(None, Some("Appended text."));
        let prompt = build_system_prompt(&cfg);
        if let SystemPrompt::Text(text) = prompt {
            assert!(
                text.contains("Appended text."),
                "Appended text must appear in the prompt"
            );
            let boundary_pos = text
                .find(claurst_core::system_prompt::SYSTEM_PROMPT_DYNAMIC_BOUNDARY)
                .expect("boundary must exist");
            let append_pos = text.find("Appended text.").unwrap();
            assert!(
                append_pos > boundary_pos,
                "Appended text must appear after the dynamic boundary"
            );
        } else {
            panic!("Expected SystemPrompt::Text");
        }
    }

    #[test]
    fn test_system_prompt_with_custom_output_style_prompt() {
        let mut cfg = make_config(None, None);
        cfg.output_style_prompt = Some("Answer like a pirate.".to_string());
        let prompt = build_system_prompt(&cfg);
        if let SystemPrompt::Text(text) = prompt {
            assert!(text.contains("Answer like a pirate."));
        } else {
            panic!("Expected SystemPrompt::Text");
        }
    }

    // ---- QueryConfig tests --------------------------------------------------

    #[test]
    fn test_query_config_clone() {
        let cfg = make_config(Some("test"), Some("append"));
        let cloned = cfg.clone();
        assert_eq!(cloned.model, "claude-sonnet-4-6");
        assert_eq!(cloned.max_tokens, 4096);
        assert_eq!(cloned.system_prompt, Some("test".to_string()));
    }

    // ---- QueryOutcome variant tests -----------------------------------------

    #[test]
    fn test_query_outcome_debug() {
        // Ensure the enum variants can be created and debug-formatted
        let outcome = QueryOutcome::Cancelled;
        let s = format!("{:?}", outcome);
        assert!(s.contains("Cancelled"));

        let err_outcome = QueryOutcome::Error(claurst_core::error::ClaudeError::RateLimit);
        let s2 = format!("{:?}", err_outcome);
        assert!(s2.contains("Error"));
    }

    #[test]
    fn test_build_provider_options_for_google_gemini_3() {
        let options = build_provider_options(
            "google",
            "gemini-3-flash-preview",
            Some(claurst_core::effort::EffortLevel::High),
            None,
        );
        assert_eq!(
            options["thinkingConfig"]["thinkingLevel"],
            serde_json::json!("high")
        );
        assert_eq!(
            options["thinkingConfig"]["includeThoughts"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn test_build_provider_options_for_openrouter_gpt5() {
        let options = build_provider_options(
            "openrouter",
            "gpt-5.4",
            Some(claurst_core::effort::EffortLevel::Medium),
            None,
        );
        assert_eq!(options["reasoningEffort"], serde_json::json!("medium"));
        assert_eq!(options["textVerbosity"], serde_json::json!("low"));
        assert_eq!(options["usage"]["include"], serde_json::json!(true));
    }

    #[test]
    fn test_build_provider_options_for_bedrock_anthropic() {
        let options = build_provider_options(
            "amazon-bedrock",
            "anthropic.claude-sonnet-4-6-v1",
            Some(claurst_core::effort::EffortLevel::High),
            Some(10_000),
        );
        assert_eq!(
            options["reasoningConfig"]["budgetTokens"],
            serde_json::json!(10_000)
        );
    }
}

/// Stream handler that forwards events to an unbounded channel.
struct ChannelStreamHandler {
    tx: mpsc::UnboundedSender<QueryEvent>,
}

impl StreamHandler for ChannelStreamHandler {
    fn on_event(&self, event: &AnthropicStreamEvent) {
        let _ = self.tx.send(QueryEvent::Stream(event.clone()));
    }
}

// ---------------------------------------------------------------------------
// Single-shot query (non-looping, for simple one-off calls)
// ---------------------------------------------------------------------------

/// Run a single (non-agentic) query – no tool loop, just one API call.
pub async fn run_single_query(
    client: &claurst_api::AnthropicClient,
    messages: Vec<Message>,
    config: &QueryConfig,
) -> Result<Message, ClaudeError> {
    let api_messages: Vec<ApiMessage> = messages.iter().map(ApiMessage::from).collect();
    let system = build_system_prompt(config);

    let request = CreateMessageRequest::builder(&config.model, config.max_tokens)
        .messages(api_messages)
        .system(system)
        .build();

    let handler: Arc<dyn StreamHandler> = Arc::new(claurst_api::streaming::NullStreamHandler);

    let mut rx = client.create_message_stream(request, handler).await?;
    let mut acc = StreamAccumulator::new();

    while let Some(evt) = rx.recv().await {
        acc.on_event(&evt);
        if matches!(evt, AnthropicStreamEvent::MessageStop) {
            break;
        }
    }

    let (msg, _usage, _stop) = acc.finish();
    Ok(msg)
}
