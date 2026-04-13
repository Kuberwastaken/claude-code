// FileEdit tool: exact string replacement with old/new strings (like sed but
// deterministic).  Mirrors the TypeScript Edit tool behaviour.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::debug;

pub struct FileEditTool;

#[derive(Debug, Deserialize)]
struct FileEditInput {
    file_path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        claurst_core::constants::TOOL_NAME_FILE_EDIT
    }

    fn description(&self) -> &str {
        "Performs exact string replacements in files. The edit will FAIL if \
         `old_string` is not unique in the file (unless `replace_all` is true). \
         You MUST read the file first before editing. Preserve the exact \
         indentation as it appears in the file."
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
                    "description": "The absolute path to the file to modify"
                },
                "old_string": {
                    "type": "string",
                    "description": "The text to replace (must be unique in the file unless replace_all is true)"
                },
                "new_string": {
                    "type": "string",
                    "description": "The text to replace it with (must be different from old_string)"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences of old_string (default false)"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: FileEditInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        // Validate old != new
        if params.old_string == params.new_string {
            return ToolResult::error("old_string and new_string must be different".to_string());
        }

        let path = ctx.resolve_path(&params.file_path);
        debug!(path = %path.display(), "Editing file");

        // Permission check
        if let Err(e) =
            ctx.check_permission(self.name(), &format!("Edit {}", path.display()), false)
        {
            return ToolResult::error(e.to_string());
        }

        // Read current content
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                return ToolResult::error(format!("Failed to read file {}: {}", path.display(), e))
            }
        };

        // Count occurrences
        let count = content.matches(&params.old_string).count();

        if count == 0 {
            return ToolResult::error(format!(
                "old_string not found in {}. Make sure the string matches exactly, \
                 including whitespace and indentation.",
                path.display()
            ));
        }

        if count > 1 && !params.replace_all {
            return ToolResult::error(format!(
                "old_string appears {} times in {}. Either provide a larger string \
                 with more surrounding context to make it unique, or set replace_all \
                 to true to replace every occurrence.",
                count,
                path.display()
            ));
        }

        // Perform replacement
        let new_content = if params.replace_all {
            content.replace(&params.old_string, &params.new_string)
        } else {
            // Replace only the first occurrence
            content.replacen(&params.old_string, &params.new_string, 1)
        };

        // Write back
        if let Err(e) = tokio::fs::write(&path, &new_content).await {
            return ToolResult::error(format!("Failed to write file {}: {}", path.display(), e));
        }

        ctx.record_file_change(
            path.clone(),
            content.as_bytes(),
            new_content.as_bytes(),
            self.name(),
        );

        // Run any configured formatter for this file type.
        crate::try_format_file(&path.to_string_lossy(), ctx).await;

        // Build a diff snippet for the response
        let replacements = if params.replace_all { count } else { 1 };
        let msg = format!(
            "Successfully edited {} ({} replacement{}).",
            path.display(),
            replacements,
            if replacements != 1 { "s" } else { "" }
        );

        ToolResult::success(msg).with_metadata(json!({
            "file_path": path.display().to_string(),
            "replacements": replacements,
        }))
    }
}
