// REPLTool: Executes code in a persistent interpreter session.
//
// Tool name: "REPL" (matches TypeScript REPL_TOOL_NAME constant)
//
// The same interpreter process stays alive across multiple tool calls within
// a session. Supports: python3, node, bash (default).
//
// Input: { language?: "python"|"javascript"|"bash", code: string }
// Output: stdout/stderr from the interpreter
//
// Implementation uses per-(session, language) child processes kept alive in a
// global registry.  Code is injected over stdin; a known sentinel string is
// printed after each block so we know when output is complete.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::debug;

// ---------------------------------------------------------------------------
// Session registry
// ---------------------------------------------------------------------------

struct ReplSession {
    // We hold the Child handle so that the process is not killed when the
    // session is dropped.  We don't read from it directly after spawn.
    #[allow(dead_code)]
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Key: (session_id, language)
static REPL_SESSIONS: Lazy<Arc<DashMap<(String, String), Arc<Mutex<ReplSession>>>>> =
    Lazy::new(|| Arc::new(DashMap::new()));

// ---------------------------------------------------------------------------
// Sentinel values
// The interpreter prints this after executing user code so we know output is done.
// ---------------------------------------------------------------------------

const SENTINEL: &str = "__REPL_DONE_7f3a9b__";

/// Return the command + args to spawn for a given language.
fn interpreter_for(language: &str) -> Option<(&'static str, Vec<&'static str>)> {
    match language {
        "python" | "python3" => Some(("python3", vec!["-u", "-i"])),
        "javascript" | "node" => Some(("node", vec![])),
        "bash" | "" => Some(("bash", vec!["--norc", "--noprofile"])),
        _ => None,
    }
}

/// Build the code block + sentinel emission for the given language.
fn wrap_code(language: &str, code: &str) -> String {
    match language {
        "python" | "python3" => {
            // Wrap in exec() so multi-line blocks work inside `-i` mode.
            // After execution, print the sentinel unconditionally.
            format!(
                "import sys as _sys\ntry:\n    exec({:?})\nexcept Exception as _e:\n    print(repr(_e), file=_sys.stderr)\nprint({:?})\n",
                code, SENTINEL
            )
        }
        "javascript" | "node" => {
            // Node REPL (.load) can't do this inline; use eval via --input-type
            // but since we spawned a bare `node` process we use process.stdout.write.
            format!(
                "try {{ {} }} catch(e) {{ process.stderr.write(String(e) + '\\n') }}\nprocess.stdout.write({:?} + '\\n')\n",
                code, SENTINEL
            )
        }
        _ => {
            // bash: run code, echo sentinel at end
            format!("{}\necho {:?}\n", code, SENTINEL)
        }
    }
}

async fn get_or_spawn_session(
    session_id: &str,
    language: &str,
) -> Result<Arc<Mutex<ReplSession>>, String> {
    let key = (session_id.to_string(), language.to_string());

    // Fast path: session already exists
    if let Some(entry) = REPL_SESSIONS.get(&key) {
        return Ok(entry.clone());
    }

    // Spawn a new interpreter
    let (cmd, args) =
        interpreter_for(language).ok_or_else(|| format!("Unsupported language: {}", language))?;

    let mut child = tokio::process::Command::new(cmd)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn '{}': {}", cmd, e))?;

    let stdin = child.stdin.take().ok_or("No stdin")?;
    let stdout = child.stdout.take().ok_or("No stdout")?;

    let session = Arc::new(Mutex::new(ReplSession {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    }));

    REPL_SESSIONS.insert(key, session.clone());
    Ok(session)
}

/// Execute code in a session, returning collected output up to the sentinel.
async fn run_in_session(
    session: &Arc<Mutex<ReplSession>>,
    language: &str,
    code: &str,
) -> Result<String, String> {
    let wrapped = wrap_code(language, code);

    let mut guard = session.lock().await;
    guard
        .stdin
        .write_all(wrapped.as_bytes())
        .await
        .map_err(|e| format!("Write to interpreter stdin failed: {}", e))?;
    guard
        .stdin
        .flush()
        .await
        .map_err(|e| format!("Flush interpreter stdin failed: {}", e))?;

    // Read lines until we see the sentinel, with a timeout
    let mut output_lines: Vec<String> = Vec::new();
    let read_timeout = Duration::from_secs(30);

    loop {
        let mut line = String::new();
        let line_fut = guard.stdout.read_line(&mut line);
        match timeout(read_timeout, line_fut).await {
            Err(_) => {
                return Err(format!(
                    "Interpreter timed out after {}s waiting for output.",
                    read_timeout.as_secs()
                ))
            }
            Ok(Err(e)) => return Err(format!("Read error: {}", e)),
            Ok(Ok(0)) => {
                // EOF — interpreter exited
                return Err("Interpreter exited unexpectedly.".to_string());
            }
            Ok(Ok(_)) => {
                let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
                if trimmed == SENTINEL {
                    break;
                }
                // Strip the Python `>>>` / `...` prompts that -i mode emits
                let clean = trimmed
                    .trim_start_matches(">>> ")
                    .trim_start_matches("... ");
                output_lines.push(clean.to_string());
            }
        }
    }

    Ok(output_lines.join("\n"))
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

pub struct ReplTool;

#[derive(Debug, Deserialize)]
struct ReplInput {
    code: String,
    #[serde(default)]
    language: Option<String>,
}

#[async_trait]
impl Tool for ReplTool {
    fn name(&self) -> &str {
        "REPL"
    }

    fn description(&self) -> &str {
        "Execute code in a persistent interpreter session. The same interpreter process \
         stays alive across multiple tool calls so variables, imports, and state persist \
         between invocations. Supports bash (default), python, and javascript (node)."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "The code to execute in the interpreter session"
                },
                "language": {
                    "type": "string",
                    "enum": ["bash", "python", "javascript"],
                    "description": "Interpreter language. Defaults to bash."
                }
            },
            "required": ["code"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: ReplInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        let language = params.language.as_deref().unwrap_or("bash").to_lowercase();

        debug!(
            session = %ctx.session_id,
            language = %language,
            "ReplTool execute"
        );

        let session = match get_or_spawn_session(&ctx.session_id, &language).await {
            Ok(s) => s,
            Err(e) => return ToolResult::error(format!("Failed to start REPL session: {}", e)),
        };

        match run_in_session(&session, &language, &params.code).await {
            Ok(output) => ToolResult::success(output),
            Err(e) => {
                // Remove the dead session so next call spawns a fresh one
                let key = (ctx.session_id.clone(), language.clone());
                REPL_SESSIONS.remove(&key);
                ToolResult::error(format!("REPL error: {}", e))
            }
        }
    }
}
