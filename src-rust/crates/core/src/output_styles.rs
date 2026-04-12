//! Output style system — customises how Claude responds to the user.
//!
//! Styles are applied by injecting `OutputStyleDef::prompt` into the system
//! prompt.  Built-in styles are defined in code; users can add their own by
//! placing `.md` or `.json` files in:
//!   - Global: `~/.claurst/output-styles/`
//!   - Project: `.claurst/output-styles/`
//!
//! Markdown style files have a simple structure:
//!   Line 1: `# <Label>` (heading becomes the label)
//!   Line 2: short description
//!   Remainder: the prompt text injected into the system prompt

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single output style definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputStyleDef {
    /// Machine-readable identifier (e.g. `"concise"`).
    pub name: String,
    /// Human-readable label shown in picker UI (e.g. `"Concise"`).
    pub label: String,
    /// One-line description.
    pub description: String,
    /// Text injected into the system prompt when this style is active.
    /// Empty string for the default style (no extra injection).
    pub prompt: String,
}

impl OutputStyleDef {
    // ---- Built-in styles ---------------------------------------------------

    pub fn builtin_default() -> Self {
        Self {
            name: "default".to_string(),
            label: "Default".to_string(),
            description: "Standard Claurst responses.".to_string(),
            prompt: String::new(),
        }
    }

    pub fn builtin_concise() -> Self {
        Self {
            name: "concise".to_string(),
            label: "Concise".to_string(),
            description: "Short, direct responses with minimal explanation.".to_string(),
            prompt: "Be maximally concise. Skip preamble, summaries, and filler. \
                     Lead with the answer."
                .to_string(),
        }
    }

    pub fn builtin_explanatory() -> Self {
        Self {
            name: "explanatory".to_string(),
            label: "Explanatory".to_string(),
            description: "Thorough explanations with reasoning and alternatives.".to_string(),
            prompt: "When explaining code or concepts, be thorough and educational. \
                     Include reasoning, alternatives considered, and potential pitfalls. \
                     Err on the side of over-explaining."
                .to_string(),
        }
    }

    pub fn builtin_learning() -> Self {
        Self {
            name: "learning".to_string(),
            label: "Learning".to_string(),
            description: "Pedagogical mode — explains patterns and decisions.".to_string(),
            prompt: "This user is learning. Explain concepts as you implement them. \
                     Point out patterns, best practices, and why you made each decision. \
                     Use analogies when helpful."
                .to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Built-ins
// ---------------------------------------------------------------------------

/// Return all built-in output styles in display order.
pub fn builtin_styles() -> Vec<OutputStyleDef> {
    vec![
        OutputStyleDef::builtin_default(),
        OutputStyleDef::builtin_concise(),
        OutputStyleDef::builtin_explanatory(),
        OutputStyleDef::builtin_learning(),
    ]
}

// ---------------------------------------------------------------------------
// Loading from disk
// ---------------------------------------------------------------------------

/// Load user-defined output styles from a directory.
///
/// Supported file formats:
/// - `.md`   — Markdown: `# Label\ndescription\n\nprompt text…`
/// - `.json` — JSON: `{ "name": "…", "label": "…", "description": "…", "prompt": "…" }`
///
/// Files that cannot be parsed are silently skipped.
pub fn load_output_styles_dir(styles_dir: &Path) -> Vec<OutputStyleDef> {
    if !styles_dir.exists() {
        return Vec::new();
    }

    let entries = match std::fs::read_dir(styles_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut styles = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "md" || ext == "json" {
            if let Some(style) = load_style_file(&path) {
                styles.push(style);
            }
        }
    }

    // Sort alphabetically so the list is deterministic.
    styles.sort_by(|a, b| a.name.cmp(&b.name));
    styles
}

fn load_style_file(path: &Path) -> Option<OutputStyleDef> {
    let content = std::fs::read_to_string(path).ok()?;
    let stem = path.file_stem()?.to_string_lossy().into_owned();

    if path.extension().and_then(|e| e.to_str()) == Some("json") {
        // Try deserialising directly; fall back to inserting the stem as name.
        let mut def: OutputStyleDef = serde_json::from_str(&content).ok()?;
        if def.name.is_empty() {
            def.name = stem;
        }
        return Some(def);
    }

    // Markdown format:
    //   Line 1:  # Label   (optional leading `#` and whitespace)
    //   Line 2:  description (short, plain text)
    //   Lines 3+: prompt text (everything after the blank / second line)
    let mut lines = content.lines();

    let raw_label = lines.next().unwrap_or("").trim().to_string();
    let label = raw_label.trim_start_matches('#').trim().to_string();
    let label = if label.is_empty() {
        stem.clone()
    } else {
        label
    };

    let description = lines
        .next()
        .map(|l| l.trim().to_string())
        .unwrap_or_default();

    // Collect remaining lines as the prompt, trimming leading blank lines.
    let prompt_lines: Vec<&str> = lines.collect();
    let prompt = prompt_lines.join("\n").trim().to_string();

    Some(OutputStyleDef {
        name: stem,
        label,
        description,
        prompt,
    })
}

// ---------------------------------------------------------------------------
// Aggregated access
// ---------------------------------------------------------------------------

/// Return all styles available for `config_dir`:
/// built-ins first, then styles from `<config_dir>/output-styles/`.
///
/// `config_dir` is typically `~/.claurst`.
pub fn all_styles(config_dir: &Path) -> Vec<OutputStyleDef> {
    let mut styles = builtin_styles();
    let user_dir = config_dir.join("output-styles");
    styles.extend(load_output_styles_dir(&user_dir));
    styles
}

/// Find a style by its `name` field.
pub fn find_style<'a>(styles: &'a [OutputStyleDef], name: &str) -> Option<&'a OutputStyleDef> {
    styles.iter().find(|s| s.name == name)
}

// ---------------------------------------------------------------------------
// Runtime style registry (populated by plugins at startup)
// ---------------------------------------------------------------------------

static RUNTIME_STYLES: Lazy<Mutex<Vec<OutputStyleDef>>> = Lazy::new(|| Mutex::new(Vec::new()));

/// Register an `OutputStyleDef` at runtime (called from plugin loading code).
///
/// Styles registered here are included in `all_styles_with_runtime` and
/// `find_style_runtime`.  Duplicate names are silently ignored so that
/// hot-reloading a plugin does not double-register styles.
pub fn register_runtime_style(style: OutputStyleDef) {
    if let Ok(mut list) = RUNTIME_STYLES.lock() {
        if !list.iter().any(|s| s.name == style.name) {
            list.push(style);
        }
    }
}

/// Return all runtime-registered styles.
pub fn runtime_styles() -> Vec<OutputStyleDef> {
    RUNTIME_STYLES.lock().map(|g| g.clone()).unwrap_or_default()
}

/// Like `all_styles`, but also includes runtime-registered plugin styles.
pub fn all_styles_with_runtime(config_dir: &Path) -> Vec<OutputStyleDef> {
    let mut styles = all_styles(config_dir);
    let rt = runtime_styles();
    for s in rt {
        if !styles.iter().any(|existing| existing.name == s.name) {
            styles.push(s);
        }
    }
    styles
}

/// Like `find_style`, but also searches runtime-registered plugin styles.
pub fn find_style_runtime<'a>(
    styles: &'a [OutputStyleDef],
    name: &str,
) -> Option<std::borrow::Cow<'a, OutputStyleDef>> {
    if let Some(s) = find_style(styles, name) {
        return Some(std::borrow::Cow::Borrowed(s));
    }
    // Fall back to runtime registry.
    if let Ok(rt) = RUNTIME_STYLES.lock() {
        if let Some(s) = rt.iter().find(|s| s.name == name) {
            return Some(std::borrow::Cow::Owned(s.clone()));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;
    use tempfile::TempDir;

    // ---- builtin_styles ----------------------------------------------------

    #[test]
    fn builtin_styles_non_empty() {
        assert!(!builtin_styles().is_empty());
    }

    #[test]
    fn builtin_styles_have_unique_names() {
        let styles = builtin_styles();
        let mut seen = std::collections::HashSet::new();
        for s in &styles {
            assert!(seen.insert(&s.name), "duplicate style name: {}", s.name);
        }
    }

    #[test]
    fn builtin_default_has_empty_prompt() {
        let def = OutputStyleDef::builtin_default();
        assert!(def.prompt.is_empty());
    }

    #[test]
    fn builtin_non_default_have_prompts() {
        for s in builtin_styles() {
            if s.name != "default" {
                assert!(
                    !s.prompt.is_empty(),
                    "style '{}' should have a non-empty prompt",
                    s.name
                );
            }
        }
    }

    // ---- find_style --------------------------------------------------------

    #[test]
    fn find_style_by_name() {
        let styles = builtin_styles();
        let found = find_style(&styles, "concise");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "concise");
    }

    #[test]
    fn find_style_missing() {
        let styles = builtin_styles();
        assert!(find_style(&styles, "nonexistent-xyz").is_none());
    }

    // ---- load_output_styles_dir (markdown) ---------------------------------

    fn write_file(dir: &TempDir, name: &str, content: &str) {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn load_markdown_style() {
        let dir = TempDir::new().unwrap();
        write_file(
            &dir,
            "terse.md",
            "# Terse\nVery short answers.\n\nOne sentence per response.",
        );
        let styles = load_output_styles_dir(dir.path());
        assert_eq!(styles.len(), 1);
        let s = &styles[0];
        assert_eq!(s.name, "terse");
        assert_eq!(s.label, "Terse");
        assert_eq!(s.description, "Very short answers.");
        assert_eq!(s.prompt, "One sentence per response.");
    }

    #[test]
    fn load_json_style() {
        let dir = TempDir::new().unwrap();
        write_file(
            &dir,
            "formal.json",
            r#"{"name":"formal","label":"Formal","description":"Formal tone.","prompt":"Use formal language."}"#,
        );
        let styles = load_output_styles_dir(dir.path());
        assert_eq!(styles.len(), 1);
        let s = &styles[0];
        assert_eq!(s.name, "formal");
        assert_eq!(s.label, "Formal");
        assert_eq!(s.prompt, "Use formal language.");
    }

    #[test]
    fn load_skips_unknown_extensions() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "ignore.txt", "should be skipped");
        let styles = load_output_styles_dir(dir.path());
        assert!(styles.is_empty());
    }

    #[test]
    fn load_non_existent_dir_returns_empty() {
        use std::path::PathBuf;
        let styles = load_output_styles_dir(&PathBuf::from("/nonexistent/path/xyz"));
        assert!(styles.is_empty());
    }

    #[test]
    fn load_multiple_styles_sorted() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "zebra.md", "# Zebra\nZ style.\n\nZ prompt.");
        write_file(&dir, "apple.md", "# Apple\nA style.\n\nA prompt.");
        let styles = load_output_styles_dir(dir.path());
        assert_eq!(styles[0].name, "apple");
        assert_eq!(styles[1].name, "zebra");
    }

    // ---- all_styles --------------------------------------------------------

    #[test]
    fn all_styles_includes_builtins() {
        let dir = TempDir::new().unwrap();
        // no output-styles subdir → only built-ins
        let styles = all_styles(dir.path());
        assert!(styles.iter().any(|s| s.name == "default"));
        assert!(styles.iter().any(|s| s.name == "concise"));
    }

    #[test]
    fn all_styles_merges_user_styles() {
        let dir = TempDir::new().unwrap();
        let output_styles_dir = dir.path().join("output-styles");
        std::fs::create_dir_all(&output_styles_dir).unwrap();

        // Write a user style file.
        let mut f = std::fs::File::create(output_styles_dir.join("pirate.md")).unwrap();
        f.write_all(b"# Pirate\nSpeak like a pirate.\n\nArrr matey!")
            .unwrap();

        let styles = all_styles(dir.path());
        assert!(styles.iter().any(|s| s.name == "pirate"));
        // Built-ins still present.
        assert!(styles.iter().any(|s| s.name == "default"));
    }
}
