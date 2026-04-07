// ConfigTool: get or set Claurst configuration settings at runtime.
//
// Reads from and persists to ~/.claurst/settings.json.
// Supported settings: model, max_tokens, verbose, permission_mode.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

pub struct ConfigTool;

#[derive(Debug, Deserialize)]
struct ConfigInput {
    setting: String,
    value: Option<Value>,
}

static SUPPORTED_SETTINGS: &[(&str, &str)] = &[
    ("model", "LLM model to use (e.g. 'claude-opus-4-6')"),
    ("max_tokens", "Maximum output tokens per response"),
    ("verbose", "Enable verbose logging (true/false)"),
    (
        "permission_mode",
        "Permission mode: default | accept_edits | bypass_permissions | plan",
    ),
    (
        "auto_compact",
        "Auto-compact conversation when context fills (true/false)",
    ),
];

#[async_trait]
impl Tool for ConfigTool {
    fn name(&self) -> &str {
        "Config"
    }

    fn description(&self) -> &str {
        "Get or set Claurst configuration settings. Omit 'value' to read the current value. \
         Supported settings: model, max_tokens, verbose, permission_mode, auto_compact. \
         Changes persist to ~/.claurst/settings.json."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "setting": {
                    "type": "string",
                    "description": "Setting key (e.g. 'model', 'verbose', 'max_tokens', 'permission_mode')"
                },
                "value": {
                    "description": "New value to set. Omit to read the current value."
                }
            },
            "required": ["setting"]
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let params: ConfigInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        let key = params.setting.trim();

        // List all supported settings
        if key == "list" || key == "help" {
            let lines: Vec<String> = SUPPORTED_SETTINGS
                .iter()
                .map(|(k, d)| format!("  {} — {}", k, d))
                .collect();
            return ToolResult::success(format!("Supported settings:\n{}", lines.join("\n")));
        }

        // Load current settings
        let mut settings = match claurst_core::config::Settings::load().await {
            Ok(s) => s,
            Err(e) => return ToolResult::error(format!("Failed to load settings: {}", e)),
        };

        if let Some(new_value) = params.value {
            // SET operation
            match key {
                "model" => {
                    let s = match new_value.as_str() {
                        Some(s) => s.to_string(),
                        None => return ToolResult::error("'model' must be a string".to_string()),
                    };
                    settings.config.model = Some(s.clone());
                    if let Err(e) = settings.save().await {
                        return ToolResult::error(format!("Failed to save settings: {}", e));
                    }
                    ToolResult::success(format!("model = \"{}\"", s))
                }
                "max_tokens" => {
                    let n = match new_value.as_u64() {
                        Some(n) => n as u32,
                        None => {
                            return ToolResult::error(
                                "'max_tokens' must be a positive integer".to_string(),
                            )
                        }
                    };
                    settings.config.max_tokens = Some(n);
                    if let Err(e) = settings.save().await {
                        return ToolResult::error(format!("Failed to save settings: {}", e));
                    }
                    ToolResult::success(format!("max_tokens = {}", n))
                }
                "verbose" => {
                    let b = match new_value.as_bool() {
                        Some(b) => b,
                        None => {
                            return ToolResult::error("'verbose' must be true or false".to_string())
                        }
                    };
                    settings.config.verbose = b;
                    if let Err(e) = settings.save().await {
                        return ToolResult::error(format!("Failed to save settings: {}", e));
                    }
                    ToolResult::success(format!("verbose = {}", b))
                }
                "auto_compact" => {
                    let b = match new_value.as_bool() {
                        Some(b) => b,
                        None => {
                            return ToolResult::error(
                                "'auto_compact' must be true or false".to_string(),
                            )
                        }
                    };
                    settings.config.auto_compact = b;
                    if let Err(e) = settings.save().await {
                        return ToolResult::error(format!("Failed to save settings: {}", e));
                    }
                    ToolResult::success(format!("auto_compact = {}", b))
                }
                "permission_mode" => {
                    use claurst_core::config::PermissionMode;
                    let s = match new_value.as_str() {
                        Some(s) => s,
                        None => {
                            return ToolResult::error(
                                "'permission_mode' must be a string".to_string(),
                            )
                        }
                    };
                    let mode = match s {
                        "default" => PermissionMode::Default,
                        "accept_edits" | "acceptEdits" => PermissionMode::AcceptEdits,
                        "bypass_permissions" | "bypassPermissions" => {
                            PermissionMode::BypassPermissions
                        }
                        "plan" => PermissionMode::Plan,
                        _ => {
                            return ToolResult::error(format!(
                                "Unknown permission_mode '{}'. Use: default | accept_edits | bypass_permissions | plan",
                                s
                            ))
                        }
                    };
                    settings.config.permission_mode = mode;
                    if let Err(e) = settings.save().await {
                        return ToolResult::error(format!("Failed to save settings: {}", e));
                    }
                    ToolResult::success(format!("permission_mode = \"{}\"", s))
                }
                _ => ToolResult::error(format!(
                    "Unknown setting '{}'. Use setting='list' to see all supported settings.",
                    key
                )),
            }
        } else {
            // GET operation
            match key {
                "model" => ToolResult::success(format!(
                    "model = \"{}\"",
                    settings.config.effective_model()
                )),
                "max_tokens" => ToolResult::success(format!(
                    "max_tokens = {}",
                    settings.config.effective_max_tokens()
                )),
                "verbose" => ToolResult::success(format!("verbose = {}", settings.config.verbose)),
                "auto_compact" => {
                    ToolResult::success(format!("auto_compact = {}", settings.config.auto_compact))
                }
                "permission_mode" => ToolResult::success(format!(
                    "permission_mode = \"{}\"",
                    permission_mode_str(&settings.config.permission_mode)
                )),
                _ => ToolResult::error(format!(
                    "Unknown setting '{}'. Use setting='list' to see all supported settings.",
                    key
                )),
            }
        }
    }
}

fn permission_mode_str(mode: &claurst_core::config::PermissionMode) -> &'static str {
    use claurst_core::config::PermissionMode;
    match mode {
        PermissionMode::Default => "default",
        PermissionMode::AcceptEdits => "accept_edits",
        PermissionMode::BypassPermissions => "bypass_permissions",
        PermissionMode::Plan => "plan",
    }
}
