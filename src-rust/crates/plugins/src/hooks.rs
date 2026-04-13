/// Plugin hook execution — ported from `loadPluginHooks.ts`.
///
/// Hooks let plugins run shell commands in response to lifecycle events.
/// This module defines the data model and the synchronous dispatch helper.
use crate::manifest::{HookEventKind, PluginHookMatcher, PluginHooksConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Registered hook set
// ---------------------------------------------------------------------------

/// A hook entry enriched with the plugin context it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredHook {
    /// Shell command to execute.
    pub command: String,
    /// Optional tool-name matcher (glob accepted by the shell runner).
    pub matcher: Option<String>,
    /// Whether a non-zero exit code should block the operation.
    pub blocking: bool,
    /// Absolute path to the plugin root directory.
    pub plugin_root: String,
    /// Plugin name for display / logging.
    pub plugin_name: String,
    /// Plugin source identifier (e.g. `"myplugin@builtin"`).
    pub plugin_source: String,
}

/// All registered hooks for a running session, keyed by event name.
pub type HookRegistry = HashMap<String, Vec<RegisteredHook>>;

// ---------------------------------------------------------------------------
// Building the registry from loaded plugins
// ---------------------------------------------------------------------------

/// Convert raw `PluginHooksConfig` (from `hooks/hooks.json` or inline in the
/// manifest) into `RegisteredHook` entries and merge them into `registry`.
pub fn register_plugin_hooks(
    config: &PluginHooksConfig,
    plugin_root: &str,
    plugin_name: &str,
    plugin_source: &str,
    registry: &mut HookRegistry,
) {
    for (event_name, matchers) in &config.events {
        let registered_hooks = registry.entry(event_name.clone()).or_default();

        for matcher in matchers {
            for hook in &matcher.hooks {
                registered_hooks.push(RegisteredHook {
                    command: hook.command.clone(),
                    matcher: matcher.matcher.clone().or_else(|| hook.matcher.clone()),
                    blocking: hook.blocking,
                    plugin_root: plugin_root.to_string(),
                    plugin_name: plugin_name.to_string(),
                    plugin_source: plugin_source.to_string(),
                });
            }
        }
    }
}

/// Remove all hooks whose `plugin_root` is not in `active_roots`.
/// Used when plugins are disabled / uninstalled so their hooks stop firing.
pub fn prune_hooks(registry: &mut HookRegistry, active_roots: &std::collections::HashSet<String>) {
    for hooks in registry.values_mut() {
        hooks.retain(|h| active_roots.contains(&h.plugin_root));
    }
}

/// Parse a raw `serde_json::Value` that may be either a flat hooks object
/// (`{ "PreToolUse": [...] }`) or a wrapped object with a `hooks` key
/// (`{ "description": "...", "hooks": { "PreToolUse": [...] } }`).
///
/// Returns `None` when the value cannot be parsed (errors are logged).
pub fn parse_hooks_value(value: &serde_json::Value) -> Option<PluginHooksConfig> {
    // Try the wrapped form first.
    if let Some(inner) = value.get("hooks") {
        let description = value
            .get("description")
            .and_then(|d| d.as_str())
            .map(String::from);
        let events = parse_hooks_events_map(inner)?;
        return Some(PluginHooksConfig {
            description,
            events,
        });
    }

    // Fall back: the whole value is the events map.
    let events = parse_hooks_events_map(value)?;
    Some(PluginHooksConfig {
        description: None,
        events,
    })
}

/// Parse the events map `{ "PreToolUse": [ { matcher: ..., hooks: [...] } ] }`.
fn parse_hooks_events_map(
    value: &serde_json::Value,
) -> Option<HashMap<String, Vec<PluginHookMatcher>>> {
    let obj = value.as_object()?;
    let mut events: HashMap<String, Vec<PluginHookMatcher>> = HashMap::new();

    for (event_name, matchers_val) in obj {
        let matchers_arr = match matchers_val.as_array() {
            Some(a) => a,
            None => {
                tracing::warn!(
                    "Plugin hooks: expected array for event '{}', got something else",
                    event_name
                );
                continue;
            }
        };

        let mut parsed_matchers: Vec<PluginHookMatcher> = Vec::new();
        for mv in matchers_arr {
            match serde_json::from_value::<PluginHookMatcher>(mv.clone()) {
                Ok(m) => parsed_matchers.push(m),
                Err(e) => {
                    tracing::warn!(
                        "Plugin hooks: failed to parse matcher for '{}': {}",
                        event_name,
                        e
                    );
                }
            }
        }

        if !parsed_matchers.is_empty() {
            events.insert(event_name.clone(), parsed_matchers);
        }
    }

    Some(events)
}

// ---------------------------------------------------------------------------
// Hook execution result
// ---------------------------------------------------------------------------

/// What happened when a hook ran.
#[derive(Debug, Clone)]
pub enum HookOutcome {
    /// Hook exited 0 — allow the operation.
    Allow,
    /// Hook exited non-zero and was blocking — deny the operation.
    Deny(String),
    /// Hook failed to start / timed out — treated as allow (non-blocking failure).
    Error(String),
}

// ---------------------------------------------------------------------------
// Hook runner
// ---------------------------------------------------------------------------

/// Execute a single `RegisteredHook` by spawning its shell command.
///
/// `event_json` is the JSON payload that will be written to the child's stdin.
///
/// This is a synchronous wrapper around `std::process::Command`.  For
/// real-world async usage the caller should spawn a blocking task.
pub fn run_hook_sync(hook: &RegisteredHook, event_json: &str) -> HookOutcome {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = match Command::new(if cfg!(windows) { "cmd" } else { "sh" })
        .args(if cfg!(windows) {
            vec!["/C", &hook.command]
        } else {
            vec!["-c", &hook.command]
        })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CLAUDE_PLUGIN_ROOT", &hook.plugin_root)
        .env("CLAUDE_PLUGIN_NAME", &hook.plugin_name)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let msg = format!(
                "Plugin '{}' hook '{}' failed to start: {}",
                hook.plugin_name, hook.command, e
            );
            tracing::warn!("{}", msg);
            return HookOutcome::Error(msg);
        }
    };

    // Write event JSON to stdin, ignoring errors (child may not read it).
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(event_json.as_bytes());
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            let msg = format!(
                "Plugin '{}' hook '{}' wait error: {}",
                hook.plugin_name, hook.command, e
            );
            tracing::warn!("{}", msg);
            return HookOutcome::Error(msg);
        }
    };

    if output.status.success() {
        HookOutcome::Allow
    } else if hook.blocking {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let msg = format!(
            "Plugin '{}' blocked operation (exit {}): {}",
            hook.plugin_name,
            output.status.code().unwrap_or(-1),
            stderr.trim()
        );
        tracing::info!("{}", msg);
        HookOutcome::Deny(msg)
    } else {
        tracing::debug!(
            "Plugin '{}' hook exited {} (non-blocking)",
            hook.plugin_name,
            output.status.code().unwrap_or(-1)
        );
        HookOutcome::Allow
    }
}

/// Return the canonical string key for a `HookEventKind`.
pub fn event_key(kind: &HookEventKind) -> String {
    kind.to_string()
}

// ---------------------------------------------------------------------------
// PostToolUse hook dispatch (T2-14)
// ---------------------------------------------------------------------------

/// Dispatch PostToolUse hooks for a completed tool call.
///
/// Mirrors the TS plugin hook dispatch path in src/plugins/hooks.ts.
/// All matching hooks run concurrently; results are collected.
/// Non-blocking: non-zero exit logs a warning but doesn't fail the tool call.
pub async fn dispatch_post_tool_hooks(
    registry: &HookRegistry,
    tool_name: &str,
    tool_input_json: &str,
    tool_result_json: &str,
) -> Vec<String> {
    let event_name = event_key(&HookEventKind::PostToolUse);

    let matching_hooks: Vec<RegisteredHook> = match registry.get(&event_name) {
        None => return Vec::new(),
        Some(hooks) => hooks
            .iter()
            .filter(|h| {
                let pattern = h.matcher.as_deref().unwrap_or("*");
                glob_match(pattern, tool_name)
            })
            .cloned()
            .collect(),
    };

    if matching_hooks.is_empty() {
        return Vec::new();
    }

    let tool_name = tool_name.to_string();
    let tool_input = tool_input_json.to_string();
    let tool_result = tool_result_json.to_string();

    let mut tasks = Vec::new();
    for hook in matching_hooks {
        let tn = tool_name.clone();
        let ti = tool_input.clone();
        let tr = tool_result.clone();
        let task =
            tokio::task::spawn_blocking(move || run_post_tool_hook(&hook.command, &tn, &ti, &tr));
        tasks.push(task);
    }

    let mut outputs = Vec::new();
    for task in tasks {
        if let Ok(Ok(output)) = task.await {
            if !output.is_empty() {
                outputs.push(output);
            }
        }
    }
    outputs
}

/// Run a single PostToolUse hook command synchronously.
fn run_post_tool_hook(
    command: &str,
    tool_name: &str,
    tool_input_json: &str,
    tool_result_json: &str,
) -> Result<String, std::io::Error> {
    let output = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
        .args(if cfg!(windows) {
            vec!["/C", command]
        } else {
            vec!["-c", command]
        })
        .env("CLAUDE_TOOL_NAME", tool_name)
        .env("CLAUDE_TOOL_INPUT", tool_input_json)
        .env("CLAUDE_TOOL_RESULT", tool_result_json)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            tool_name,
            command,
            exit_code = output.status.code().unwrap_or(-1),
            stderr = %stderr,
            "PostToolUse hook returned non-zero exit"
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout.trim().to_string())
}

/// Simple glob pattern matching: `*` matches any sequence of chars.
fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern == text {
        return true;
    }
    // Handle patterns like "File*" or "*Tool"
    if let Some(prefix) = pattern.strip_suffix('*') {
        return text.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return text.ends_with(suffix);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wrapped_hooks_value() {
        let raw = serde_json::json!({
            "description": "test hooks",
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [{ "command": "echo pre", "blocking": false }] }
                ]
            }
        });
        let config = parse_hooks_value(&raw).unwrap();
        assert_eq!(config.description.as_deref(), Some("test hooks"));
        assert!(config.events.contains_key("PreToolUse"));
    }

    #[test]
    fn parse_flat_hooks_value() {
        let raw = serde_json::json!({
            "Stop": [
                { "hooks": [{ "command": "echo stop", "blocking": false }] }
            ]
        });
        let config = parse_hooks_value(&raw).unwrap();
        assert!(config.events.contains_key("Stop"));
    }
}
