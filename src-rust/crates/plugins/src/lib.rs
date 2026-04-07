// claurst-plugins: Plugin runtime for the Claurst CLI.
//
// This crate handles plugin discovery, manifest parsing, hook registration,
// and the /plugin + /reload-plugins command definitions.
//
// Dependency order: cc-plugins → cc-core only.
// cc-commands → cc-plugins (not the other way around).

pub mod hooks;
pub mod loader;
pub mod manifest;
pub mod marketplace;
pub mod plugin;
pub mod registry;

// Re-export the most commonly used items at the crate root.
pub use hooks::{register_plugin_hooks, HookOutcome, HookRegistry, RegisteredHook};
pub use loader::{default_user_plugins_dir, discover_plugins, project_plugins_dir};
pub use manifest::{
    HookEventKind, PluginAuthor, PluginHookEntry, PluginHookMatcher, PluginHooksConfig,
    PluginLspServer, PluginManifest, PluginMcpServer, UserConfigValueType,
};
pub use plugin::{
    CommandRunAction, LoadedPlugin, PluginCommandDef, PluginError, PluginSource, ReloadDiff,
};
pub use registry::PluginRegistry;

// ---------------------------------------------------------------------------
// Capability enforcement
// ---------------------------------------------------------------------------

/// Check whether a plugin command is allowed to execute based on its declared
/// capability grants and the capability the action requires.
///
/// # Policy
/// - If the manifest has **no `capabilities` field** (`None`), the plugin
///   predates capability enforcement and is trusted unconditionally (backwards
///   compatibility with existing plugins).
/// - If the manifest declares an explicit list (even an empty one), the
///   required capability **must** appear in that list.
///
/// Returns `Ok(())` when execution is permitted, or `Err(reason)` when it
/// should be blocked.  The caller should convert `Err` into a `ToolResult::error`.
pub fn check_plugin_capability(def: &PluginCommandDef) -> Result<(), String> {
    // Determine what capability this action needs.
    let required = match def.run_action.required_capability() {
        None => return Ok(()), // StaticResponse — no capability needed.
        Some(cap) => cap,
    };

    match &def.plugin_capabilities {
        // No `capabilities` field — old-style manifest, allow everything.
        None => Ok(()),
        // Explicit capability list — enforce it.
        Some(granted) => {
            if granted.iter().any(|g| g.as_str() == required) {
                Ok(())
            } else {
                Err(format!(
                    "Plugin '{}' is not allowed to use capability '{}'. \
                     Add '{}' to the 'capabilities' list in its manifest to enable this action.",
                    def.plugin_name, required, required
                ))
            }
        }
    }
}

use std::path::Path;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Global hook registry (set once at startup, read during tool execution)
// ---------------------------------------------------------------------------

static GLOBAL_HOOK_REGISTRY: OnceLock<HookRegistry> = OnceLock::new();

// ---------------------------------------------------------------------------
// Global plugin registry (set once at startup, read by commands / tools)
// ---------------------------------------------------------------------------

static GLOBAL_PLUGIN_REGISTRY: OnceLock<PluginRegistry> = OnceLock::new();

/// Store the fully-loaded `PluginRegistry` into a process-global static so
/// that slash commands and tools can query it without carrying the registry
/// through every call frame.
pub fn set_global_registry(registry: PluginRegistry) {
    // OnceLock::set fails silently if already initialised (e.g. during tests).
    let _ = GLOBAL_PLUGIN_REGISTRY.set(registry);
}

/// Access the global `PluginRegistry`, if it has been set.
pub fn global_plugin_registry() -> Option<&'static PluginRegistry> {
    GLOBAL_PLUGIN_REGISTRY.get()
}

/// Store the hook registry built from loaded plugins into a process-global
/// static so that `run_global_pre_tool_hook` / `run_global_post_tool_hook`
/// can access it from anywhere without passing the registry around.
pub fn set_global_hooks(registry: HookRegistry) {
    // OnceLock::set fails silently if already initialised (e.g. during tests).
    let _ = GLOBAL_HOOK_REGISTRY.set(registry);
}

/// Run all `PreToolUse` hooks registered by plugins for the given tool.
///
/// Returns `HookOutcome::Deny` if any blocking hook returns a non-zero exit
/// code, otherwise `HookOutcome::Allow`.
pub fn run_global_pre_tool_hook(
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> hooks::HookOutcome {
    let registry = match GLOBAL_HOOK_REGISTRY.get() {
        Some(r) => r,
        None => return hooks::HookOutcome::Allow,
    };

    let event_key = HookEventKind::PreToolUse.to_string();
    let hooks_for_event = match registry.get(&event_key) {
        Some(h) => h,
        None => return hooks::HookOutcome::Allow,
    };

    let event_json = serde_json::json!({
        "event": "PreToolUse",
        "tool_name": tool_name,
        "tool_input": tool_input,
    })
    .to_string();

    for hook in hooks_for_event {
        // Apply matcher filter if present
        if let Some(ref matcher) = hook.matcher {
            if !matcher.is_empty() && matcher != tool_name && matcher != "*" {
                continue;
            }
        }
        match hooks::run_hook_sync(hook, &event_json) {
            hooks::HookOutcome::Deny(reason) => return hooks::HookOutcome::Deny(reason),
            hooks::HookOutcome::Allow | hooks::HookOutcome::Error(_) => {}
        }
    }

    hooks::HookOutcome::Allow
}

/// Run all `PostToolUse` hooks registered by plugins for the given tool.
/// Post-tool hooks are informational; the return value is not used to block.
pub fn run_global_post_tool_hook(
    tool_name: &str,
    tool_input: &serde_json::Value,
    tool_output: &str,
    is_error: bool,
) {
    let registry = match GLOBAL_HOOK_REGISTRY.get() {
        Some(r) => r,
        None => return,
    };

    let event_key = HookEventKind::PostToolUse.to_string();
    let hooks_for_event = match registry.get(&event_key) {
        Some(h) => h,
        None => return,
    };

    let event_json = serde_json::json!({
        "event": "PostToolUse",
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_output": tool_output,
        "is_error": is_error,
    })
    .to_string();

    for hook in hooks_for_event {
        if let Some(ref matcher) = hook.matcher {
            if !matcher.is_empty() && matcher != tool_name && matcher != "*" {
                continue;
            }
        }
        hooks::run_hook_sync(hook, &event_json);
    }
}

// ---------------------------------------------------------------------------
// Top-level async API (called from cc-commands / cc-cli)
// ---------------------------------------------------------------------------

/// Discover and load all plugins from the standard locations.
///
/// Search order:
/// 1. `~/.claurst/plugins/`  (user-global)
/// 2. `<project_dir>/.claurst/plugins/`  (project-local)
/// 3. Any paths listed in `extra_paths`
///
/// Returns a fully populated `PluginRegistry`.  Errors encountered during
/// loading are stored in `registry.errors` rather than propagated, so the
/// caller always gets a usable registry even when individual plugins fail.
pub async fn load_plugins(
    project_dir: &Path,
    extra_paths: &[std::path::PathBuf],
) -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    let mut search_dirs: Vec<std::path::PathBuf> = Vec::new();

    // 1. User-global plugins directory.
    if let Some(user_dir) = default_user_plugins_dir() {
        search_dirs.push(user_dir);
    }

    // 2. Project-local plugins directory.
    search_dirs.push(project_plugins_dir(project_dir));

    // 3. Extra paths (from --plugin-dir or settings).
    search_dirs.extend_from_slice(extra_paths);

    // User plugins.
    if let Some(user_dir) = default_user_plugins_dir() {
        let (plugins, errors) = discover_plugins(&[user_dir], PluginSource::User).await;
        registry.extend(plugins, errors);
    }

    // Project plugins.
    let proj_dir = project_plugins_dir(project_dir);
    let (plugins, errors) = discover_plugins(&[proj_dir], PluginSource::Project).await;
    registry.extend(plugins, errors);

    // Extra paths.
    for path in extra_paths {
        let (plugins, errors) = discover_plugins(
            &[path.clone()],
            PluginSource::Extra(path.to_string_lossy().into_owned()),
        )
        .await;
        registry.extend(plugins, errors);
    }

    registry
}

/// Reload plugins: produce a new registry, compute the diff, and replace the old one.
///
/// Returns the new registry and a `ReloadDiff` describing what changed.
pub async fn reload_plugins(
    old_registry: &PluginRegistry,
    project_dir: &Path,
    extra_paths: &[std::path::PathBuf],
) -> (PluginRegistry, ReloadDiff) {
    let new_registry = load_plugins(project_dir, extra_paths).await;
    let diff = new_registry.diff_against(old_registry);
    (new_registry, diff)
}

// ---------------------------------------------------------------------------
// /plugin command definition (data-only, no SlashCommand impl here)
// ---------------------------------------------------------------------------

/// Sub-commands supported by `/plugin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSubCommand {
    /// `/plugin list` — show all installed plugins with enabled/disabled status.
    List,
    /// `/plugin enable <name>` — enable a plugin.
    Enable(String),
    /// `/plugin disable <name>` — disable a plugin.
    Disable(String),
    /// `/plugin info <name>` — show details about a plugin.
    Info(String),
    /// `/plugin install <path>` — install a plugin from a local path.
    Install(String),
    /// `/plugin reload` — reload plugins from disk.
    Reload,
    /// Show usage / help.
    Help,
}

/// Parse the arguments string for `/plugin`.
pub fn parse_plugin_args(args: &str) -> PluginSubCommand {
    let args = args.trim();
    // No args → show list
    if args.is_empty() {
        return PluginSubCommand::List;
    }
    let parts: Vec<&str> = args.splitn(3, char::is_whitespace).collect();
    match parts.first().map(|s| s.to_lowercase()).as_deref() {
        Some("list") | Some("ls") => PluginSubCommand::List,
        Some("enable") => PluginSubCommand::Enable(parts.get(1).unwrap_or(&"").to_string()),
        Some("disable") => PluginSubCommand::Disable(parts.get(1).unwrap_or(&"").to_string()),
        Some("info") | Some("show") => {
            PluginSubCommand::Info(parts.get(1).unwrap_or(&"").to_string())
        }
        Some("install") | Some("i") => {
            PluginSubCommand::Install(parts.get(1).unwrap_or(&"").to_string())
        }
        Some("reload") | Some("refresh") => PluginSubCommand::Reload,
        Some("help") | Some("--help") | Some("-h") => PluginSubCommand::Help,
        _ => PluginSubCommand::Help,
    }
}

/// Build the text output for `/plugin list`.
pub fn format_plugin_list(registry: &PluginRegistry) -> String {
    let mut out = String::new();
    let mut all: Vec<&LoadedPlugin> = registry.all();
    all.sort_by(|a, b| a.name.cmp(&b.name));

    if all.is_empty() {
        return "No plugins installed.\n\nUse `/plugin install <path>` to install a plugin from a local directory.".to_string();
    }

    let total = all.len();
    let enabled_count = all.iter().filter(|p| registry.is_enabled(&p.name)).count();
    out.push_str(&format!(
        "Installed plugins: {} ({} enabled)\n\n",
        total, enabled_count
    ));
    for p in &all {
        let status = if registry.is_enabled(&p.name) {
            "enabled"
        } else {
            "disabled"
        };
        let version = p.manifest.version.as_deref().unwrap_or("(no version)");
        let desc = p.manifest.description.as_deref().unwrap_or("");

        // Count commands and hooks for this plugin.
        let cmd_count = loader::collect_command_defs(p).len();
        let hook_count = p
            .hooks_config
            .as_ref()
            .map(|hc| hc.events.values().map(|v| v.len()).sum::<usize>())
            .unwrap_or(0);

        out.push_str(&format!("  {} [{}] v{}", p.name, status, version));
        if !desc.is_empty() {
            out.push_str(&format!(" — {}", desc));
        }
        let mut extras: Vec<String> = Vec::new();
        if cmd_count > 0 {
            extras.push(format!(
                "{} cmd{}",
                cmd_count,
                if cmd_count == 1 { "" } else { "s" }
            ));
        }
        if hook_count > 0 {
            extras.push(format!(
                "{} hook{}",
                hook_count,
                if hook_count == 1 { "" } else { "s" }
            ));
        }
        if !extras.is_empty() {
            out.push_str(&format!(" ({})", extras.join(", ")));
        }
        out.push('\n');
    }

    if registry.error_count() > 0 {
        out.push_str(&format!(
            "\n{} plugin{} failed to load. Use `/plugin info <name>` for details.\n",
            registry.error_count(),
            if registry.error_count() == 1 { "" } else { "s" }
        ));
    }

    out
}

/// Build the text output for `/plugin info <name>`.
pub fn format_plugin_info(registry: &PluginRegistry, name: &str) -> String {
    match registry.get(name) {
        None => format!(
            "Plugin '{}' not found. Use `/plugin list` to see installed plugins.",
            name
        ),
        Some(p) => {
            let mut out = String::new();
            out.push_str(&format!("Plugin: {}\n", p.name));
            if let Some(v) = &p.manifest.version {
                out.push_str(&format!("Version: {}\n", v));
            }
            if let Some(d) = &p.manifest.description {
                out.push_str(&format!("Description: {}\n", d));
            }
            if let Some(author) = &p.manifest.author {
                out.push_str(&format!("Author: {}\n", author.name));
            }
            out.push_str(&format!(
                "Status: {}\n",
                if registry.is_enabled(name) {
                    "enabled"
                } else {
                    "disabled"
                }
            ));
            out.push_str(&format!("Source: {}\n", p.source_id));
            out.push_str(&format!("Path: {}\n", p.path.display()));

            // Count commands.
            let cmd_defs = loader::collect_command_defs(p);
            if !cmd_defs.is_empty() {
                out.push_str(&format!("\nCommands ({}):\n", cmd_defs.len()));
                for cmd in &cmd_defs {
                    out.push_str(&format!("  /{} — {}\n", cmd.name, cmd.description));
                }
            }

            // Hooks.
            if let Some(ref hooks_config) = p.hooks_config {
                let hook_count: usize = hooks_config.events.values().map(|v| v.len()).sum();
                if hook_count > 0 {
                    out.push_str(&format!("\nHooks ({}):\n", hook_count));
                    for (event, matchers) in &hooks_config.events {
                        for matcher in matchers {
                            for hook in &matcher.hooks {
                                let blocking = if hook.blocking { " [blocking]" } else { "" };
                                out.push_str(&format!(
                                    "  {} {}{}\n",
                                    event, hook.command, blocking
                                ));
                            }
                        }
                    }
                }
            }

            // MCP servers.
            if !p.manifest.mcp_servers.is_empty() {
                out.push_str(&format!(
                    "\nMCP servers ({}):\n",
                    p.manifest.mcp_servers.len()
                ));
                for srv in &p.manifest.mcp_servers {
                    out.push_str(&format!("  {}\n", srv.name));
                }
            }

            // LSP servers.
            if !p.manifest.lsp_servers.is_empty() {
                out.push_str(&format!(
                    "\nLSP servers ({}):\n",
                    p.manifest.lsp_servers.len()
                ));
                for srv in &p.manifest.lsp_servers {
                    out.push_str(&format!("  {}\n", srv.name));
                }
            }

            out
        }
    }
}

/// Install a plugin from a local path.
///
/// Copies the plugin directory into `~/.claurst/plugins/` and returns the
/// loaded plugin name on success.
pub fn install_plugin_from_path(source_path: &Path) -> Result<String, PluginError> {
    // Validate that the source looks like a plugin directory.
    if !source_path.exists() {
        return Err(PluginError::Io {
            path: source_path.to_string_lossy().into_owned(),
            message: "Path does not exist".to_string(),
        });
    }

    let manifest_path = if source_path.join("plugin.json").exists() {
        source_path.join("plugin.json")
    } else if source_path.join("plugin.toml").exists() {
        source_path.join("plugin.toml")
    } else {
        return Err(PluginError::InvalidManifest {
            path: source_path.to_string_lossy().into_owned(),
            message: "No plugin.json or plugin.toml found in directory".to_string(),
        });
    };

    let bytes = std::fs::read(&manifest_path).map_err(|e| PluginError::Io {
        path: manifest_path.to_string_lossy().into_owned(),
        message: e.to_string(),
    })?;

    let manifest = if manifest_path
        .extension()
        .map(|e| e == "toml")
        .unwrap_or(false)
    {
        PluginManifest::from_toml(&bytes)
    } else {
        PluginManifest::from_json(&bytes)
    }
    .map_err(|e| PluginError::InvalidManifest {
        path: manifest_path.to_string_lossy().into_owned(),
        message: e.to_string(),
    })?;

    let plugin_name = manifest.name.clone();

    // Determine install destination.
    let dest = match default_user_plugins_dir() {
        Some(d) => d.join(&plugin_name),
        None => {
            return Err(PluginError::Io {
                path: String::new(),
                message: "Cannot determine home directory for plugin installation".to_string(),
            })
        }
    };

    // Copy source to dest.
    copy_dir_all(source_path, &dest).map_err(|e| PluginError::Io {
        path: dest.to_string_lossy().into_owned(),
        message: e.to_string(),
    })?;

    tracing::info!(
        "Installed plugin '{}' from '{}' to '{}'",
        plugin_name,
        source_path.display(),
        dest.display()
    );

    Ok(plugin_name)
}

/// Recursively copy a directory.
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// /reload-plugins summary formatting
// ---------------------------------------------------------------------------

/// Format the result of a plugin reload into a human-readable string,
/// suitable for the `/reload-plugins` command output.
pub fn format_reload_summary(registry: &PluginRegistry, diff: &ReloadDiff) -> String {
    let enabled = registry.enabled_count();
    let total = registry.plugin_count();

    let mut parts: Vec<String> = Vec::new();
    parts.push(format!(
        "{} plugin{} loaded ({} enabled)",
        total,
        if total == 1 { "" } else { "s" },
        enabled
    ));

    let cmd_count: usize = registry.all_command_defs().len();
    parts.push(format!(
        "{} command{}",
        cmd_count,
        if cmd_count == 1 { "" } else { "s" }
    ));

    let hook_count: usize = registry
        .build_hook_registry()
        .values()
        .map(|v| v.len())
        .sum();
    parts.push(format!(
        "{} hook{}",
        hook_count,
        if hook_count == 1 { "" } else { "s" }
    ));

    let mcp_count = registry.all_mcp_servers().len();
    parts.push(format!(
        "{} plugin MCP server{}",
        mcp_count,
        if mcp_count == 1 { "" } else { "s" }
    ));

    let lsp_count = registry.all_lsp_servers().len();
    parts.push(format!(
        "{} plugin LSP server{}",
        lsp_count,
        if lsp_count == 1 { "" } else { "s" }
    ));

    let mut msg = format!("Reloaded: {}", parts.join(" · "));

    if !diff.added.is_empty() {
        msg.push_str(&format!("\nAdded: {}", diff.added.join(", ")));
    }
    if !diff.removed.is_empty() {
        msg.push_str(&format!("\nRemoved: {}", diff.removed.join(", ")));
    }
    if !diff.updated.is_empty() {
        msg.push_str(&format!("\nUpdated: {}", diff.updated.join(", ")));
    }
    if diff.error_count > 0 {
        msg.push_str(&format!(
            "\n{} error{} during load.",
            diff.error_count,
            if diff.error_count == 1 { "" } else { "s" }
        ));
    }

    msg
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_manifest(dir: &Path, json: &serde_json::Value) {
        let path = dir.join("plugin.json");
        std::fs::write(path, serde_json::to_vec_pretty(json).unwrap()).unwrap();
    }

    #[test]
    fn parse_plugin_args_list() {
        assert_eq!(parse_plugin_args("list"), PluginSubCommand::List);
        assert_eq!(parse_plugin_args("ls"), PluginSubCommand::List);
    }

    #[test]
    fn parse_plugin_args_enable() {
        assert_eq!(
            parse_plugin_args("enable my-plugin"),
            PluginSubCommand::Enable("my-plugin".to_string())
        );
    }

    #[test]
    fn parse_plugin_args_info() {
        assert_eq!(
            parse_plugin_args("info my-plugin"),
            PluginSubCommand::Info("my-plugin".to_string())
        );
    }

    #[tokio::test]
    async fn load_plugins_empty_dirs() {
        let tmp = TempDir::new().unwrap();
        let reg = load_plugins(tmp.path(), &[]).await;
        assert_eq!(reg.plugin_count(), 0);
        assert_eq!(reg.error_count(), 0);
    }

    #[tokio::test]
    async fn load_plugins_finds_project_plugin() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp
            .path()
            .join(".claurst")
            .join("plugins")
            .join("test-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        write_manifest(
            &plugin_dir,
            &serde_json::json!({ "name": "test-plugin", "version": "1.0.0", "description": "A test plugin" }),
        );

        let reg = load_plugins(tmp.path(), &[]).await;
        assert_eq!(reg.plugin_count(), 1);
        assert!(reg.get("test-plugin").is_some());
        assert!(reg.is_enabled("test-plugin"));
    }

    #[test]
    fn manifest_parse_json() {
        let json = serde_json::json!({
            "name": "my-plugin",
            "version": "0.1.0",
            "description": "Test",
            "mcpServers": {
                "my-server": { "command": "node", "args": ["server.js"] }
            }
        });
        let bytes = serde_json::to_vec(&json).unwrap();
        let manifest = PluginManifest::from_json(&bytes).unwrap();
        assert_eq!(manifest.name, "my-plugin");
        assert_eq!(manifest.mcp_servers.len(), 1);
        assert_eq!(manifest.mcp_servers[0].name, "my-server");
    }

    #[test]
    fn format_plugin_list_empty() {
        let reg = PluginRegistry::new();
        let out = format_plugin_list(&reg);
        assert!(out.contains("No plugins installed"));
    }

    #[test]
    fn format_reload_summary_basic() {
        let reg = PluginRegistry::new();
        let diff = ReloadDiff::default();
        let out = format_reload_summary(&reg, &diff);
        assert!(out.contains("Reloaded"));
    }
}
