//! Text truncation utilities — mirrors src/utils/truncate.ts

/// Truncate `text` to at most `max_chars` characters.
/// If truncated, appends `… (truncated)`.
pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    // Find a safe char boundary
    let mut end = max_chars;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (truncated)", &text[..end])
}

/// Truncate a list of lines to at most `max_lines`.
/// If truncated, appends a `"… N more lines"` indicator.
pub fn truncate_lines(lines: &[String], max_lines: usize) -> (Vec<String>, bool) {
    if lines.len() <= max_lines {
        return (lines.to_vec(), false);
    }
    let mut out = lines[..max_lines].to_vec();
    let remaining = lines.len() - max_lines;
    out.push(format!(
        "… {} more line{}",
        remaining,
        if remaining == 1 { "" } else { "s" }
    ));
    (out, true)
}

/// Truncate tool output to a safe display length.
/// Returns `(truncated_text, was_truncated)`.
pub fn truncate_tool_output(text: &str, max_chars: usize) -> (String, bool) {
    if text.len() <= max_chars {
        return (text.to_string(), false);
    }
    let mut end = max_chars;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (
        format!("{}… [{} chars truncated]", &text[..end], text.len() - end),
        true,
    )
}

/// Truncate a file path for display, keeping the filename and shortening the directory.
pub fn truncate_path(path: &str, max_chars: usize) -> String {
    if path.len() <= max_chars {
        return path.to_string();
    }
    let filename = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);
    if filename.len() >= max_chars {
        return filename.to_string();
    }
    let prefix_len = max_chars - filename.len() - 4; // 4 for "…/"
    let dir = std::path::Path::new(path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("");
    if dir.len() <= prefix_len {
        return path.to_string();
    }
    format!("…/{}/{}", &dir[dir.len() - prefix_len..], filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_text_short() {
        assert_eq!(truncate_text("hello", 10), "hello");
    }

    #[test]
    fn truncate_text_long() {
        let t = truncate_text("hello world", 5);
        assert!(t.starts_with("hello"));
        assert!(t.contains("truncated"));
    }

    #[test]
    fn truncate_lines_over_limit() {
        let lines: Vec<String> = (0..10).map(|i| format!("line {}", i)).collect();
        let (out, truncated) = truncate_lines(&lines, 5);
        assert!(truncated);
        assert_eq!(out.len(), 6); // 5 lines + 1 indicator
        assert!(out[5].contains("5 more"));
    }
}
