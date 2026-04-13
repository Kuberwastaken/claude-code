// FileWrite tool: write/create files.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::debug;

pub struct FileWriteTool;

#[derive(Debug, Deserialize)]
struct FileWriteInput {
    file_path: String,
    content: String,
}

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        claurst_core::constants::TOOL_NAME_FILE_WRITE
    }

    fn description(&self) -> &str {
        "Writes a file to the local filesystem. This tool will overwrite the existing \
         file if there is one. Prefer the Edit tool for modifying existing files. \
         Only use this tool to create new files or for complete rewrites."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: FileWriteInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        let path = ctx.resolve_path(&params.file_path);
        debug!(path = %path.display(), "Writing file");

        // Permission check
        if let Err(e) =
            ctx.check_permission(self.name(), &format!("Write {}", path.display()), false)
        {
            return ToolResult::error(e.to_string());
        }

        // Ensure parent directories exist
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    return ToolResult::error(format!(
                        "Failed to create directory {}: {}",
                        parent.display(),
                        e
                    ));
                }
            }
        }

        let existed = path.exists();
        let before_content = if existed {
            match tokio::fs::read(&path).await {
                Ok(bytes) => bytes,
                Err(e) => {
                    return ToolResult::error(format!(
                        "Failed to read existing file {}: {}",
                        path.display(),
                        e
                    ))
                }
            }
        } else {
            Vec::new()
        };
        let is_new = !existed;

        // Write the file
        if let Err(e) = tokio::fs::write(&path, &params.content).await {
            return ToolResult::error(format!("Failed to write file {}: {}", path.display(), e));
        }

        ctx.record_file_change(
            path.clone(),
            &before_content,
            params.content.as_bytes(),
            self.name(),
        );

        // Run any configured formatter for this file type.
        crate::try_format_file(&path.to_string_lossy(), ctx).await;

        let line_count = params.content.lines().count();
        let byte_count = params.content.len();

        let action = if is_new { "Created" } else { "Wrote" };
        ToolResult::success(format!(
            "{} {} ({} lines, {} bytes)",
            action,
            path.display(),
            line_count,
            byte_count
        ))
        .with_metadata(json!({
            "file_path": path.display().to_string(),
            "is_new": is_new,
            "lines": line_count,
            "bytes": byte_count,
        }))
    }
}
