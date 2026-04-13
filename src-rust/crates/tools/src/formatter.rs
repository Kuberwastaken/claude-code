// formatter.rs — run a configured file formatter after writes/edits.

use crate::ToolContext;

/// Try to format a file using any configured formatter.
/// Returns silently if no formatter is configured or the formatter fails.
pub async fn try_format_file(path: &str, ctx: &ToolContext) {
    let formatters = &ctx.config.formatter;
    if formatters.is_empty() {
        return;
    }

    // Determine the file's extension (with leading dot, e.g. ".ts").
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e))
        .unwrap_or_default();

    for (_name, fmt) in formatters {
        if fmt.disabled || fmt.command.is_empty() {
            continue;
        }
        if !fmt.extensions.iter().any(|e| e == &ext) {
            continue;
        }

        let mut cmd = tokio::process::Command::new(&fmt.command[0]);
        let mut file_injected = false;
        for arg in &fmt.command[1..] {
            if arg == "$FILE" || arg == "{file}" {
                cmd.arg(path);
                file_injected = true;
            } else {
                cmd.arg(arg);
            }
        }
        // Append the file path if no explicit placeholder was present.
        if !file_injected {
            cmd.arg(path);
        }

        // Run with a 30-second timeout; silently ignore all errors.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output()).await;

        // Only apply the first matching formatter.
        break;
    }
}
