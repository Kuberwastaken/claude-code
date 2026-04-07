//! Skill prefetch — mirrors src/services/skillSearch/prefetch.js
//!
//! Reads all bundled and user-defined skill definitions in the background
//! and builds a searchable index. The query loop injects the skill listing
//! as a tool-context attachment when the index is ready.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A single skill definition.
#[derive(Debug, Clone)]
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    /// Source: "bundled" | "user" | "plugin:{name}"
    pub source: String,
    /// Path to the skill file on disk.
    pub path: Option<std::path::PathBuf>,
}

/// In-memory skill search index.
#[derive(Debug, Default)]
pub struct SkillIndex {
    /// All skills, keyed by name (lowercase).
    skills: HashMap<String, SkillDefinition>,
}

impl SkillIndex {
    /// Add a skill to the index.
    pub fn insert(&mut self, skill: SkillDefinition) {
        self.skills.insert(skill.name.to_lowercase(), skill);
    }

    /// Query by partial name or tag match (case-insensitive).
    pub fn search(&self, query: &str) -> Vec<&SkillDefinition> {
        let q = query.to_lowercase();
        self.skills
            .values()
            .filter(|s| {
                s.name.to_lowercase().contains(&q)
                    || s.description.to_lowercase().contains(&q)
                    || s.tags.iter().any(|t| t.to_lowercase().contains(&q))
            })
            .collect()
    }

    /// Return all skills.
    pub fn all(&self) -> Vec<&SkillDefinition> {
        self.skills.values().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }
}

/// Shared handle to the skill index (populated in the background).
pub type SharedSkillIndex = Arc<RwLock<SkillIndex>>;

/// Scan `project_root` for skill definitions in `.claurst/skills/` and the bundled
/// skill list, build the index, and store it in `index`.
///
/// This runs as a `tokio::task::spawn` parallel to model streaming.
pub async fn prefetch_skills(project_root: &Path, index: SharedSkillIndex) {
    let mut local = SkillIndex::default();

    // 1. User-defined skills: ~/.claurst/skills/*.md + {project_root}/.claurst/skills/*.md
    let search_dirs: Vec<std::path::PathBuf> = {
        let mut dirs = Vec::new();
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".claurst").join("skills"));
        }
        dirs.push(project_root.join(".claurst").join("skills"));
        dirs
    };

    for dir in &search_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "md") {
                    if let Some(skill) = load_skill_from_file(&path) {
                        local.insert(skill);
                    }
                }
            }
        }
    }

    // 2. Bundled skills: check if we ship any in a `skills/` directory next to the binary.
    if let Ok(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .ok_or(())
    {
        let bundled = exe_dir.join("skills");
        if let Ok(entries) = std::fs::read_dir(&bundled) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "md") {
                    if let Some(mut skill) = load_skill_from_file(&path) {
                        skill.source = "bundled".to_string();
                        local.insert(skill);
                    }
                }
            }
        }
    }

    // Write the index once loaded.
    let mut guard = index.write().await;
    *guard = local;
}

/// Parse a skill Markdown file into a `SkillDefinition`.
///
/// Expected format:
/// ```text
/// ---
/// name: my-skill
/// description: Does something useful
/// tags: [tag1, tag2]
/// ---
///
/// Skill instructions here...
/// ```
fn load_skill_from_file(path: &std::path::Path) -> Option<SkillDefinition> {
    let content = std::fs::read_to_string(path).ok()?;
    let stem = path.file_stem()?.to_string_lossy().to_string();

    // Try to parse front-matter
    if content.starts_with("---") {
        let end = content[3..].find("\n---")? + 3;
        let front = &content[3..end];
        let name = extract_yaml_str(front, "name").unwrap_or_else(|| stem.clone());
        let description = extract_yaml_str(front, "description").unwrap_or_default();
        let tags = extract_yaml_list(front, "tags");
        Some(SkillDefinition {
            name,
            description,
            tags,
            source: "user".to_string(),
            path: Some(path.to_path_buf()),
        })
    } else {
        // No front-matter: use filename as name
        Some(SkillDefinition {
            name: stem,
            description: content.lines().next().unwrap_or("").to_string(),
            tags: Vec::new(),
            source: "user".to_string(),
            path: Some(path.to_path_buf()),
        })
    }
}

fn extract_yaml_str(front: &str, key: &str) -> Option<String> {
    for line in front.lines() {
        if let Some(rest) = line.strip_prefix(&format!("{key}:")) {
            return Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

fn extract_yaml_list(front: &str, key: &str) -> Vec<String> {
    for line in front.lines() {
        if let Some(rest) = line.strip_prefix(&format!("{key}:")) {
            let rest = rest.trim().trim_matches('[').trim_matches(']');
            return rest
                .split(',')
                .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    Vec::new()
}

/// Format a skill listing attachment for injection into the conversation.
pub fn format_skill_listing(index: &SkillIndex) -> String {
    if index.is_empty() {
        return String::new();
    }
    let mut out = String::from("Available skills:\n");
    let mut skills: Vec<_> = index.all();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    for skill in skills {
        let tags = if skill.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", skill.tags.join(", "))
        };
        out.push_str(&format!(
            "  /{} — {}{}\n",
            skill.name, skill.description, tags
        ));
    }
    out
}
