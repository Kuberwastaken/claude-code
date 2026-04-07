// LspTool — code intelligence via Language Server Protocol.
//
// Supports hover, definition, references, document symbols, and diagnostics.
// Ported from the TypeScript LSPTool; extended with full action routing.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde_json::Value;

pub struct LspTool;

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str {
        "LSP"
    }

    fn description(&self) -> &str {
        "Query a language server for code intelligence. Supports hover documentation, \
         go-to-definition, find-references, document symbols, and diagnostics. \
         Language servers must be configured in settings (lsp_servers)."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["hover", "definition", "references", "symbols", "diagnostics"],
                    "description": "The LSP action to perform."
                },
                "file": {
                    "type": "string",
                    "description": "Absolute or working-directory-relative path to the source file."
                },
                "line": {
                    "type": "integer",
                    "description": "1-based line number (required for hover, definition, references)."
                },
                "column": {
                    "type": "integer",
                    "description": "1-based column number (required for hover, definition, references)."
                }
            },
            "required": ["action", "file"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        // --- Parse inputs ---------------------------------------------------
        let action = match input.get("action").and_then(|v| v.as_str()) {
            Some(a) => a.to_string(),
            None => return ToolResult::error("'action' is required"),
        };

        let file_raw = match input.get("file").and_then(|v| v.as_str()) {
            Some(f) => f.to_string(),
            None => return ToolResult::error("'file' is required"),
        };

        // Resolve to absolute path
        let file_path = if std::path::Path::new(&file_raw).is_absolute() {
            file_raw.clone()
        } else {
            ctx.working_dir
                .join(&file_raw)
                .to_string_lossy()
                .into_owned()
        };

        // line/column only required for position-based actions
        let line = input.get("line").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
        let column = input.get("column").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

        // --- Seed the global LSP manager with configs from current session ---
        let lsp_manager_arc = claurst_core::lsp::global_lsp_manager();
        {
            let mut manager = lsp_manager_arc.lock().await;
            manager.seed_from_config(&ctx.config.lsp_servers);
        }

        // Check that at least one server is registered for this file before
        // doing expensive I/O.
        {
            let manager = lsp_manager_arc.lock().await;
            if manager.server_name_for_file_pub(&file_path).is_none() {
                return ToolResult::success(format!(
                    "No LSP server configured for '{}'. \
                     Add a server entry to lsp_servers in your settings to enable \
                     code intelligence for this file type.",
                    file_path
                ));
            }
        }

        // --- Ensure the file is opened on its LSP server --------------------
        {
            let mut manager = lsp_manager_arc.lock().await;
            if let Err(e) = manager.open_file(&file_path, &ctx.working_dir).await {
                return ToolResult::error(format!("Failed to open file in LSP: {}", e));
            }
        }

        // --- Dispatch action ------------------------------------------------
        match action.as_str() {
            "hover" => {
                let result = {
                    let mut manager = lsp_manager_arc.lock().await;
                    manager
                        .hover(&file_path, &ctx.working_dir, line, column)
                        .await
                };
                match result {
                    Ok(Some(text)) => ToolResult::success(text),
                    Ok(None) => ToolResult::success(format!(
                        "No hover information at {}:{}:{}",
                        file_path, line, column
                    )),
                    Err(e) => ToolResult::error(format!("hover failed: {}", e)),
                }
            }

            "definition" => {
                let result = {
                    let mut manager = lsp_manager_arc.lock().await;
                    manager
                        .definition(&file_path, &ctx.working_dir, line, column)
                        .await
                };
                match result {
                    Ok(locs) if locs.is_empty() => ToolResult::success(format!(
                        "No definition found at {}:{}:{}",
                        file_path, line, column
                    )),
                    Ok(locs) => ToolResult::success(locs.join("\n")),
                    Err(e) => ToolResult::error(format!("definition failed: {}", e)),
                }
            }

            "references" => {
                let result = {
                    let mut manager = lsp_manager_arc.lock().await;
                    manager
                        .references(&file_path, &ctx.working_dir, line, column)
                        .await
                };
                match result {
                    Ok(locs) if locs.is_empty() => ToolResult::success(format!(
                        "No references found at {}:{}:{}",
                        file_path, line, column
                    )),
                    Ok(locs) => ToolResult::success(format!(
                        "{} reference(s):\n{}",
                        locs.len(),
                        locs.join("\n")
                    )),
                    Err(e) => ToolResult::error(format!("references failed: {}", e)),
                }
            }

            "symbols" => {
                let result = {
                    let mut manager = lsp_manager_arc.lock().await;
                    manager
                        .document_symbols(&file_path, &ctx.working_dir)
                        .await
                };
                match result {
                    Ok(syms) if syms.is_empty() => {
                        ToolResult::success(format!("No symbols found in '{}'.", file_path))
                    }
                    Ok(syms) => ToolResult::success(syms.join("\n")),
                    Err(e) => ToolResult::error(format!("symbols failed: {}", e)),
                }
            }

            "diagnostics" => {
                // Give the server a short window to deliver diagnostics via the
                // textDocument/publishDiagnostics notification (at most 200 ms).
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;

                let diagnostics = {
                    let manager = lsp_manager_arc.lock().await;
                    manager.get_diagnostics_for_file(&file_path)
                };

                if diagnostics.is_empty() {
                    return ToolResult::success(format!(
                        "No diagnostics for '{}'.",
                        file_path
                    ));
                }

                let output = claurst_core::lsp::LspManager::format_diagnostics(&diagnostics);
                ToolResult::success(output)
            }

            other => ToolResult::error(format!(
                "Unknown action '{}'. Valid actions: hover, definition, references, symbols, diagnostics",
                other
            )),
        }
    }
}
