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
pub mod compact;
pub mod coordinator;
pub mod cron_scheduler;
pub use agent_tool::AgentTool;
pub use cron_scheduler::start_cron_scheduler;
pub use compact::{
    AutoCompactState, TokenWarningState, auto_compact_if_needed, calculate_token_warning_state,
    compact_conversation, context_window_for_model, should_auto_compact,
};

use cc_api::{
    ApiMessage, ApiToolDefinition, CreateMessageRequest, StreamAccumulator, StreamEvent,
    StreamHandler, SystemPrompt, ThinkingConfig,
};
use cc_core::config::Config;
use cc_core::cost::CostTracker;
use cc_core::error::ClaudeError;
use cc_core::types::{ContentBlock, Message, ToolResultContent, UsageInfo};
use cc_tools::{Tool, ToolContext, ToolResult};
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
    MaxTokens { partial_message: Message, usage: UsageInfo },
    /// The conversation was cancelled by the user.
    Cancelled,
    /// An unrecoverable error occurred.
    Error(ClaudeError),
}

/// Configuration for a single query-loop invocation.
#[derive(Clone)]
pub struct QueryConfig {
    pub model: String,
    pub max_tokens: u32,
    pub max_turns: u32,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub output_style: cc_core::system_prompt::OutputStyle,
    pub output_style_prompt: Option<String>,
    pub working_directory: Option<String>,
    pub thinking_budget: Option<u32>,
    pub temperature: Option<f32>,
}

impl Default for QueryConfig {
    fn default() -> Self {
        Self {
            model: cc_core::constants::DEFAULT_MODEL.to_string(),
            max_tokens: cc_core::constants::DEFAULT_MAX_TOKENS,
            max_turns: cc_core::constants::MAX_TURNS_DEFAULT,
            system_prompt: None,
            append_system_prompt: None,
            output_style: cc_core::system_prompt::OutputStyle::Default,
            output_style_prompt: None,
            working_directory: None,
            thinking_budget: None,
            temperature: None,
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
            working_directory: cfg
                .project_dir
                .as_ref()
                .map(|p| p.display().to_string()),
            ..Default::default()
        }
    }
}

/// Events emitted by the query loop for the TUI to render.
#[derive(Debug, Clone)]
pub enum QueryEvent {
    /// A stream event from the API.
    Stream(StreamEvent),
    /// A tool is about to be executed.
    ToolStart { tool_name: String, tool_id: String },
    /// A tool has finished executing.
    ToolEnd { tool_name: String, tool_id: String, result: String, is_error: bool },
    /// The model finished a turn.
    TurnComplete { turn: u32, stop_reason: String },
    /// An informational status message.
    Status(String),
    /// An error.
    Error(String),
}

// ---------------------------------------------------------------------------
// Query loop
// ---------------------------------------------------------------------------

/// Run the agentic query loop.
///
/// This sends the conversation to the API, handles tool calls in a loop, and
/// returns when the model issues an end_turn or an error/limit is hit.
pub async fn run_query_loop(
    client: &(dyn cc_api::LlmClient + Send + Sync),
    messages: &mut Vec<Message>,
    tools: &[Box<dyn Tool>],
    tool_ctx: &ToolContext,
    config: &QueryConfig,
    cost_tracker: Arc<CostTracker>,
    event_tx: Option<mpsc::UnboundedSender<QueryEvent>>,
    cancel_token: tokio_util::sync::CancellationToken,
) -> QueryOutcome {
    let mut turn = 0u32;
    let mut compact_state = compact::AutoCompactState::default();

    loop {
        turn += 1;
        if turn > config.max_turns {
            info!(turns = turn, "Max turns reached");
            if let Some(ref tx) = event_tx {
                let _ = tx.send(QueryEvent::Status(format!(
                    "Reached maximum turn limit ({})",
                    config.max_turns
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

        // Build API request
        let api_messages: Vec<ApiMessage> = messages.iter().map(ApiMessage::from).collect();
        let api_tools: Vec<ApiToolDefinition> = tools
            .iter()
            .map(|t| ApiToolDefinition::from(&t.to_definition()))
            .collect();

        let system = build_system_prompt(config);

        let mut req_builder = CreateMessageRequest::builder(&config.model, config.max_tokens)
            .messages(api_messages)
            .system(system)
            .tools(api_tools);

        // Only enable extended thinking if an explicit budget was provided.
        if let Some(budget) = config.thinking_budget {
            req_builder = req_builder.thinking(ThinkingConfig::enabled(budget));
        }

        let request = req_builder.build();

        // Create a stream handler that forwards to the event channel
        let handler: Arc<dyn StreamHandler> = if let Some(ref tx) = event_tx {
            let tx = tx.clone();
            Arc::new(ChannelStreamHandler { tx })
        } else {
            Arc::new(cc_api::streaming::NullStreamHandler)
        };

        // Send to API
        debug!(turn, model = %config.model, "Sending API request");
        let mut stream_rx = match client.create_message_stream(request, handler).await {
            Ok(rx) => rx,
            Err(e) => {
                error!(error = %e, "API request failed");
                return QueryOutcome::Error(e);
            }
        };

        // Accumulate the streamed response
        let mut accumulator = StreamAccumulator::new();

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    return QueryOutcome::Cancelled;
                }
                event = stream_rx.recv() => {
                    match event {
                        Some(evt) => {
                            accumulator.on_event(&evt);
                            match &evt {
                                StreamEvent::Error { error_type, message } => {
                                    if error_type == "overloaded_error" {
                                        warn!("API overloaded, should retry");
                                    }
                                    error!(error_type, message, "Stream error");
                                }
                                StreamEvent::MessageStop => break,
                                _ => {}
                            }
                        }
                        None => break, // Stream ended
                    }
                }
            }
        }

        let (assistant_msg, usage, stop_reason) = accumulator.finish();

        // Track costs
        cost_tracker.add_usage(
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_creation_input_tokens,
            usage.cache_read_input_tokens,
        );

        // Append assistant message to conversation
        messages.push(assistant_msg.clone());

        let stop = stop_reason.as_deref().unwrap_or("end_turn");

        // Auto-compact: if context is near-full, summarise older messages now
        // (before the next turn's API call would fail with prompt-too-long).
        if stop == "end_turn" || stop == "tool_use" {
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
            });
        }

        // Helper closure for firing the Stop hook.
        macro_rules! fire_stop_hook {
            ($msg:expr) => {{
                let stop_ctx = cc_core::hooks::HookContext {
                    event: "Stop".to_string(),
                    tool_name: None,
                    tool_input: None,
                    tool_output: Some($msg.get_all_text()),
                    is_error: None,
                    session_id: Some(tool_ctx.session_id.clone()),
                };
                cc_core::hooks::run_hooks(
                    &tool_ctx.config.hooks,
                    cc_core::config::HookEvent::Stop,
                    &stop_ctx,
                    &tool_ctx.working_dir,
                )
                .await;
            }};
        }

        match stop {
            "end_turn" => {
                fire_stop_hook!(assistant_msg);
                return QueryOutcome::EndTurn {
                    message: assistant_msg,
                    usage,
                };
            }
            "max_tokens" => {
                return QueryOutcome::MaxTokens {
                    partial_message: assistant_msg,
                    usage,
                };
            }
            "tool_use" => {
                // Extract tool calls and execute them
                let tool_blocks = assistant_msg.get_tool_use_blocks();
                if tool_blocks.is_empty() {
                    // Shouldn't happen but treat as end_turn
                    return QueryOutcome::EndTurn {
                        message: assistant_msg,
                        usage,
                    };
                }

                let mut result_blocks: Vec<ContentBlock> = Vec::new();

                for block in tool_blocks {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        if let Some(ref tx) = event_tx {
                            let _ = tx.send(QueryEvent::ToolStart {
                                tool_name: name.clone(),
                                tool_id: id.clone(),
                            });
                        }

                        // Fire PreToolUse hooks (blocking hooks can cancel execution)
                        let hooks = &tool_ctx.config.hooks;
                        let hook_ctx = cc_core::hooks::HookContext {
                            event: "PreToolUse".to_string(),
                            tool_name: Some(name.clone()),
                            tool_input: Some(input.clone()),
                            tool_output: None,
                            is_error: None,
                            session_id: Some(tool_ctx.session_id.clone()),
                        };
                        let pre_outcome = cc_core::hooks::run_hooks(
                            hooks,
                            cc_core::config::HookEvent::PreToolUse,
                            &hook_ctx,
                            &tool_ctx.working_dir,
                        )
                        .await;

                        let result = if let cc_core::hooks::HookOutcome::Blocked(reason) = pre_outcome {
                            warn!(tool = name, reason = %reason, "PreToolUse hook blocked execution");
                            cc_tools::ToolResult::error(format!("Blocked by hook: {}", reason))
                        } else {
                            execute_tool(&name, &input, tools, tool_ctx).await
                        };

                        // Fire PostToolUse hooks
                        let post_ctx = cc_core::hooks::HookContext {
                            event: "PostToolUse".to_string(),
                            tool_name: Some(name.clone()),
                            tool_input: Some(input.clone()),
                            tool_output: Some(result.content.clone()),
                            is_error: Some(result.is_error),
                            session_id: Some(tool_ctx.session_id.clone()),
                        };
                        cc_core::hooks::run_hooks(
                            hooks,
                            cc_core::config::HookEvent::PostToolUse,
                            &post_ctx,
                            &tool_ctx.working_dir,
                        )
                        .await;

                        if let Some(ref tx) = event_tx {
                            let _ = tx.send(QueryEvent::ToolEnd {
                                tool_name: name.clone(),
                                tool_id: id.clone(),
                                result: result.content.clone(),
                                is_error: result.is_error,
                            });
                        }

                        result_blocks.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: ToolResultContent::Text(result.content),
                            is_error: if result.is_error { Some(true) } else { None },
                        });
                    }
                }

                // Append tool results as a user message
                messages.push(Message::user_blocks(result_blocks));

                // Continue the loop to send results back to the model
                continue;
            }
            "stop_sequence" => {
                fire_stop_hook!(assistant_msg);
                return QueryOutcome::EndTurn {
                    message: assistant_msg,
                    usage,
                };
            }
            other => {
                warn!(stop_reason = other, "Unknown stop reason, treating as end_turn");
                fire_stop_hook!(assistant_msg);
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

/// Build the system prompt from config.
///
/// Delegates to `cc_core::system_prompt::build_system_prompt` so that all
/// default content (capabilities, safety guidelines, dynamic-boundary marker,
/// etc.) is assembled in one place.  The `QueryConfig` fields map directly to
/// `SystemPromptOptions`:
///
/// - `system_prompt`        → `custom_system_prompt` (added to cacheable block)
/// - `append_system_prompt` → `append_system_prompt` (added after boundary)
fn build_system_prompt(config: &QueryConfig) -> SystemPrompt {
    use cc_core::system_prompt::SystemPromptOptions;

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

    let text = cc_core::system_prompt::build_system_prompt(&opts);
    SystemPrompt::Text(text)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cc_api::SystemPrompt;

    fn make_config(sys: Option<&str>, append: Option<&str>) -> QueryConfig {
        QueryConfig {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 4096,
            max_turns: 10,
            system_prompt: sys.map(String::from),
            append_system_prompt: append.map(String::from),
            output_style: cc_core::system_prompt::OutputStyle::Default,
            output_style_prompt: None,
            working_directory: None,
            thinking_budget: None,
            temperature: None,
        }
    }

    // ---- build_system_prompt tests ------------------------------------------

    #[test]
    fn test_system_prompt_default_when_empty() {
        // The default prompt (no custom system prompt set) should include the
        // Claude Code attribution and standard sections.
        let cfg = make_config(None, None);
        let prompt = build_system_prompt(&cfg);
        if let SystemPrompt::Text(text) = prompt {
            assert!(
                text.contains("Claude Code") || text.contains("Claude agent"),
                "Default prompt should contain attribution: {}",
                text
            );
            assert!(
                text.contains(cc_core::system_prompt::SYSTEM_PROMPT_DYNAMIC_BOUNDARY),
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
                text.contains("Claude Code") || text.contains("Claude agent"),
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
                .find(cc_core::system_prompt::SYSTEM_PROMPT_DYNAMIC_BOUNDARY)
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
                .find(cc_core::system_prompt::SYSTEM_PROMPT_DYNAMIC_BOUNDARY)
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

        let err_outcome = QueryOutcome::Error(cc_core::error::ClaudeError::RateLimit);
        let s2 = format!("{:?}", err_outcome);
        assert!(s2.contains("Error"));
    }
}

/// Stream handler that forwards events to an unbounded channel.
struct ChannelStreamHandler {
    tx: mpsc::UnboundedSender<QueryEvent>,
}

impl StreamHandler for ChannelStreamHandler {
    fn on_event(&self, event: &StreamEvent) {
        let _ = self.tx.send(QueryEvent::Stream(event.clone()));
    }
}

// ---------------------------------------------------------------------------
// Single-shot query (non-looping, for simple one-off calls)
// ---------------------------------------------------------------------------

/// Run a single (non-agentic) query – no tool loop, just one API call.
pub async fn run_single_query(
    client: &(dyn cc_api::LlmClient + Send + Sync),
    messages: Vec<Message>,
    config: &QueryConfig,
) -> Result<Message, ClaudeError> {
    let api_messages: Vec<ApiMessage> = messages.iter().map(ApiMessage::from).collect();
    let system = build_system_prompt(config);

    let request = CreateMessageRequest::builder(&config.model, config.max_tokens)
        .messages(api_messages)
        .system(system)
        .build();

    let handler: Arc<dyn StreamHandler> = Arc::new(cc_api::streaming::NullStreamHandler);

    let mut rx = client.create_message_stream(request, handler).await?;
    let mut acc = StreamAccumulator::new();

    while let Some(evt) = rx.recv().await {
        acc.on_event(&evt);
        if matches!(evt, StreamEvent::MessageStop) {
            break;
        }
    }

    let (msg, _usage, _stop) = acc.finish();
    Ok(msg)
}
