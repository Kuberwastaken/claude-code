// PTY-backed Bash tool: wraps every command in a real pseudo-terminal so that
// programs that query isatty() (npm, cargo, git, pytest, …) behave correctly.
//
// Shell state (cwd + env) is persisted across calls through the same sentinel
// mechanism as the original BashTool, so `cd` and `export` work as expected.
//
// Platform notes
// ──────────────
//  Unix  → portable_pty (native openpty)
//  Windows → falls back to the existing cmd.exe approach; ConPTY is available
//             in portable_pty but adds complexity for minimal gain on Windows.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult, session_shell_state};
use async_trait::async_trait;
use claurst_core::bash_classifier::{BashRiskLevel, classify_bash_command};
use claurst_core::tasks::{BackgroundTask, global_registry};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::debug;

// Unix-only imports used by the shell-state helpers and PTY execution path.
#[cfg(unix)]
use crate::ShellState;
#[cfg(unix)]
use regex::Regex;
#[cfg(unix)]
use std::collections::HashMap;

/// Sentinel appended to the shell wrapper script (Unix only).
#[cfg(unix)]
const SHELL_STATE_SENTINEL: &str = "__CC_SHELL_STATE__";

pub struct PtyBashTool;

#[derive(Debug, Deserialize)]
struct BashInput {
    command: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_timeout")]
    timeout: u64,
    #[serde(default)]
    run_in_background: bool,
}

fn default_timeout() -> u64 {
    120_000
}

// ---------------------------------------------------------------------------
// Shell state helpers — Unix only (used by the PTY wrapper script)
// ---------------------------------------------------------------------------

#[cfg(unix)]
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
            if !key.starts_with('_')
                && !["SHLVL", "BASH_LINENO", "BASH_SOURCE", "FUNCNAME", "PIPESTATUS", "OLDPWD"]
                    .contains(&key.as_str())
            {
                env_vars.insert(key, val);
            }
        }
    }

    Some((cwd, env_vars))
}

#[cfg(unix)]
fn extract_exports_from_command(command: &str) -> HashMap<String, String> {
    let re = Regex::new(
        r#"(?m)^\s*export\s+([A-Za-z_][A-Za-z0-9_]*)=(?:"([^"]*)"|'([^']*)'|(\S*))"#,
    )
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

#[cfg(unix)]
fn build_wrapper_script(command: &str, state: &ShellState, base_cwd: &PathBuf) -> String {
    let effective_cwd = state.cwd.as_ref().unwrap_or(base_cwd);
    let cwd_escaped: String = effective_cwd.to_string_lossy().replace('\'', "'\\''");

    let mut export_lines = String::new();
    for (k, v) in &state.env_vars {
        let v_escaped: String = v.replace('\'', "'\\''");
        export_lines.push_str(&format!("export {}='{}'\n", k, v_escaped));
    }

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

// ---------------------------------------------------------------------------
// Background execution (identical to bash.rs — no PTY needed for background)
// ---------------------------------------------------------------------------

async fn run_in_background(command: String, cwd: PathBuf, timeout_ms: u64) -> ToolResult {
    let task_name = format!("bg: {}", &command[..command.len().min(60)]);
    let mut task = BackgroundTask::new(&task_name);
    task.pid = None;
    let task_id = global_registry().register(task);
    let task_id_clone = task_id.clone();
    let command_clone = command.clone();

    tokio::spawn(async move {
        let result = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
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
                            global_registry()
                                .append_output(&task_id_clone, &format!("STDERR: {}", line));
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
                                claurst_core::tasks::TaskStatus::Failed(format!(
                                    "exit code {}",
                                    code
                                )),
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
        })
        .await;

        if result.is_err() {
            global_registry().update_status(
                &task_id_clone,
                claurst_core::tasks::TaskStatus::Failed(format!(
                    "timed out after {}ms",
                    timeout_ms
                )),
            );
        }
    });

    ToolResult::success(format!(
        "Command started in background.\nTask ID: {}\nCommand: {}",
        task_id, command
    ))
}

// ---------------------------------------------------------------------------
// ANSI stripping (Unix only — PTY output only happens on Unix)
// ---------------------------------------------------------------------------

/// Remove ANSI/VT escape sequences from PTY output, producing clean text.
#[cfg(unix)]
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next(); // consume '['
                    // CSI: consume parameter + intermediate bytes, stop at final byte
                    for c in &mut chars {
                        if c.is_ascii_alphabetic() || c == '@' {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC: consume until ST (ESC \) or BEL
                    chars.next(); // consume ']'
                    let mut prev = '\0';
                    for c in &mut chars {
                        if c == '\x07' {
                            break; // BEL terminates OSC
                        }
                        if prev == '\x1b' && c == '\\' {
                            break; // ST = ESC \ terminates OSC
                        }
                        prev = c;
                    }
                }
                Some('(') | Some(')') | Some('*') | Some('+') => {
                    chars.next(); // consume designator introducer
                    chars.next(); // consume charset code
                }
                _ => {
                    // Two-character escape (ESC X): skip next char
                    chars.next();
                }
            }
        } else if ch == '\r' {
            // CR without LF: treat as line reset (discard pending partial line)
            // CR+LF is fine: LF will follow and push the newline
        } else {
            result.push(ch);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Unix PTY execution
// ---------------------------------------------------------------------------

#[cfg(unix)]
async fn run_in_pty(
    script: &str,
    working_dir: &str,
    timeout: Duration,
) -> Result<(String, i32), String> {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::Read;

    let pty_system = native_pty_system();

    let pair = pty_system
        .openpty(PtySize {
            rows: 50,
            cols: 220,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Failed to open PTY: {}", e))?;

    let mut cmd = CommandBuilder::new("bash");
    cmd.args(["-c", script]);
    cmd.cwd(working_dir);

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn in PTY: {}", e))?;

    // Grab the reader *before* dropping slave so the fd stays valid
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to clone PTY reader: {}", e))?;

    // Drop slave after spawn — once the child's controlling terminal is gone,
    // the master side will see EOF when the child exits.
    drop(pair.slave);
    // Keep master alive until after reading is done.
    let _master = pair.master;

    // Read all PTY output in a blocking thread (portable_pty reader is sync)
    let read_handle = tokio::task::spawn_blocking(move || {
        let mut output = String::new();
        let mut buf = [0u8; 4096];
        const MAX_BYTES: usize = 2 * 1024 * 1024;
        let mut total = 0usize;

        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if total > MAX_BYTES {
                        output.push_str("\n[output truncated at 2 MB limit]");
                        break;
                    }
                    output.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
                Err(_) => break,
            }
        }
        output
    });

    let raw_output = tokio::time::timeout(timeout, read_handle)
        .await
        .map_err(|_| "Command timed out".to_string())?
        .map_err(|e| format!("PTY read thread panicked: {}", e))?;

    let exit_code = match child.wait() {
        Ok(status) => status.exit_code() as i32,
        Err(_) => -1,
    };

    Ok((raw_output, exit_code))
}

// ---------------------------------------------------------------------------
// Windows fallback (cmd.exe, no PTY)
// ---------------------------------------------------------------------------

#[cfg(windows)]
async fn run_windows_fallback(
    command: &str,
    effective_cwd: &PathBuf,
    timeout_dur: Duration,
    timeout_ms: u64,
) -> ToolResult {
    let mut child = match Command::new("cmd")
        .arg("/C")
        .arg(command)
        .current_dir(effective_cwd)
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
            truncate_output(output, exit_code)
        }
        Err(_) => {
            let _ = child.kill().await;
            ToolResult::error(format!("Command timed out after {}ms", timeout_ms))
        }
    }
}

// ---------------------------------------------------------------------------
// Shared output truncation helper
// ---------------------------------------------------------------------------

fn truncate_output(mut output: String, exit_code: i32) -> ToolResult {
    const MAX_OUTPUT_LEN: usize = 100_000;
    if output.len() > MAX_OUTPUT_LEN {
        let half = MAX_OUTPUT_LEN / 2;
        let start = output[..half].to_string();
        let end = output[output.len() - half..].to_string();
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

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for PtyBashTool {
    fn name(&self) -> &str {
        claurst_core::constants::TOOL_NAME_BASH
    }

    fn description(&self) -> &str {
        "Executes a given bash command in a real terminal (PTY) and returns its output. \
         The working directory persists between commands. Supports interactive programs, \
         colored output (stripped for readability), and terminal-aware tools like npm, \
         cargo, git, and pytest. Use for running shell commands, scripts, git operations, \
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
        let reason = params
            .description
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("This will execute a shell command.")
            .to_string();

        if let Err(e) = ctx.check_permission_for_path(
            self.name(),
            &reason,
            std::path::PathBuf::from(&params.command),
            false,
        ) {
            return ToolResult::error(e.to_string());
        }

        // Security classifier — block Critical-risk commands unconditionally.
        if classify_bash_command(&params.command) == BashRiskLevel::Critical {
            return ToolResult::error(format!(
                "Command blocked: classified as Critical risk by the bash security classifier.\n\
                 Refusing to execute: {}",
                params.command
            ));
        }

        let timeout_ms = params.timeout.min(600_000);
        let timeout_dur = Duration::from_millis(timeout_ms);
        let shell_state_arc = session_shell_state(&ctx.session_id);

        // ── Background path ──────────────────────────────────────────────────
        if params.run_in_background {
            let cwd = {
                let state = shell_state_arc.lock();
                state.cwd.clone().unwrap_or_else(|| ctx.working_dir.clone())
            };
            return run_in_background(params.command, cwd, timeout_ms).await;
        }

        debug!(command = %params.command, "Executing bash command via PTY");

        // ── Windows path (no PTY — use cmd.exe fallback) ─────────────────────
        #[cfg(windows)]
        {
            let effective_cwd = {
                let state = shell_state_arc.lock();
                state.cwd.clone().unwrap_or_else(|| ctx.working_dir.clone())
            };
            return run_windows_fallback(&params.command, &effective_cwd, timeout_dur, timeout_ms)
                .await;
        }

        // ── Unix PTY path ────────────────────────────────────────────────────
        #[cfg(unix)]
        {
            // Build the wrapper script that restores + captures shell state.
            let (script, working_dir_str) = {
                let state = shell_state_arc.lock();
                let script = build_wrapper_script(&params.command, &state, &ctx.working_dir);
                let wd = ctx.working_dir.to_string_lossy().into_owned();
                (script, wd)
            };

            let result =
                tokio::time::timeout(timeout_dur, run_in_pty(&script, &working_dir_str, timeout_dur))
                    .await;

            match result {
                Ok(Ok((raw_output, exit_code))) => {
                    // Strip ANSI escape codes from PTY output
                    let cleaned = strip_ansi(&raw_output);

                    // Split into user-visible lines and state block
                    let all_lines: Vec<String> =
                        cleaned.lines().map(|l| l.to_string()).collect();

                    let sentinel_pos = all_lines
                        .iter()
                        .rposition(|l| l.trim() == SHELL_STATE_SENTINEL);

                    let (user_lines, state_lines) = match sentinel_pos {
                        Some(pos) => (&all_lines[..pos], &all_lines[pos + 1..]),
                        None => (all_lines.as_slice(), &[][..]),
                    };

                    // Update persistent shell state
                    if !state_lines.is_empty() {
                        if let Some((new_cwd, env_delta)) =
                            parse_shell_state_block(&state_lines.to_vec())
                        {
                            let mut state = shell_state_arc.lock();
                            state.cwd = Some(new_cwd);
                            for (k, v) in env_delta {
                                state.env_vars.insert(k, v);
                            }
                        }
                    }

                    // Fast-path export capture
                    {
                        let exports = extract_exports_from_command(&params.command);
                        if !exports.is_empty() {
                            let mut state = shell_state_arc.lock();
                            for (k, v) in exports {
                                state.env_vars.insert(k, v);
                            }
                        }
                    }

                    let mut output = user_lines.join("\n");
                    if output.is_empty() {
                        output = "(no output)".to_string();
                    }

                    truncate_output(output, exit_code)
                }
                Ok(Err(e)) => ToolResult::error(format!("PTY execution failed: {}", e)),
                Err(_) => ToolResult::error(format!("Command timed out after {}ms", timeout_ms)),
            }
        }
    }
}
