/// Plugin manifest types — ported from the TypeScript `schemas.ts` / `plugin.json` format.
///
/// A plugin directory looks like:
///
/// ```text
/// my-plugin/
/// ├── plugin.json     ← this manifest (also supports plugin.toml)
/// ├── commands/       ← *.md slash command definitions
/// ├── agents/         ← *.md agent definitions
/// ├── skills/         ← subdirs with SKILL.md
/// ├── hooks/          ← hooks.json
/// ├── output-styles/  ← *.md or *.json style definitions
/// └── .mcp.json       ← MCP server config (optional)
/// ```
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Author
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginAuthor {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

// ---------------------------------------------------------------------------
// MCP server config (simplified — full config lives in cc-core)
// ---------------------------------------------------------------------------

/// Inline MCP server declaration inside a plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginMcpServer {
    /// Server name/key used to identify it in the running session.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(rename = "type", default = "default_mcp_type")]
    pub server_type: String,
}

fn default_mcp_type() -> String {
    "stdio".to_string()
}

// ---------------------------------------------------------------------------
// LSP server config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginLspServer {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Map of file extension → LSP language ID, e.g. `{ ".ts": "typescript" }`.
    #[serde(default)]
    pub extension_to_language: HashMap<String, String>,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutdown_timeout: Option<u64>,
    #[serde(default)]
    pub restart_on_crash: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_restarts: Option<u32>,
}

fn default_transport() -> String {
    "stdio".to_string()
}

// ---------------------------------------------------------------------------
// Hook event types
// ---------------------------------------------------------------------------

/// All lifecycle events that plugin hooks can subscribe to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum HookEventKind {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    PermissionDenied,
    Notification,
    UserPromptSubmit,
    SessionStart,
    SessionEnd,
    Stop,
    StopFailure,
    SubagentStart,
    SubagentStop,
    PreCompact,
    PostCompact,
    PermissionRequest,
    Setup,
    TeammateIdle,
    TaskCreated,
    TaskCompleted,
    Elicitation,
    ElicitationResult,
    ConfigChange,
    WorktreeCreate,
    WorktreeRemove,
    InstructionsLoaded,
    CwdChanged,
    FileChanged,
}

impl std::fmt::Display for HookEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            HookEventKind::PreToolUse => "PreToolUse",
            HookEventKind::PostToolUse => "PostToolUse",
            HookEventKind::PostToolUseFailure => "PostToolUseFailure",
            HookEventKind::PermissionDenied => "PermissionDenied",
            HookEventKind::Notification => "Notification",
            HookEventKind::UserPromptSubmit => "UserPromptSubmit",
            HookEventKind::SessionStart => "SessionStart",
            HookEventKind::SessionEnd => "SessionEnd",
            HookEventKind::Stop => "Stop",
            HookEventKind::StopFailure => "StopFailure",
            HookEventKind::SubagentStart => "SubagentStart",
            HookEventKind::SubagentStop => "SubagentStop",
            HookEventKind::PreCompact => "PreCompact",
            HookEventKind::PostCompact => "PostCompact",
            HookEventKind::PermissionRequest => "PermissionRequest",
            HookEventKind::Setup => "Setup",
            HookEventKind::TeammateIdle => "TeammateIdle",
            HookEventKind::TaskCreated => "TaskCreated",
            HookEventKind::TaskCompleted => "TaskCompleted",
            HookEventKind::Elicitation => "Elicitation",
            HookEventKind::ElicitationResult => "ElicitationResult",
            HookEventKind::ConfigChange => "ConfigChange",
            HookEventKind::WorktreeCreate => "WorktreeCreate",
            HookEventKind::WorktreeRemove => "WorktreeRemove",
            HookEventKind::InstructionsLoaded => "InstructionsLoaded",
            HookEventKind::CwdChanged => "CwdChanged",
            HookEventKind::FileChanged => "FileChanged",
        };
        write!(f, "{}", s)
    }
}

// ---------------------------------------------------------------------------
// Hook definitions (hooks.json / inline hooks in manifest)
// ---------------------------------------------------------------------------

/// A single hook command entry (mirrors the TS `HookEntry`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHookEntry {
    /// Shell command to run. Receives the event JSON on stdin.
    pub command: String,
    /// Optional tool-name filter — only fires for this tool (PreToolUse / PostToolUse).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// If true, a non-zero exit code blocks the operation.
    #[serde(default)]
    pub blocking: bool,
}

/// A matcher + list of hooks for one event.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginHookMatcher {
    /// Optional glob/regex pattern to match tool names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub hooks: Vec<PluginHookEntry>,
}

/// The hooks configuration object (`hooks.json` or inline in `plugin.json`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginHooksConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Map of event name → list of matchers.
    #[serde(flatten)]
    pub events: HashMap<String, Vec<PluginHookMatcher>>,
}

// ---------------------------------------------------------------------------
// User-configurable option
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginUserConfigOption {
    #[serde(rename = "type")]
    pub value_type: UserConfigValueType,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserConfigValueType {
    String,
    Number,
    Boolean,
    Directory,
    File,
}

// ---------------------------------------------------------------------------
// Plugin manifest
// ---------------------------------------------------------------------------

/// The parsed contents of a `plugin.json` or `plugin.toml` manifest file.
///
/// Unknown fields are silently ignored (matches TS behaviour: `z.object({...})`
/// strips unknown top-level keys by default).
///
/// NOTE: No `rename_all` here — we normalise the raw JSON in `normalize_manifest_json`
/// before deserializing so that both camelCase (TS source) and snake_case variants
/// end up as the field names this struct declares.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginManifest {
    // ---- Required ----
    pub name: String,

    // ---- Optional metadata ----
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<PluginAuthor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,

    // ---- Content declarations ----
    /// Extra command files / directories beyond `commands/`.
    /// Each entry is a path relative to the plugin root (e.g. `"./extra/cmd.md"`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,

    /// Extra agent markdown files beyond `agents/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,

    /// Extra skill directories beyond `skills/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,

    /// Additional output-style files/directories beyond `output-styles/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_styles: Vec<String>,

    // ---- MCP servers ----
    /// Inline MCP server definitions (equivalent of `mcpServers` key in manifest).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<PluginMcpServer>,

    // ---- LSP servers ----
    /// Inline LSP server definitions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lsp_servers: Vec<PluginLspServer>,

    // ---- Hooks ----
    /// Inline hooks or a path to a hooks JSON file.
    /// Stored as raw JSON so we can handle both forms at load time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks: Option<serde_json::Value>,

    // ---- User config ----
    // Normalised from `userConfig` by normalize_manifest_json before deserializing.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub user_config: HashMap<String, PluginUserConfigOption>,

    // ---- Marketplace identifier (for registry use) ----
    // Normalised from `marketplaceId` by normalize_manifest_json before deserializing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marketplace_id: Option<String>,

    // ---- Capability grants ----
    /// The set of capability categories this plugin is allowed to use.
    ///
    /// When `None`, the plugin was written before capability enforcement was
    /// introduced — all categories are allowed for backwards compatibility.
    ///
    /// When `Some([])`, the plugin explicitly declares no capabilities, so any
    /// tool action that requires a capability will be blocked.
    ///
    /// Known categories:
    /// - `"read_files"`  — read from the local filesystem
    /// - `"write_files"` — write / modify files on the local filesystem
    /// - `"network"`     — make outbound HTTP/network requests
    /// - `"shell"`       — execute arbitrary shell commands
    /// - `"browser"`     — control a web browser (CDP / Playwright)
    /// - `"mcp"`         — call MCP server tools
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}

impl PluginManifest {
    /// Parse a manifest from JSON bytes (plugin.json).
    pub fn from_json(bytes: &[u8]) -> anyhow::Result<Self> {
        let v: serde_json::Value = serde_json::from_slice(bytes)?;
        // Handle both `mcpServers` (object) and `mcp_servers` (array) keys.
        let manifest = serde_json::from_value::<PluginManifest>(normalize_manifest_json(v))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Parse a manifest from TOML bytes (plugin.toml).
    pub fn from_toml(bytes: &[u8]) -> anyhow::Result<Self> {
        let s = std::str::from_utf8(bytes)?;
        let manifest: PluginManifest = toml::from_str(s)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Basic validation matching the TS schema checks.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.name.is_empty() {
            anyhow::bail!("Plugin name cannot be empty");
        }
        if self.name.contains(' ') {
            anyhow::bail!(
                "Plugin name '{}' cannot contain spaces. Use kebab-case.",
                self.name
            );
        }
        Ok(())
    }
}

/// Normalise the raw JSON value so that both camelCase and snake_case
/// variants of known fields work, and so `mcpServers` (object mapping) is
/// converted to a `Vec<PluginMcpServer>`.
fn normalize_manifest_json(mut v: serde_json::Value) -> serde_json::Value {
    let obj = match v.as_object_mut() {
        Some(o) => o,
        None => return v,
    };

    // Promote `mcpServers` (TS camelCase object) → `mcp_servers` (array).
    if let Some(mcp) = obj.remove("mcpServers") {
        if mcp.is_object() {
            let arr: Vec<serde_json::Value> = mcp
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, srv)| {
                    let mut entry = srv.clone();
                    if let Some(o) = entry.as_object_mut() {
                        o.insert("name".to_string(), serde_json::Value::String(k.clone()));
                    }
                    entry
                })
                .collect();
            obj.insert("mcp_servers".to_string(), serde_json::Value::Array(arr));
        } else if mcp.is_array() {
            obj.insert("mcp_servers".to_string(), mcp);
        }
    }

    // Promote `lspServers` (TS camelCase object) → `lsp_servers` (array).
    if let Some(lsp) = obj.remove("lspServers") {
        if lsp.is_object() {
            let arr: Vec<serde_json::Value> = lsp
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, srv)| {
                    let mut entry = srv.clone();
                    if let Some(o) = entry.as_object_mut() {
                        o.insert("name".to_string(), serde_json::Value::String(k.clone()));
                    }
                    entry
                })
                .collect();
            obj.insert("lsp_servers".to_string(), serde_json::Value::Array(arr));
        } else if lsp.is_array() {
            obj.insert("lsp_servers".to_string(), lsp);
        }
    }

    // Camel → snake for other top-level keys the Rust struct uses.
    let renames: &[(&str, &str)] = &[
        ("outputStyles", "output_styles"),
        ("userConfig", "user_config"),
        ("marketplaceId", "marketplace_id"),
        // `capabilities` has the same spelling in camelCase, so no rename
        // needed for that one — but keep the entry for future camelCase aliases.
    ];
    for (camel, snake) in renames {
        if let Some(val) = obj.remove(*camel) {
            obj.insert(snake.to_string(), val);
        }
    }

    // `commands` in TS can be a single string or array — normalise to array.
    if let Some(cmds) = obj.get("commands") {
        if cmds.is_string() {
            let s = cmds.as_str().unwrap().to_string();
            obj.insert(
                "commands".to_string(),
                serde_json::Value::Array(vec![serde_json::Value::String(s)]),
            );
        }
    }

    // Same for `agents`, `skills`, `output_styles`.
    for key in &["agents", "skills", "output_styles"] {
        if let Some(val) = obj.get(*key) {
            if val.is_string() {
                let s = val.as_str().unwrap().to_string();
                obj.insert(
                    key.to_string(),
                    serde_json::Value::Array(vec![serde_json::Value::String(s)]),
                );
            }
        }
    }

    serde_json::Value::Object(obj.clone())
}
