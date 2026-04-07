// Team tools: create and disband multi-agent swarm teams.
//
// TeamCreateTool — create a named team, run N AgentTool sub-agents in parallel
//                  via the globally-injected AgentRunner, and return aggregated
//                  results from every agent.
// TeamDeleteTool — cancel / clean up a named team.
//
// Architecture note
// -----------------
// cc-tools cannot depend on cc-query (that would be circular: cc-query already
// depends on cc-tools).  We therefore use a dependency-injection pattern:
//
//   1. cc-tools exposes `register_agent_runner(f)` which stores a callable in a
//      process-global slot.
//   2. cc-query calls `register_agent_runner` at process startup, passing a
//      closure that invokes `run_query_loop`.
//   3. TeamCreateTool calls `run_agent(...)` which dispatches through that slot.
//
// This keeps the module self-contained and avoids any extra crate boundary.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use futures::future::join_all;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Global agent-runner injection
// ---------------------------------------------------------------------------

/// A boxed async function that runs an agent sub-task and returns its output.
///
/// Arguments:
///   description — short label for logging
///   prompt      — full task prompt
///   tools       — optional allowlist of tool names; None means all tools
///   system      — optional system prompt override
///   max_turns   — max agent turns (default 10 when None)
///   ctx         — parent tool context (cloned in for the sub-agent)
///
/// Returns the agent's final text output.
pub type AgentRunFn = Arc<
    dyn Fn(
            String,              // description
            String,              // prompt
            Option<Vec<String>>, // tools allowlist
            Option<String>,      // system prompt
            Option<u32>,         // max_turns
            Arc<ToolContext>,    // context
        ) -> Pin<Box<dyn Future<Output = String> + Send>>
        + Send
        + Sync,
>;

static AGENT_RUNNER: OnceCell<AgentRunFn> = OnceCell::new();

/// Register the global agent runner.  Called once at process startup by cc-query.
///
/// # Panics
/// Panics if called more than once (once_cell semantics).
pub fn register_agent_runner(f: AgentRunFn) {
    if AGENT_RUNNER.set(f).is_err() {
        panic!("register_agent_runner called more than once");
    }
}

/// Execute a sub-agent via the registered runner.
///
/// Falls back to a stub result when no runner has been registered (e.g., in
/// unit tests that don't initialise cc-query).
async fn run_agent(
    description: String,
    prompt: String,
    tools: Option<Vec<String>>,
    system: Option<String>,
    max_turns: Option<u32>,
    ctx: Arc<ToolContext>,
) -> String {
    if let Some(runner) = AGENT_RUNNER.get() {
        runner(description, prompt, tools, system, max_turns, ctx).await
    } else {
        "[No agent runner registered — cc-query not initialised]".to_string()
    }
}

// ---------------------------------------------------------------------------
// Active-team registry
// ---------------------------------------------------------------------------
//
// Maps sanitized_team_name -> list of per-agent cancel tokens so that
// TeamDeleteTool can signal cancellation to still-running agents.

use dashmap::DashMap;
use once_cell::sync::Lazy;
use tokio_util::sync::CancellationToken;

static ACTIVE_TEAMS: Lazy<DashMap<String, Vec<CancellationToken>>> = Lazy::new(DashMap::new);

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn teams_base_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".claurst").join("teams"))
}

fn team_dir(team_name: &str) -> Option<std::path::PathBuf> {
    teams_base_dir().map(|b| b.join(sanitize_name(team_name)))
}

/// Sanitize a team name to a safe directory component.
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// On-disk schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TeamMember {
    agent_id: String,
    name: String,
    role: String,
    joined_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TeamConfig {
    name: String,
    task: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    created_at: u64,
    lead_agent_id: String,
    lead_session_id: String,
    parallel: bool,
    members: Vec<TeamMember>,
}

// ---------------------------------------------------------------------------
// TeamCreateTool
// ---------------------------------------------------------------------------

pub struct TeamCreateTool;

/// Per-agent specification provided in the input.
#[derive(Debug, Deserialize)]
struct AgentSpec {
    name: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    /// Optional per-agent task override.  When absent the shared top-level
    /// `task` is used.
    #[serde(default)]
    task: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TeamCreateInput {
    team_name: String,
    /// The shared task all agents work on (individual agents may override via
    /// `agents[i].task`).
    task: String,
    /// List of agents to spawn.
    #[serde(default)]
    agents: Vec<AgentSpec>,
    /// When true (default) all agents run in parallel via join_all.
    /// When false they run sequentially.
    #[serde(default = "default_parallel")]
    parallel: bool,
    /// Optional description stored in the config file.
    #[serde(default)]
    description: Option<String>,
}

fn default_parallel() -> bool {
    true
}

#[async_trait]
impl Tool for TeamCreateTool {
    fn name(&self) -> &str {
        "TeamCreate"
    }

    fn description(&self) -> &str {
        "Create a named team of agents that collectively work on a shared task. \
         Each agent gets a restricted tool list and its own prompt. \
         Agents run in parallel by default and their outputs are aggregated. \
         Input: { team_name, task, agents: [{name, role?, tools?, task?}], parallel?, description? }"
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "team_name": {
                    "type": "string",
                    "description": "Name for the new team."
                },
                "task": {
                    "type": "string",
                    "description": "The shared task all agents should work on."
                },
                "agents": {
                    "type": "array",
                    "description": "Agent specifications.  Each agent runs independently.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "role": { "type": "string", "description": "Role/persona description." },
                            "tools": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Allowed tool names.  Omit to use all tools."
                            },
                            "task": {
                                "type": "string",
                                "description": "Per-agent task override.  Falls back to top-level task."
                            }
                        },
                        "required": ["name"]
                    }
                },
                "parallel": {
                    "type": "boolean",
                    "description": "Run all agents in parallel (default: true).  Set false for sequential."
                },
                "description": {
                    "type": "string",
                    "description": "Optional team description stored in config."
                }
            },
            "required": ["team_name", "task"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: TeamCreateInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        if params.team_name.trim().is_empty() {
            return ToolResult::error("team_name is required for TeamCreate".to_string());
        }
        if params.task.trim().is_empty() {
            return ToolResult::error("task is required for TeamCreate".to_string());
        }

        let safe_name = sanitize_name(&params.team_name);
        let lead_agent_id = format!("team-lead@{}", safe_name);

        // Resolve team directory, disambiguating if name already exists.
        let dir = match team_dir(&params.team_name) {
            Some(d) => d,
            None => return ToolResult::error("Could not determine home directory".to_string()),
        };

        let (final_name, final_dir) = if dir.exists() {
            let suffix = &Uuid::new_v4().to_string()[..6];
            let new_name = format!("{}-{}", safe_name, suffix);
            let new_dir = match team_dir(&new_name) {
                Some(d) => d,
                None => return ToolResult::error("Could not determine home directory".to_string()),
            };
            (new_name, new_dir)
        } else {
            (safe_name.clone(), dir)
        };

        if let Err(e) = tokio::fs::create_dir_all(&final_dir).await {
            return ToolResult::error(format!("Failed to create team directory: {}", e));
        }

        let now = now_millis();

        // Build the member list for the config file.
        let members: Vec<TeamMember> = params
            .agents
            .iter()
            .enumerate()
            .map(|(i, spec)| TeamMember {
                agent_id: format!("agent-{}@{}", i, final_name),
                name: spec.name.clone(),
                role: spec.role.clone().unwrap_or_else(|| "assistant".to_string()),
                joined_at: now,
                tools: spec.tools.clone(),
            })
            .collect();

        let config = TeamConfig {
            name: final_name.clone(),
            task: params.task.clone(),
            description: params.description.clone(),
            created_at: now,
            lead_agent_id: lead_agent_id.clone(),
            lead_session_id: ctx.session_id.clone(),
            parallel: params.parallel,
            members: members.clone(),
        };

        let config_json = match serde_json::to_string_pretty(&config) {
            Ok(j) => j,
            Err(e) => return ToolResult::error(format!("Serialisation error: {}", e)),
        };

        let config_path = final_dir.join("config.json");
        if let Err(e) = tokio::fs::write(&config_path, &config_json).await {
            return ToolResult::error(format!("Failed to write config.json: {}", e));
        }

        // Write empty results placeholder.
        let results_path = final_dir.join("results.json");
        if let Err(e) = tokio::fs::write(&results_path, "[]").await {
            return ToolResult::error(format!("Failed to write results.json: {}", e));
        }

        // -----------------------------------------------------------------------
        // Spawn agents
        // -----------------------------------------------------------------------
        //
        // If there are no agent specs, return early with just the config info.
        if params.agents.is_empty() {
            let team_file_path = config_path.to_string_lossy().to_string();
            return ToolResult::success(
                json!({
                    "team_name": final_name,
                    "team_file_path": team_file_path,
                    "lead_agent_id": lead_agent_id,
                    "agents_spawned": 0,
                    "results": []
                })
                .to_string(),
            );
        }

        // Create one CancellationToken per agent so TeamDeleteTool can signal stop.
        let cancel_tokens: Vec<CancellationToken> = params
            .agents
            .iter()
            .map(|_| CancellationToken::new())
            .collect();

        ACTIVE_TEAMS.insert(final_name.clone(), cancel_tokens.clone());

        // Wrap the ToolContext in an Arc so it can be shared across agent futures.
        let ctx_arc = Arc::new(ctx.clone());

        // Build per-agent futures.
        let agent_futures: Vec<_> = params
            .agents
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let agent_name = spec.name.clone();
                let role = spec.role.clone().unwrap_or_else(|| "assistant".to_string());
                let tools = spec.tools.clone();
                let agent_task = spec.task.clone().unwrap_or_else(|| params.task.clone());
                let team_name_inner = final_name.clone();
                let cancel = cancel_tokens[i].clone();
                let ctx_inner = ctx_arc.clone();

                let system_prompt = format!(
                    "You are agent '{name}' on team '{team}'.  Your role: {role}.\n\
                     Work on the assigned task thoroughly and return your complete findings.",
                    name = agent_name,
                    team = team_name_inner,
                    role = role,
                );

                let description = format!("{}/{}", team_name_inner, agent_name);

                async move {
                    // Honour cancellation: return early if the team was deleted
                    // before we even start.
                    if cancel.is_cancelled() {
                        return (agent_name, "[Cancelled before start]".to_string());
                    }

                    let result = tokio::select! {
                        out = run_agent(
                            description,
                            agent_task,
                            tools,
                            Some(system_prompt),
                            Some(10),
                            ctx_inner,
                        ) => out,
                        _ = cancel.cancelled() => "[Agent cancelled by TeamDelete]".to_string(),
                    };

                    (agent_name, result)
                }
            })
            .collect();

        // Run agents: parallel (join_all) or sequential (iterate).
        let agent_results: Vec<(String, String)> = if params.parallel {
            join_all(agent_futures).await
        } else {
            let mut results = Vec::with_capacity(agent_futures.len());
            for fut in agent_futures {
                results.push(fut.await);
            }
            results
        };

        // Clean up the active-team registry.
        ACTIVE_TEAMS.remove(&final_name);

        // Persist results to disk.
        let results_json: Vec<Value> = agent_results
            .iter()
            .map(|(name, output)| json!({ "agent": name, "output": output }))
            .collect();
        let _ = tokio::fs::write(
            &results_path,
            serde_json::to_string_pretty(&results_json).unwrap_or_default(),
        )
        .await;

        // Build the aggregated output string.
        let mut aggregated = String::new();
        for (name, output) in &agent_results {
            aggregated.push_str(&format!("## Agent: {}\n\n{}\n\n", name, output));
        }

        let team_file_path = config_path.to_string_lossy().to_string();

        ToolResult::success(
            json!({
                "team_name": final_name,
                "team_file_path": team_file_path,
                "lead_agent_id": lead_agent_id,
                "agents_spawned": agent_results.len(),
                "parallel": params.parallel,
                "results": results_json,
                "aggregated_output": aggregated.trim()
            })
            .to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// TeamDeleteTool
// ---------------------------------------------------------------------------

pub struct TeamDeleteTool;

#[derive(Debug, Deserialize)]
struct TeamDeleteInput {
    team_name: String,
}

#[async_trait]
impl Tool for TeamDeleteTool {
    fn name(&self) -> &str {
        "TeamDelete"
    }

    fn description(&self) -> &str {
        "Cancel a running team and clean up its directories. \
         Signals all in-flight agents to stop, then removes \
         ~/.claurst/teams/{team_name}/."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "team_name": {
                    "type": "string",
                    "description": "Name of the team to delete."
                }
            },
            "required": ["team_name"]
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let params: TeamDeleteInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        if params.team_name.trim().is_empty() {
            return ToolResult::error("team_name is required for TeamDelete".to_string());
        }

        let safe_name = sanitize_name(&params.team_name);

        // Cancel any still-running agents.
        let cancelled_count = if let Some((_, tokens)) = ACTIVE_TEAMS.remove(&safe_name) {
            let count = tokens.len();
            for token in tokens {
                token.cancel();
            }
            count
        } else {
            0
        };

        // Remove the team directory from disk.
        let dir = match team_dir(&params.team_name) {
            Some(d) => d,
            None => return ToolResult::error("Could not determine home directory".to_string()),
        };

        if !dir.exists() {
            // Directory already gone — treat as success if we cancelled agents,
            // or as an informational message if nothing was running.
            return ToolResult::success(
                json!({
                    "success": true,
                    "message": format!(
                        "Team '{}' directory not found (may have been cleaned up already). \
                         Cancelled {} agent(s).",
                        params.team_name, cancelled_count
                    ),
                    "team_name": params.team_name,
                    "cancelled_agents": cancelled_count
                })
                .to_string(),
            );
        }

        if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
            return ToolResult::error(format!(
                "Failed to remove team directory '{}': {}",
                dir.display(),
                e
            ));
        }

        ToolResult::success(
            json!({
                "success": true,
                "message": format!(
                    "Cleaned up team \"{}\" and cancelled {} agent(s).",
                    params.team_name, cancelled_count
                ),
                "team_name": params.team_name,
                "cancelled_agents": cancelled_count
            })
            .to_string(),
        )
    }
}
