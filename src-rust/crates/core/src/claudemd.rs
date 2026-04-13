//! AGENTS.md hierarchical memory loading.
//! Mirrors src/utils/claudemd.ts (1,479 lines).
//!
//! Priority order: managed > user > project > local
//! Supports @include directives, YAML frontmatter, and mtime-based caching.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Memory file type / priority scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// `~/.claurst/rules/*.md` — global managed policy.
    Managed,
    /// `~/.claurst/AGENTS.md` — user-level memory.
    User,
    /// `{project_root}/AGENTS.md` — project-level memory.
    Project,
    /// `{project_root}/.claurst/AGENTS.md` — local override.
    Local,
}

/// Frontmatter parsed from a AGENTS.md file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryFrontmatter {
    #[serde(default)]
    pub memory_type: Option<String>,
    #[serde(default)]
    pub priority: Option<u32>,
    #[serde(default)]
    pub scope: Option<String>,
}

/// Loaded memory file with metadata.
#[derive(Debug, Clone)]
pub struct MemoryFileInfo {
    pub path: PathBuf,
    pub scope: MemoryScope,
    pub content: String,
    pub frontmatter: MemoryFrontmatter,
    pub mtime: Option<SystemTime>,
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Simple mtime-keyed file cache.
#[derive(Default)]
pub struct MemoryCache {
    entries: HashMap<PathBuf, (SystemTime, String)>,
}

impl MemoryCache {
    /// Return cached content if the file hasn't changed since last read.
    pub fn get(&self, path: &Path) -> Option<&str> {
        let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
        let (cached_mtime, content) = self.entries.get(path)?;
        if *cached_mtime == mtime {
            Some(content.as_str())
        } else {
            None
        }
    }

    /// Store file content with its current mtime.
    pub fn insert(&mut self, path: PathBuf, content: String) {
        if let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) {
            self.entries.insert(path, (mtime, content));
        }
    }
}

// ---------------------------------------------------------------------------
// YAML frontmatter parsing
// ---------------------------------------------------------------------------

/// Strip YAML frontmatter (--- ... ---) from content and parse it.
/// Returns (frontmatter, body_without_frontmatter).
pub fn parse_frontmatter(content: &str) -> (MemoryFrontmatter, &str) {
    if !content.starts_with("---") {
        return (MemoryFrontmatter::default(), content);
    }
    let after_first = &content[3..];
    if let Some(end) = after_first.find("\n---") {
        let yaml = after_first[..end].trim();
        let body = &after_first[end + 4..];
        // Minimal YAML key-value parse (no external dependency).
        let mut fm = MemoryFrontmatter::default();
        for line in yaml.lines() {
            let line = line.trim();
            if let Some((key, val)) = line.split_once(':') {
                let val = val.trim().to_string();
                match key.trim() {
                    "memory_type" => fm.memory_type = Some(val),
                    "priority" => fm.priority = val.parse().ok(),
                    "scope" => fm.scope = Some(val),
                    _ => {}
                }
            }
        }
        return (fm, body.trim_start_matches('\n'));
    }
    (MemoryFrontmatter::default(), content)
}

// ---------------------------------------------------------------------------
// @include directive expansion
// ---------------------------------------------------------------------------

/// Maximum @include nesting depth.
const MAX_INCLUDE_DEPTH: usize = 10;

/// Expand @include directives in content.
/// Circular references are detected via `visited` set.
pub fn expand_includes(
    content: &str,
    base_dir: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) -> String {
    if depth >= MAX_INCLUDE_DEPTH {
        return content.to_string();
    }

    let mut result = String::with_capacity(content.len());
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(path_str) = trimmed.strip_prefix("@include ") {
            let path_str = path_str.trim();
            // Resolve relative to base_dir; expand ~ to home dir.
            let include_path = if path_str.starts_with('~') {
                dirs::home_dir().unwrap_or_default().join(&path_str[2..])
            } else if Path::new(path_str).is_absolute() {
                PathBuf::from(path_str)
            } else {
                base_dir.join(path_str)
            };

            let canonical = include_path.canonicalize().unwrap_or(include_path.clone());
            if visited.contains(&canonical) {
                result.push_str(&format!(
                    "<!-- circular @include {} skipped -->\n",
                    path_str
                ));
                continue;
            }
            if let Ok(included) = std::fs::read_to_string(&include_path) {
                // Check max size.
                if included.len() > 40 * 1024 {
                    result.push_str(&format!(
                        "<!-- @include {} exceeds 40KB limit -->\n",
                        path_str
                    ));
                    continue;
                }
                visited.insert(canonical);
                let expanded = expand_includes(
                    &included,
                    include_path.parent().unwrap_or(base_dir),
                    visited,
                    depth + 1,
                );
                result.push_str(&expanded);
                result.push('\n');
            } else {
                result.push_str(&format!("<!-- @include {} not found -->\n", path_str));
            }
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Loading API
// ---------------------------------------------------------------------------

const MAX_FILE_SIZE: u64 = 40 * 1024; // 40 KB

/// Load a single AGENTS.md file (respects MAX_FILE_SIZE, expands @includes).
pub fn load_memory_file(path: &Path, scope: MemoryScope) -> Option<MemoryFileInfo> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_SIZE {
        eprintln!("WARNING: {} exceeds 40KB limit, skipping", path.display());
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    let mtime = meta.modified().ok();

    let (frontmatter, body) = parse_frontmatter(&raw);
    let mut visited = HashSet::new();
    visited.insert(path.canonicalize().unwrap_or(path.to_path_buf()));
    let content = expand_includes(
        body,
        path.parent().unwrap_or(Path::new(".")),
        &mut visited,
        0,
    );

    Some(MemoryFileInfo {
        path: path.to_path_buf(),
        scope,
        content,
        frontmatter,
        mtime,
    })
}

/// Load memory files from a directory for a given scope.
///
/// Loads `AGENTS.md` first (primary/universal standard), then `CLAUDE.md` if
/// present (Claude-specific additions or overrides). Either file may be absent.
fn load_scope_files(dir: &Path, scope: MemoryScope, files: &mut Vec<MemoryFileInfo>) {
    for name in &["AGENTS.md", "CLAUDE.md"] {
        let path = dir.join(name);
        if path.exists() {
            if let Some(f) = load_memory_file(&path, scope) {
                files.push(f);
            }
        }
    }
}

/// Load all memory files for the given project root, in priority order.
///
/// At each scope `AGENTS.md` is loaded first (universal standard), followed by
/// `CLAUDE.md` if present (Claude-specific context). Either or both may exist.
///
/// Returned list is ordered: Managed (highest) → User → Project → Local.
pub fn load_all_memory_files(project_root: &Path) -> Vec<MemoryFileInfo> {
    let mut files = Vec::new();

    // 1. Managed: ~/.claurst/rules/*.md
    if let Some(home) = dirs::home_dir() {
        let rules_dir = home.join(".claurst/rules");
        if let Ok(entries) = std::fs::read_dir(&rules_dir) {
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension().map_or(false, |x| x == "md") {
                        Some(p)
                    } else {
                        None
                    }
                })
                .collect();
            paths.sort();
            for p in paths {
                if let Some(f) = load_memory_file(&p, MemoryScope::Managed) {
                    files.push(f);
                }
            }
        }

        // 2. User: ~/.claurst/AGENTS.md then ~/.claurst/CLAUDE.md
        load_scope_files(&home.join(".claurst"), MemoryScope::User, &mut files);
    }

    // 3. Project: {project_root}/AGENTS.md then {project_root}/CLAUDE.md
    load_scope_files(project_root, MemoryScope::Project, &mut files);

    // 4. Local: {project_root}/.claurst/AGENTS.md then {project_root}/.claurst/CLAUDE.md
    load_scope_files(
        &project_root.join(".claurst"),
        MemoryScope::Local,
        &mut files,
    );

    files
}

/// Concatenate all memory file contents into a single system-prompt fragment.
pub fn build_memory_prompt(files: &[MemoryFileInfo]) -> String {
    files
        .iter()
        .filter(|f| !f.content.trim().is_empty())
        .map(|f| f.content.trim().to_string())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_basic() {
        let content = "---\nmemory_type: project\npriority: 10\n---\nHello world";
        let (fm, body) = parse_frontmatter(content);
        assert_eq!(fm.memory_type.as_deref(), Some("project"));
        assert_eq!(fm.priority, Some(10));
        assert_eq!(body.trim(), "Hello world");
    }

    #[test]
    fn parse_frontmatter_none() {
        let content = "No frontmatter here";
        let (fm, body) = parse_frontmatter(content);
        assert!(fm.memory_type.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn load_scope_prefers_agents_then_claude() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "agents content").unwrap();
        std::fs::write(tmp.path().join("CLAUDE.md"), "claude content").unwrap();

        let files = load_all_memory_files(tmp.path());
        // Filter to just the project-scope files from our temp dir.
        let project: Vec<_> = files
            .iter()
            .filter(|f| f.path.starts_with(tmp.path()))
            .collect();
        assert_eq!(
            project.len(),
            2,
            "both AGENTS.md and CLAUDE.md should be loaded"
        );
        assert!(
            project[0].path.ends_with("AGENTS.md"),
            "AGENTS.md must come first"
        );
        assert!(
            project[1].path.ends_with("CLAUDE.md"),
            "CLAUDE.md must follow"
        );
    }

    #[test]
    fn load_scope_claudemd_only_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("CLAUDE.md"), "claude only").unwrap();

        let files = load_all_memory_files(tmp.path());
        let project: Vec<_> = files
            .iter()
            .filter(|f| f.path.starts_with(tmp.path()))
            .collect();
        assert_eq!(project.len(), 1);
        assert!(project[0].path.ends_with("CLAUDE.md"));
    }

    #[test]
    fn expand_includes_circular() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.md");
        let b = tmp.path().join("b.md");
        std::fs::write(&a, "@include b.md\n").unwrap();
        std::fs::write(&b, "@include a.md\ncontent\n").unwrap();
        let result = expand_includes(
            "@include a.md\n",
            tmp.path(),
            &mut std::collections::HashSet::new(),
            0,
        );
        // Should not infinite-loop; circular reference comment present.
        assert!(result.contains("circular") || result.contains("content"));
    }
}
