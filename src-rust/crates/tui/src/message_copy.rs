//! Message copy utilities for different formatting options.
//!
//! Provides functions to copy messages in various formats:
//! - Markdown: Preserves markdown formatting
//! - Plaintext: Removes all markdown formatting
//! - Code blocks only: Extracts code blocks
//! - JSON: Serialized message data
//! - Selection only: Selected text only

use claurst_core::Message;
use serde_json::json;
use std::io::Write;

/// Copy message as markdown (preserving formatting)
pub fn copy_as_markdown(message: &Message) -> String {
    let content = match &message.content {
        claurst_core::MessageContent::Text(text) => text.clone(),
        claurst_core::MessageContent::Blocks(blocks) => {
            blocks
                .iter()
                .filter_map(|block| match block {
                    claurst_core::ContentBlock::Text { text } => Some(text.clone()),
                    claurst_core::ContentBlock::Thinking {
                        thinking,
                        signature,
                    } => {
                        // Include thinking blocks as collapsible sections
                        Some(format!(
                            "<details>\n<summary>Thinking ({})</summary>\n\n{}\n</details>",
                            signature, thinking
                        ))
                    }
                    claurst_core::ContentBlock::ToolUse { id, name, input } => {
                        // Format tool use as code block
                        Some(format!(
                            "```json\n// Tool: {}\n// ID: {}\n{}\n```",
                            name,
                            id,
                            serde_json::to_string_pretty(input).unwrap_or_default()
                        ))
                    }
                    claurst_core::ContentBlock::ToolResult {
                        tool_use_id: _,
                        content,
                        is_error,
                    } => {
                        let error_marker = if is_error.unwrap_or(false) {
                            "ERROR: "
                        } else {
                            ""
                        };
                        let result_text = match content {
                            claurst_core::ToolResultContent::Text(text) => text.clone(),
                            claurst_core::ToolResultContent::Blocks(blocks) => blocks
                                .iter()
                                .filter_map(|b| match b {
                                    claurst_core::ContentBlock::Text { text } => Some(text.clone()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n"),
                        };
                        Some(format!("```\n{}{}\n```", error_marker, result_text))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        }
    };

    format_markdown_message(&message.role, &content)
}

/// Copy message as plaintext (no markdown formatting)
pub fn copy_as_plaintext(message: &Message) -> String {
    let content = match &message.content {
        claurst_core::MessageContent::Text(text) => strip_markdown(text),
        claurst_core::MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                claurst_core::ContentBlock::Text { text } => Some(strip_markdown(text)),
                claurst_core::ContentBlock::Thinking { thinking, .. } => {
                    Some(format!("[Thinking]\n{}", thinking))
                }
                claurst_core::ContentBlock::ToolUse { name, input, .. } => Some(format!(
                    "[Tool: {}]\n{}",
                    name,
                    serde_json::to_string_pretty(input).unwrap_or_default()
                )),
                claurst_core::ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let error_marker = if is_error.unwrap_or(false) {
                        "[ERROR] "
                    } else {
                        ""
                    };
                    let result_text = match content {
                        claurst_core::ToolResultContent::Text(text) => text.clone(),
                        claurst_core::ToolResultContent::Blocks(blocks) => blocks
                            .iter()
                            .filter_map(|b| match b {
                                claurst_core::ContentBlock::Text { text } => Some(text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    };
                    Some(format!("{}{}", error_marker, result_text))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
    };

    let role_str = match message.role {
        claurst_core::Role::User => "User",
        claurst_core::Role::Assistant => "Assistant",
    };
    format!("{}:\n\n{}", role_str, content)
}

/// Extract and copy only code blocks from message
pub fn copy_code_blocks(message: &Message) -> String {
    let mut code_blocks = Vec::new();

    match &message.content {
        claurst_core::MessageContent::Text(text) => {
            extract_code_blocks_from_text(text, &mut code_blocks);
        }
        claurst_core::MessageContent::Blocks(blocks) => {
            for block in blocks {
                if let claurst_core::ContentBlock::Text { text } = block {
                    extract_code_blocks_from_text(text, &mut code_blocks);
                }
            }
        }
    }

    if code_blocks.is_empty() {
        "[No code blocks found in message]".to_string()
    } else {
        code_blocks.join("\n\n---\n\n")
    }
}

/// Copy message as JSON
pub fn copy_as_json(message: &Message) -> String {
    let role_str = match message.role {
        claurst_core::Role::User => "user",
        claurst_core::Role::Assistant => "assistant",
    };

    let json_value = json!({
        "role": role_str,
        "content": match &message.content {
            claurst_core::MessageContent::Text(text) => text.clone(),
            claurst_core::MessageContent::Blocks(blocks) => {
                blocks.iter().map(|b| format_block_for_json(b)).collect::<Vec<_>>().join("\n")
            }
        },
        "uuid": message.uuid,
        "cost": message.cost.as_ref().map(|c| json!({
            "input_tokens": c.input_tokens,
            "output_tokens": c.output_tokens,
            "cache_creation_input_tokens": c.cache_creation_input_tokens,
            "cache_read_input_tokens": c.cache_read_input_tokens,
            "cost_usd": c.cost_usd,
        }))
    });

    serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| "{}".to_string())
}

/// Extract plaintext from selected text (identity function, for consistency)
pub fn copy_selection(selected_text: &str) -> String {
    selected_text.to_string()
}

// ============================================================================
// Private helpers
// ============================================================================

/// Format a message with role prefix as markdown
fn format_markdown_message(role: &claurst_core::Role, content: &str) -> String {
    let role_str = match role {
        claurst_core::Role::User => "**User**",
        claurst_core::Role::Assistant => "**Assistant**",
    };
    format!("{}\n\n{}", role_str, content)
}

/// Strip markdown formatting from text
fn strip_markdown(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    let mut _in_code_block = false;
    let mut in_inline_code = false;

    while let Some(ch) = chars.next() {
        match ch {
            // Skip markdown syntax
            '*' | '_' => {
                // Check for bold/italic markers (skip them)
                if chars.peek() == Some(&ch) {
                    chars.next(); // consume second marker
                }
            }
            '`' => {
                in_inline_code = !in_inline_code;
                // Don't add backticks to plaintext
            }
            '[' => {
                // Handle markdown links: [text](url) -> text
                let mut link_text = String::new();
                let mut found_close = false;
                while let Some(ch) = chars.next() {
                    if ch == ']' {
                        found_close = true;
                        break;
                    }
                    link_text.push(ch);
                }
                result.push_str(&link_text);
                // Skip URL part
                if found_close && chars.peek() == Some(&'(') {
                    chars.next(); // consume '('
                    while let Some(ch) = chars.next() {
                        if ch == ')' {
                            break;
                        }
                    }
                }
            }
            '#' => {
                // Skip markdown headers, but keep content
                // Skip the hash marks
                while chars.peek() == Some(&'#') {
                    chars.next();
                }
                // Skip space after hashes
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
            }
            '!' => {
                // Skip markdown image syntax ![alt](url)
                if chars.peek() == Some(&'[') {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == ']' {
                            break;
                        }
                    }
                    if chars.peek() == Some(&'(') {
                        chars.next();
                        while let Some(c) = chars.next() {
                            if c == ')' {
                                break;
                            }
                        }
                    }
                }
            }
            '>' if text.starts_with('>') => {
                // Skip blockquote markers, but keep content
                if result.is_empty() || result.ends_with('\n') {
                    while chars.peek() == Some(&'>') {
                        chars.next();
                    }
                    if chars.peek() == Some(&' ') {
                        chars.next();
                    }
                }
            }
            _ => result.push(ch),
        }
    }

    result.trim().to_string()
}

/// Extract code blocks from markdown text
fn extract_code_blocks_from_text(text: &str, blocks: &mut Vec<String>) {
    let mut in_block = false;
    let mut current_block = String::new();
    let mut language = String::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        if line.starts_with("```") {
            if in_block {
                // End of code block
                if !current_block.trim().is_empty() {
                    blocks.push(current_block.clone());
                }
                current_block.clear();
                language.clear();
                in_block = false;
            } else {
                // Start of code block
                in_block = true;
                language = line[3..].trim().to_string();
            }
        } else if in_block {
            current_block.push_str(line);
            current_block.push('\n');
        }
    }

    // Handle unclosed block
    if in_block && !current_block.trim().is_empty() {
        blocks.push(current_block);
    }
}

/// Format a content block as JSON-compatible string
fn format_block_for_json(block: &claurst_core::ContentBlock) -> String {
    match block {
        claurst_core::ContentBlock::Text { text } => text.clone(),
        claurst_core::ContentBlock::Image { .. } => "[Image content]".to_string(),
        claurst_core::ContentBlock::ToolUse { id, name, input } => {
            format!(
                "[Tool: {} (ID: {})]\n{}",
                name,
                id,
                serde_json::to_string_pretty(input).unwrap_or_default()
            )
        }
        claurst_core::ContentBlock::ToolResult {
            tool_use_id: _,
            content,
            is_error,
        } => {
            let error_marker = if is_error.unwrap_or(false) {
                "[ERROR] "
            } else {
                ""
            };
            let result_text = match content {
                claurst_core::ToolResultContent::Text(text) => text.clone(),
                claurst_core::ToolResultContent::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|b| match b {
                        claurst_core::ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            format!("{}{}", error_marker, result_text)
        }
        claurst_core::ContentBlock::Thinking { thinking, .. } => thinking.clone(),
        _ => "[Unsupported content type]".to_string(),
    }
}

// ============================================================================
// Clipboard integration
// ============================================================================

/// Attempt to copy text to clipboard using platform CLI tools
pub fn copy_to_clipboard(text: &str) -> bool {
    // Windows
    #[cfg(target_os = "windows")]
    {
        // Call clip.exe directly (not through cmd.exe) for reliability in raw terminal mode.
        if let Ok(mut child) = std::process::Command::new("clip")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
                drop(stdin); // close stdin so clip reads EOF
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
        // Fallback: PowerShell Set-Clipboard (works in more environments)
        {
            let escaped = text.replace('\'', "''");
            if let Ok(mut child) = std::process::Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-Command",
                    &format!("Set-Clipboard '{}'", escaped),
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                return child.wait().map(|s| s.success()).unwrap_or(false);
            }
        }
    }

    // macOS
    #[cfg(target_os = "macos")]
    {
        if let Ok(mut child) = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
                drop(stdin);
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
    }

    // Linux
    #[cfg(target_os = "linux")]
    {
        for cmd in &["xclip -selection clipboard", "xsel --clipboard --input"] {
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            if let Ok(mut child) = std::process::Command::new(parts[0])
                .args(&parts[1..])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                    drop(stdin);
                }
                if let Ok(status) = child.wait() {
                    if status.success() {
                        return true;
                    }
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_markdown() {
        assert_eq!(strip_markdown("**bold**"), "bold");
        assert_eq!(strip_markdown("*italic*"), "italic");
        assert_eq!(strip_markdown("[link](url)"), "link");
        assert_eq!(strip_markdown("# Header"), "Header");
    }

    #[test]
    fn test_extract_code_blocks() {
        let text = r#"Some text
```rust
fn main() {}
```
More text
```python
print("hello")
```"#;
        let mut blocks = Vec::new();
        extract_code_blocks_from_text(text, &mut blocks);
        assert_eq!(blocks.len(), 2);
    }
}
