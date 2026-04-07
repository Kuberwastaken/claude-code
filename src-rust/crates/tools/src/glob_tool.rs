// Glob tool: fast file pattern matching.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use tracing::debug;

pub struct GlobTool;

#[derive(Debug, Deserialize)]
struct GlobInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        claurst_core::constants::TOOL_NAME_GLOB
    }

    fn description(&self) -> &str {
        "Fast file pattern matching tool that works with any codebase size. \
         Supports glob patterns like \"**/*.rs\" or \"src/**/*.ts\". Returns \
         matching file paths sorted by modification time. Use this tool when \
         you need to find files by name patterns."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match files against"
                },
                "path": {
                    "type": "string",
                    "description": "The directory to search in. Defaults to working directory."
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: GlobInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        let base_dir = params
            .path
            .as_ref()
            .map(|p| ctx.resolve_path(p))
            .unwrap_or_else(|| ctx.working_dir.clone());

        debug!(pattern = %params.pattern, dir = %base_dir.display(), "Running glob");

        if !base_dir.exists() || !base_dir.is_dir() {
            return ToolResult::error(format!("Directory not found: {}", base_dir.display()));
        }

        // Build the full glob pattern
        let full_pattern = base_dir.join(&params.pattern);
        let pattern_str = full_pattern.to_string_lossy().to_string();

        // On Windows, normalize backslashes to forward slashes for the glob crate
        let pattern_str = pattern_str.replace('\\', "/");

        let entries: Vec<PathBuf> = match glob::glob(&pattern_str) {
            Ok(paths) => paths.filter_map(|p| p.ok()).collect(),
            Err(e) => {
                return ToolResult::error(format!("Invalid glob pattern: {}", e));
            }
        };

        if entries.is_empty() {
            return ToolResult::success(format!(
                "No files matched pattern \"{}\" in {}",
                params.pattern,
                base_dir.display()
            ));
        }

        // Sort by modification time (most recent first) — fall back to name sort
        let mut entries_with_time: Vec<(PathBuf, std::time::SystemTime)> = entries
            .into_iter()
            .filter_map(|p| {
                let mtime = std::fs::metadata(&p).ok()?.modified().ok()?;
                Some((p, mtime))
            })
            .collect();

        entries_with_time.sort_by(|a, b| b.1.cmp(&a.1));

        let total = entries_with_time.len();
        let max_results = 250;
        let truncated = total > max_results;

        let mut output = String::new();
        for (path, _) in entries_with_time.iter().take(max_results) {
            output.push_str(&path.display().to_string());
            output.push('\n');
        }

        if truncated {
            output.push_str(&format!(
                "\n... and {} more files (showing first {})\n",
                total - max_results,
                max_results,
            ));
        }

        ToolResult::success(output)
    }
}
