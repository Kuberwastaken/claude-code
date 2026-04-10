// Bash tool: execute shell commands with timeout, streaming output, and
// persistent shell state (cwd + env) across invocations.

use crate::{PermissionLevel, ShellState, Tool, ToolContext, ToolResult, session_shell_state};
use async_trait::async_trait;
use claurst_core::bash_classifier::{BashRiskLevel, classify_bash_command};
use claurst_core::tasks::{BackgroundTask, global_registry};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::debug;

/// Sentinel appended to the shell wrapper script.  Everything printed after
/// this marker is metadata (final pwd + env dump) rather than user-visible output.
const SHELL_STATE_SENTINEL: &str = "__CC_SHELL_STATE__";

pub struct BashTool;

#[derive(Debug, Deserialize)]
struct BashInput {
    command: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_timeout")]
    timeout: u64,
    #[serde(default)]
    run_in_background: bool,
    #[serde(default)]
    notify_on_complete: bool,
}

fn default_timeout() -> u64 {
    120_000 // 2 minutes in ms
}

/// Parse a shell snapshot block (lines after `SHELL_STATE_SENTINEL`) into
/// `(new_cwd, env_delta)`.
///
/// The block format is:
/// ```text
/// __CC_SHELL_STATE__
/// /some/path          ← final cwd (first line after sentinel)
/// KEY=value           ← exported env vars (remaining lines)
/// ```
fn parse_shell_state_block(lines: &[String]) -> Option<(PathBuf, HashMap<String, String>)> {
    let mut iter = lines.iter();
    let cwd_line = iter.next()?;
    let cwd = PathBuf::from(cwd_line.trim());

    let mut env_vars = HashMap::new();
    for line in iter {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(eq) = line.find('=') {
            let key = line[..eq].to_string();
            let val = line[eq + 1..].to_string();
            // Filter out internal bash / system variables we don't want to persist
            if !key.starts_with('_') && !["SHLVL", "BASH_LINENO", "BASH_SOURCE",
                "FUNCNAME", "PIPESTATUS", "OLDPWD"].contains(&key.as_str()) {
                env_vars.insert(key, val);
            }
        }
    }

    Some((cwd, env_vars))
}

/// Extract `export VAR=value` patterns from a command string and return them
/// as a map.  Only handles simple, single-line exports; complex shell
/// constructs are handled by the full env-dump approach instead.
fn extract_exports_from_command(command: &str) -> HashMap<String, String> {
    // Match: export VAR=value  or  export VAR="value"  or  export VAR='value'
    let re = Regex::new(r#"(?m)^\s*export\s+([A-Za-z_][A-Za-z0-9_]*)=(?:"([^"]*)"|'([^']*)'|(\S*))"#)
        .unwrap();
    let mut map = HashMap::new();
    for cap in re.captures_iter(command) {
        let key = cap[1].to_string();
        let val = cap
            .get(2)
            .or_else(|| cap.get(3))
            .or_else(|| cap.get(4))
            .map(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        map.insert(key, val);
    }
    map
}

/// Build the bash wrapper script that:
/// 1. Restores saved cwd and env vars.
/// 2. Runs the user command.
/// 3. Prints the sentinel + final pwd + `env` dump so we can persist state.
///
/// On Windows we skip the wrapping (cmd.exe is a different shell).
fn build_wrapper_script(
    command: &str,
    state: &ShellState,
    base_cwd: &PathBuf,
) -> String {
    let effective_cwd = state
        .cwd
        .as_ref()
        .unwrap_or(base_cwd);

    // Escape the cwd for single-quote embedding
    let cwd_escaped: String = effective_cwd.to_string_lossy().replace('\'', "'\\''" );

    // Build export lines for persisted env vars
    let mut export_lines = String::new();
    for (k, v) in &state.env_vars {
        // Escape single quotes in value
        let v_escaped: String = v.replace('\'', "'\\''");
        export_lines.push_str(&format!("export {}='{}'\n", k, v_escaped));
    }

    // We use `env -0` + `awk` to safely dump env vars after the command
    // finishes without being confused by multi-line values.
    // If `env -0` is unavailable we fall back to a simpler printenv.
    format!(
        r#"set -e
cd '{cwd}'
{exports}
set +e
{user_cmd}
__CC_EXIT_CODE=$?
echo '{sentinel}'
pwd
env | grep -E '^[A-Za-z_][A-Za-z0-9_]*=' || true
exit $__CC_EXIT_CODE
"#,
        cwd = cwd_escaped,
        exports = export_lines,
        user_cmd = command,
        sentinel = SHELL_STATE_SENTINEL,
    )
}

/// Execute a command in the background, registering it in the global task registry.
async fn run_in_background(
    command: String,
    cwd: PathBuf,
    timeout_ms: u64,
    notify_on_complete: bool,
    completion_notifier: Option<crate::CompletionNotifier>,
) -> ToolResult {
    let task_name = format!("bg: {}", &command[..command.len().min(60)]);
    let mut task = BackgroundTask::new(&task_name);
    task.pid = None; // Will be set after spawn

    let task_id = global_registry().register(task);

    let task_id_clone = task_id.clone();
    let command_clone = command.clone();

    tokio::spawn(async move {
        let result = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            async {
                let child = if cfg!(windows) {
                    Command::new("cmd")
                        .arg("/C")
                        .arg(&command_clone)
                        .current_dir(&cwd)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .stdin(Stdio::null())
                        .spawn()
                } else {
                    Command::new("bash")
                        .arg("-c")
                        .arg(&command_clone)
                        .current_dir(&cwd)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .stdin(Stdio::null())
                        .spawn()
                };

                match child {
                    Ok(mut c) => {
                        // Record PID in the registry.
                        if let Some(pid) = c.id() {
                            global_registry().set_pid(&task_id_clone, pid);
                        }

                        let stdout = c.stdout.take();
                        let stderr = c.stderr.take();

                        if let Some(out) = stdout {
                            let mut lines = BufReader::new(out).lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                global_registry().append_output(&task_id_clone, &line);
                            }
                        }
                        if let Some(err) = stderr {
                            let mut lines = BufReader::new(err).lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                let err_line = format!("STDERR: {}", line);
                                global_registry().append_output(&task_id_clone, &err_line);
                            }
                        }

                        match c.wait().await {
                            Ok(status) if status.success() => {
                                global_registry().complete(&task_id_clone);
                            }
                            Ok(status) => {
                                let code = status.code().unwrap_or(-1);
                                global_registry().update_status(
                                    &task_id_clone,
                                    claurst_core::tasks::TaskStatus::Failed(
                                        format!("exit code {}", code)
                                    ),
                                );
                            }
                            Err(e) => {
                                global_registry().update_status(
                                    &task_id_clone,
                                    claurst_core::tasks::TaskStatus::Failed(e.to_string()),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        global_registry().update_status(
                            &task_id_clone,
                            claurst_core::tasks::TaskStatus::Failed(e.to_string()),
                        );
                    }
                }
            }
        )
        .await;

        if result.is_err() {
            global_registry().update_status(
                &task_id_clone,
                claurst_core::tasks::TaskStatus::Failed(format!("timed out after {}ms", timeout_ms)),
            );
        }
    });

    // If notify_on_complete is requested and a notifier is available, spawn a
    // watcher task that polls the registry until the task reaches a terminal
    // state, then injects a completion message into the agent's next turn.
    if notify_on_complete {
        if let Some(notifier) = completion_notifier {
            let watcher_task_id = task_id.clone();
            let watcher_command = command.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    let task = global_registry().get(&watcher_task_id);
                    match task {
                        Some(t) if matches!(
                            t.status,
                            claurst_core::tasks::TaskStatus::Completed
                                | claurst_core::tasks::TaskStatus::Failed(_)
                                | claurst_core::tasks::TaskStatus::Cancelled
                        ) => {
                            let exit_info = match &t.status {
                                claurst_core::tasks::TaskStatus::Completed => "exit 0".to_string(),
                                claurst_core::tasks::TaskStatus::Failed(msg) => {
                                    format!("failed: {}", msg)
                                }
                                claurst_core::tasks::TaskStatus::Cancelled => {
                                    "cancelled".to_string()
                                }
                                _ => unreachable!(),
                            };
                            let output = t.output.join("\n");
                            let output_tail = if output.len() > 2000 {
                                &output[output.len() - 2000..]
                            } else {
                                &output
                            };
                            let msg = format!(
                                "[Monitor] Background task {} completed ({}).\nCommand: {}\nOutput (last 2000 chars):\n{}",
                                watcher_task_id, exit_info, watcher_command, output_tail
                            );
                            notifier.notify(msg);
                            break;
                        }
                        None => break, // Task disappeared from registry
                        _ => {} // Still running, keep polling
                    }
                }
            });
        }
    }

    if notify_on_complete {
        ToolResult::success(format!(
            "Started background task {}.\nnotify_on_complete: enabled — you will be automatically notified when this task finishes.\nUse process_list or check task {} to monitor progress.\nCommand: {}",
            task_id, task_id, command
        ))
    } else {
        ToolResult::success(format!(
            "Command started in background.\nTask ID: {}\nCommand: {}",
            task_id, command
        ))
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        claurst_core::constants::TOOL_NAME_BASH
    }

    fn description(&self) -> &str {
        "Executes a given bash command and returns its output. The working directory \
         persists between commands, but shell state does not. The shell environment is \
         initialized from the user's profile (bash or zsh). Avoid using interactive \
         commands. Use this tool for running shell commands, scripts, git operations, \
         and system tasks."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "description": {
                    "type": "string",
                    "description": "Clear, concise description of what this command does"
                },
                "timeout": {
                    "type": "number",
                    "description": "Optional timeout in milliseconds (max 600000, default 120000)"
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Set to true to run command in the background"
                },
                "notify_on_complete": {
                    "type": "boolean",
                    "description": "When true (and run_in_background is also true), the agent will be automatically notified when the process finishes — no polling needed. Use for long-running tasks like test suites, builds, or deployments so you can keep working while they run.",
                    "default": false
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: BashInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        // Permission check
        let desc = params.description.as_deref().unwrap_or(&params.command);
        if let Err(e) = ctx.check_permission(self.name(), desc, false) {
            return ToolResult::error(e.to_string());
        }

        // Security classifier — unconditionally block Critical-risk commands.
        if classify_bash_command(&params.command) == BashRiskLevel::Critical {
            return ToolResult::error(format!(
                "Command blocked: classified as Critical risk by the bash security classifier.\n\
                 Refusing to execute: {}",
                params.command
            ));
        }

        let timeout_ms = params.timeout.min(600_000);

        // Retrieve the persistent shell state for this session.
        let shell_state_arc = session_shell_state(&ctx.session_id);

        // ── Background path ──────────────────────────────────────────────────
        if params.run_in_background {
            let cwd = {
                let state = shell_state_arc.lock();
                state.cwd.clone().unwrap_or_else(|| ctx.working_dir.clone())
            };
            return run_in_background(
                params.command,
                cwd,
                timeout_ms,
                params.notify_on_complete,
                ctx.completion_notifier.clone(),
            ).await;
        }

        // ── Foreground path ──────────────────────────────────────────────────
        let timeout_dur = Duration::from_millis(timeout_ms);

        debug!(command = %params.command, "Executing bash command");

        // On Windows fall back to a simpler cmd invocation without state wrapping.
        if cfg!(windows) {
            return self.execute_windows(&params.command, ctx, &shell_state_arc, timeout_dur, timeout_ms).await;
        }

        // Build a wrapper script that restores and then captures shell state.
        let script = {
            let state = shell_state_arc.lock();
            build_wrapper_script(&params.command, &state, &ctx.working_dir)
        };

        let mut child = match Command::new("bash")
            .arg("-c")
            .arg(&script)
            .current_dir(&ctx.working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to spawn command: {}", e)),
        };

        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        let result = tokio::time::timeout(timeout_dur, async {
            let mut stdout_lines = Vec::new();
            let mut stderr_lines = Vec::new();

            if let Some(stdout) = stdout_handle {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    stdout_lines.push(line);
                }
            }

            if let Some(stderr) = stderr_handle {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    stderr_lines.push(line);
                }
            }

            let status = child.wait().await;
            (stdout_lines, stderr_lines, status)
        })
        .await;

        match result {
            Ok((stdout_lines, stderr_lines, status)) => {
                let exit_code = status
                    .map(|s| s.code().unwrap_or(-1))
                    .unwrap_or(-1);

                // Split stdout into user-visible output and the state block.
                let sentinel_pos = stdout_lines
                    .iter()
                    .rposition(|l| l.trim() == SHELL_STATE_SENTINEL);

                let (user_lines, state_lines) = match sentinel_pos {
                    Some(pos) => (&stdout_lines[..pos], &stdout_lines[pos + 1..]),
                    None => (stdout_lines.as_slice(), &[][..]),
                };

                // Update persistent shell state from the block.
                if !state_lines.is_empty() {
                    if let Some((new_cwd, env_delta)) =
                        parse_shell_state_block(&state_lines.to_vec())
                    {
                        let mut state = shell_state_arc.lock();
                        state.cwd = Some(new_cwd);
                        // Merge (not replace) so vars set in earlier calls survive
                        for (k, v) in env_delta {
                            state.env_vars.insert(k, v);
                        }
                    }
                }

                // Also capture explicit exports from the command text (fast path
                // for simple export statements that might not show up in the env
                // dump if the command exited early).
                {
                    let exports = extract_exports_from_command(&params.command);
                    if !exports.is_empty() {
                        let mut state = shell_state_arc.lock();
                        for (k, v) in exports {
                            state.env_vars.insert(k, v);
                        }
                    }
                }

                let mut output = String::new();
                if !user_lines.is_empty() {
                    output.push_str(&user_lines.join("\n"));
                }
                if !stderr_lines.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str("STDERR:\n");
                    output.push_str(&stderr_lines.join("\n"));
                }
                if output.is_empty() {
                    output = "(no output)".to_string();
                }

                // Truncate very long output
                const MAX_OUTPUT_LEN: usize = 100_000;
                if output.len() > MAX_OUTPUT_LEN {
                    let half = MAX_OUTPUT_LEN / 2;
                    let start = &output[..half];
                    let end = &output[output.len() - half..];
                    output = format!(
                        "{}\n\n... ({} characters truncated) ...\n\n{}",
                        start,
                        output.len() - MAX_OUTPUT_LEN,
                        end
                    );
                }

                if exit_code != 0 {
                    ToolResult::error(format!(
                        "Command exited with code {}\n{}",
                        exit_code, output
                    ))
                } else {
                    ToolResult::success(output)
                }
            }
            Err(_) => {
                let _ = child.kill().await;
                ToolResult::error(format!("Command timed out after {}ms", timeout_ms))
            }
        }
    }
}

impl BashTool {
    /// Fallback for Windows: run via `cmd /C` without shell-state tracking.
    async fn execute_windows(
        &self,
        command: &str,
        ctx: &ToolContext,
        shell_state_arc: &std::sync::Arc<parking_lot::Mutex<crate::ShellState>>,
        timeout_dur: Duration,
        timeout_ms: u64,
    ) -> ToolResult {
        let effective_cwd = {
            let state = shell_state_arc.lock();
            state.cwd.clone().unwrap_or_else(|| ctx.working_dir.clone())
        };

        let mut child = match Command::new("cmd")
            .arg("/C")
            .arg(command)
            .current_dir(&effective_cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to spawn command: {}", e)),
        };

        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        let result = tokio::time::timeout(timeout_dur, async {
            let mut stdout_lines = Vec::new();
            let mut stderr_lines = Vec::new();

            if let Some(stdout) = stdout_handle {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    stdout_lines.push(line);
                }
            }
            if let Some(stderr) = stderr_handle {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    stderr_lines.push(line);
                }
            }
            let status = child.wait().await;
            (stdout_lines, stderr_lines, status)
        })
        .await;

        match result {
            Ok((stdout_lines, stderr_lines, status)) => {
                let exit_code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
                let mut output = String::new();
                if !stdout_lines.is_empty() {
                    output.push_str(&stdout_lines.join("\n"));
                }
                if !stderr_lines.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str("STDERR:\n");
                    output.push_str(&stderr_lines.join("\n"));
                }
                if output.is_empty() {
                    output = "(no output)".to_string();
                }
                const MAX_OUTPUT_LEN: usize = 100_000;
                if output.len() > MAX_OUTPUT_LEN {
                    let half = MAX_OUTPUT_LEN / 2;
                    let start = &output[..half];
                    let end = &output[output.len() - half..];
                    output = format!(
                        "{}\n\n... ({} characters truncated) ...\n\n{}",
                        start,
                        output.len() - MAX_OUTPUT_LEN,
                        end
                    );
                }
                if exit_code != 0 {
                    ToolResult::error(format!("Command exited with code {}\n{}", exit_code, output))
                } else {
                    ToolResult::success(output)
                }
            }
            Err(_) => {
                let _ = child.kill().await;
                ToolResult::error(format!("Command timed out after {}ms", timeout_ms))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_on_complete_in_schema() {
        let tool = BashTool;
        let schema = tool.input_schema();
        let props = &schema["properties"];
        assert!(
            props["notify_on_complete"].is_object(),
            "notify_on_complete should be in the schema"
        );
        assert_eq!(
            props["notify_on_complete"]["type"], "boolean",
            "notify_on_complete should be boolean"
        );
    }

    #[test]
    fn notify_on_complete_default_false() {
        let input: BashInput =
            serde_json::from_str(r#"{"command":"echo hi"}"#).unwrap();
        assert!(
            !input.notify_on_complete,
            "notify_on_complete should default to false"
        );
    }

    #[test]
    fn notify_on_complete_can_be_set_true() {
        let input: BashInput =
            serde_json::from_str(r#"{"command":"echo hi","notify_on_complete":true}"#).unwrap();
        assert!(input.notify_on_complete);
    }

    #[test]
    fn run_in_background_default_false() {
        let input: BashInput =
            serde_json::from_str(r#"{"command":"echo hi"}"#).unwrap();
        assert!(!input.run_in_background);
    }
}
