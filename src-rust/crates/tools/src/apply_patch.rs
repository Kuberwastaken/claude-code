// ApplyPatch tool: apply a unified diff patch to files.
//
// Parses standard unified diff format (as produced by `git diff` or `diff -u`).
// Supports dry_run mode which validates the patch without writing any files.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::debug;

pub struct ApplyPatchTool;

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ApplyPatchInput {
    patch: String,
    #[serde(default)]
    dry_run: bool,
}

// ---------------------------------------------------------------------------
// Internal diff representation
// ---------------------------------------------------------------------------

/// A single `@@` hunk within a file diff.
#[derive(Debug)]
struct Hunk {
    /// Starting line in the *original* file (0-based index).
    orig_start: usize,
    /// Lines in this hunk: `' '` = context, `'-'` = remove, `'+'` = add.
    lines: Vec<(char, String)>,
}

/// All hunks for a single file.
#[derive(Debug)]
struct FilePatch {
    /// Target path (from `+++ b/<path>` or `+++ <path>`).
    path: String,
    hunks: Vec<Hunk>,
}

// ---------------------------------------------------------------------------
// Unified diff parser
// ---------------------------------------------------------------------------

/// Parse a unified diff string into a list of `FilePatch` objects.
fn parse_unified_diff(patch: &str) -> Result<Vec<FilePatch>, String> {
    let mut file_patches: Vec<FilePatch> = Vec::new();
    let mut current_file: Option<FilePatch> = None;
    let mut current_hunk: Option<Hunk> = None;

    for line in patch.lines() {
        if line.starts_with("--- ") {
            // Start of a new file section; finalise previous hunk/file.
            if let Some(h) = current_hunk.take() {
                if let Some(ref mut f) = current_file {
                    f.hunks.push(h);
                }
            }
            if let Some(f) = current_file.take() {
                file_patches.push(f);
            }
            // Don't extract the path here — we do it from the +++ line.
        } else if line.starts_with("+++ ") {
            // Extract target path, stripping the "b/" prefix if present.
            let raw = &line[4..];
            let path = raw.trim_start_matches("b/").trim().to_string();
            current_file = Some(FilePatch {
                path,
                hunks: Vec::new(),
            });
        } else if line.starts_with("@@ ") {
            // Finalise the previous hunk.
            if let Some(h) = current_hunk.take() {
                if let Some(ref mut f) = current_file {
                    f.hunks.push(h);
                }
            }
            // Parse `@@ -l,s +l,s @@` — we only need the original start line.
            let orig_start = parse_hunk_header(line)?;
            current_hunk = Some(Hunk {
                orig_start,
                lines: Vec::new(),
            });
        } else if let Some(ref mut hunk) = current_hunk {
            if line.starts_with('+') {
                hunk.lines.push(('+', line[1..].to_string()));
            } else if line.starts_with('-') {
                hunk.lines.push(('-', line[1..].to_string()));
            } else if line.starts_with(' ') {
                hunk.lines.push((' ', line[1..].to_string()));
            } else if line.starts_with('\\') {
                // "\ No newline at end of file" — ignore.
            }
            // Skip other lines (empty, etc.)
        }
    }

    // Flush remaining hunk / file.
    if let Some(h) = current_hunk.take() {
        if let Some(ref mut f) = current_file {
            f.hunks.push(h);
        }
    }
    if let Some(f) = current_file.take() {
        file_patches.push(f);
    }

    Ok(file_patches)
}

/// Extract the original-file start line (1-based in the header, converted to
/// 0-based internally) from a `@@ -l,s +l,s @@` header line.
fn parse_hunk_header(line: &str) -> Result<usize, String> {
    // Format: "@@ -orig_start[,orig_count] +new_start[,new_count] @@"
    let after_at = line
        .strip_prefix("@@ ")
        .ok_or_else(|| format!("Invalid hunk header: {}", line))?;

    let minus_part = after_at
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("Malformed hunk header (no -part): {}", line))?;

    // minus_part is like "-12,5" or "-12"
    let digits = minus_part
        .trim_start_matches('-')
        .split(',')
        .next()
        .unwrap_or("0");

    let line_num: usize = digits
        .parse()
        .map_err(|_| format!("Could not parse line number from: {}", minus_part))?;

    // Header is 1-based; convert to 0-based.
    Ok(if line_num > 0 { line_num - 1 } else { 0 })
}

// ---------------------------------------------------------------------------
// Hunk application
// ---------------------------------------------------------------------------

/// Apply a single `Hunk` to `lines` (the split lines of the file content).
///
/// Returns the modified line vector, or an error describing why the context
/// did not match.
fn apply_hunk(lines: Vec<String>, hunk: &Hunk) -> Result<Vec<String>, String> {
    // Collect the context / removal lines we expect to find.
    let expected: Vec<&str> = hunk
        .lines
        .iter()
        .filter(|(c, _)| *c == ' ' || *c == '-')
        .map(|(_, l)| l.as_str())
        .collect();

    if expected.is_empty() {
        // Pure insertion — we'll handle it below without a search.
    }

    // Find the position in `lines` where the context starts.
    // We start searching from `orig_start` (the hint) but fall back to a
    // full scan if the hint is off (e.g. after earlier hunks shifted lines).
    let search_start = hunk.orig_start.min(lines.len());
    let pos = find_context_position(&lines, &expected, search_start).ok_or_else(|| {
        format!(
            "Context not found near line {} (looking for {} lines of context/removes)",
            hunk.orig_start + 1,
            expected.len()
        )
    })?;

    // Build the replacement: remove '-' and ' ' lines at `pos`, insert '+' and ' '.
    let mut output_prefix = lines[..pos].to_vec();
    let mut output_suffix = lines[pos..].to_vec();

    // Skip past the lines that the hunk covers (context + removals).
    let consume = expected.len();
    if consume > output_suffix.len() {
        return Err(format!(
            "Hunk extends beyond end of file at line {}",
            pos + 1
        ));
    }
    let remaining = output_suffix.split_off(consume);

    // Build the replacement content from the hunk.
    let mut replacement: Vec<String> = Vec::new();
    for (ch, content) in &hunk.lines {
        match ch {
            '+' | ' ' => replacement.push(content.clone()),
            '-' => {} // removed — skip
            _ => {}
        }
    }

    output_prefix.append(&mut replacement);
    output_prefix.extend(remaining);
    Ok(output_prefix)
}

/// Search for `expected` lines starting at `hint` and falling back to a full scan.
fn find_context_position(lines: &[String], expected: &[&str], hint: usize) -> Option<usize> {
    if expected.is_empty() {
        // A pure-insertion hunk always applies at the hint position.
        return Some(hint.min(lines.len()));
    }

    let n = lines.len();
    let max_start = if n >= expected.len() {
        n - expected.len()
    } else {
        return None;
    };

    // Try hint first, then scan forward and backward.
    let candidates: Vec<usize> = std::iter::once(hint).chain(0..=max_start).collect();

    for &start in &candidates {
        if start > max_start {
            continue;
        }
        if lines[start..start + expected.len()]
            .iter()
            .zip(expected.iter())
            .all(|(l, e)| l.as_str() == *e)
        {
            return Some(start);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        claurst_core::constants::TOOL_NAME_APPLY_PATCH
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to files. The patch must be in standard unified \
         diff format (as produced by `git diff` or `diff -u`). Set dry_run=true to \
         validate the patch without writing any changes."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Unified diff patch content"
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "If true, validate the patch without applying it (default: false)"
                }
            },
            "required": ["patch"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: ApplyPatchInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        if params.patch.trim().is_empty() {
            return ToolResult::error("patch must not be empty".to_string());
        }

        // Parse the unified diff.
        let file_patches = match parse_unified_diff(&params.patch) {
            Ok(fp) => fp,
            Err(e) => return ToolResult::error(format!("Failed to parse patch: {}", e)),
        };

        if file_patches.is_empty() {
            return ToolResult::error(
                "No file diffs found in patch (expected --- / +++ headers)".to_string(),
            );
        }

        // Permission check.
        if !params.dry_run {
            if let Err(e) = ctx.check_permission(
                self.name(),
                &format!("ApplyPatch to {} file(s)", file_patches.len()),
                false,
            ) {
                return ToolResult::error(e.to_string());
            }
        }

        // ----------------------------------------------------------------
        // Process each file in the patch
        // ----------------------------------------------------------------

        let mut total_added: i64 = 0;
        let mut total_removed: i64 = 0;
        let mut file_summaries: Vec<Value> = Vec::new();
        // (path, original, new_content) — built during validation, used for writes
        let mut to_write: Vec<(std::path::PathBuf, Vec<u8>, String)> = Vec::new();

        for fp in &file_patches {
            let path = ctx.resolve_path(&fp.path);
            debug!(path = %path.display(), "ApplyPatch processing file");

            // Read current content (or empty string for new files).
            let original_content = if path.exists() {
                match tokio::fs::read_to_string(&path).await {
                    Ok(c) => c,
                    Err(e) => {
                        return ToolResult::error(format!("Cannot read {}: {}", path.display(), e))
                    }
                }
            } else {
                String::new()
            };

            // Split into lines (keep newlines for join later).
            let mut lines: Vec<String> = original_content.lines().map(|l| l.to_string()).collect();

            let mut file_added: i64 = 0;
            let mut file_removed: i64 = 0;

            // Apply hunks in order.
            for (hunk_idx, hunk) in fp.hunks.iter().enumerate() {
                let added = hunk.lines.iter().filter(|(c, _)| *c == '+').count() as i64;
                let removed = hunk.lines.iter().filter(|(c, _)| *c == '-').count() as i64;

                lines = match apply_hunk(lines, hunk) {
                    Ok(l) => l,
                    Err(e) => {
                        return ToolResult::error(format!(
                            "Failed to apply hunk {} in {}: {}",
                            hunk_idx + 1,
                            fp.path,
                            e
                        ));
                    }
                };

                file_added += added;
                file_removed += removed;
            }

            total_added += file_added;
            total_removed += file_removed;

            let new_content = if lines.is_empty() {
                String::new()
            } else {
                // Re-join with newlines; preserve trailing newline if original had one.
                let mut s = lines.join("\n");
                if original_content.ends_with('\n') || original_content.is_empty() {
                    s.push('\n');
                }
                s
            };

            file_summaries.push(json!({
                "path": fp.path,
                "hunks": fp.hunks.len(),
                "lines_added": file_added,
                "lines_removed": file_removed,
            }));

            to_write.push((path, original_content.into_bytes(), new_content));
        }

        // ----------------------------------------------------------------
        // Dry-run: return summary without writing
        // ----------------------------------------------------------------

        if params.dry_run {
            return ToolResult::success(format!(
                "Dry run: patch would modify {} file(s) (+{} -{} lines).",
                to_write.len(),
                total_added,
                total_removed,
            ))
            .with_metadata(json!({
                "dry_run": true,
                "files": file_summaries,
                "total_lines_added": total_added,
                "total_lines_removed": total_removed,
            }));
        }

        // ----------------------------------------------------------------
        // Write all modified files
        // ----------------------------------------------------------------

        for (path, original_bytes, new_content) in &to_write {
            if let Err(e) = tokio::fs::write(path, new_content).await {
                return ToolResult::error(format!("Failed to write {}: {}", path.display(), e));
            }
            ctx.record_file_change(
                path.clone(),
                original_bytes,
                new_content.as_bytes(),
                self.name(),
            );
        }

        ToolResult::success(format!(
            "Applied patch to {} file(s) (+{} -{} lines).",
            to_write.len(),
            total_added,
            total_removed,
        ))
        .with_metadata(json!({
            "dry_run": false,
            "files": file_summaries,
            "total_lines_added": total_added,
            "total_lines_removed": total_removed,
        }))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hunk_header() {
        assert_eq!(parse_hunk_header("@@ -12,5 +12,6 @@").unwrap(), 11);
        assert_eq!(parse_hunk_header("@@ -1,3 +1,4 @@ fn foo()").unwrap(), 0);
        assert_eq!(parse_hunk_header("@@ -0,0 +1 @@").unwrap(), 0);
    }

    #[test]
    fn test_apply_hunk_simple() {
        let lines: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let hunk = Hunk {
            orig_start: 1,
            lines: vec![(' ', "b".into()), ('-', "c".into()), ('+', "C".into())],
        };
        let result = apply_hunk(lines, &hunk).unwrap();
        assert_eq!(result, vec!["a", "b", "C"]);
    }

    #[test]
    fn test_apply_hunk_context_mismatch() {
        let lines: Vec<String> = vec!["x".into(), "y".into()];
        let hunk = Hunk {
            orig_start: 0,
            lines: vec![('-', "z".into())],
        };
        assert!(apply_hunk(lines, &hunk).is_err());
    }

    #[test]
    fn test_parse_unified_diff_basic() {
        let patch = "\
--- a/foo.txt
+++ b/foo.txt
@@ -1,2 +1,2 @@
 hello
-world
+rust
";
        let fps = parse_unified_diff(patch).unwrap();
        assert_eq!(fps.len(), 1);
        assert_eq!(fps[0].path, "foo.txt");
        assert_eq!(fps[0].hunks.len(), 1);
        let hunk = &fps[0].hunks[0];
        assert_eq!(hunk.orig_start, 0);
        assert_eq!(hunk.lines.len(), 3);
    }

    #[test]
    fn test_parse_unified_diff_two_files() {
        let patch = "\
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-foo
+bar
";
        let fps = parse_unified_diff(patch).unwrap();
        assert_eq!(fps.len(), 2);
        assert_eq!(fps[0].path, "a.rs");
        assert_eq!(fps[1].path, "b.rs");
    }
}
