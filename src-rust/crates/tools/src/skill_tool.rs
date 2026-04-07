// SkillTool: execute user-defined skill (prompt template) files programmatically.
//
// Skills are Markdown files stored in:
//   <project>/.claurst/commands/<name>.md
//   ~/.claurst/commands/<name>.md
//
// Bundled skills (defined in bundled_skills.rs) are checked first before the
// disk directories, so they take precedence over same-named .md files.
//
// The model invokes this tool to expand a skill's prompt inline.
// Supports $ARGUMENTS placeholder substitution.
// Use skill="list" to discover available skills.

use crate::bundled_skills::{expand_prompt, find_bundled_skill, user_invocable_skills};
use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use tracing::debug;

pub struct SkillTool;

#[derive(Debug, Deserialize)]
struct SkillInput {
    skill: String,
    #[serde(default)]
    args: Option<String>,
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn description(&self) -> &str {
        "Execute a skill (custom prompt template) by name. \
         Skills are .md files in .claurst/commands/ or ~/.claurst/commands/. \
         Use skill=\"list\" to discover available skills. \
         The expanded skill prompt is returned for you to act on."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Skill name (without .md extension), or \"list\" to enumerate skills"
                },
                "args": {
                    "type": "string",
                    "description": "Arguments passed to the skill — replaces $ARGUMENTS in the template"
                }
            },
            "required": ["skill"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: SkillInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        let dirs = skill_search_dirs(ctx);

        if params.skill == "list" {
            return list_skills(&dirs).await;
        }

        let skill_name = params.skill.trim_end_matches(".md");
        debug!(skill = skill_name, "Loading skill");

        // Check bundled skills first — they take precedence over disk files.
        if let Some(bundled) = find_bundled_skill(skill_name) {
            let args = params.args.as_deref().unwrap_or("");
            let prompt = expand_prompt(bundled, args);
            let prompt = prompt.trim().to_string();
            if prompt.is_empty() {
                return ToolResult::error(format!(
                    "Bundled skill '{}' expanded to empty content.",
                    skill_name
                ));
            }
            return ToolResult::success(prompt);
        }

        let raw = match find_and_read_skill(skill_name, &dirs).await {
            Some(c) => c,
            None => {
                return ToolResult::error(format!(
                    "Skill '{}' not found. Use skill=\"list\" to see available skills.",
                    skill_name
                ));
            }
        };

        // Strip YAML frontmatter if present (--- ... ---)
        let content = strip_frontmatter(&raw);

        // Substitute $ARGUMENTS
        let prompt = if let Some(args) = &params.args {
            content.replace("$ARGUMENTS", args)
        } else {
            content.replace("$ARGUMENTS", "")
        };

        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return ToolResult::error(format!("Skill '{}' expanded to empty content.", skill_name));
        }

        ToolResult::success(prompt)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn skill_search_dirs(ctx: &ToolContext) -> Vec<PathBuf> {
    let mut dirs = vec![ctx.working_dir.join(".claurst").join("commands")];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".claurst").join("commands"));
    }
    dirs
}

async fn list_skills(dirs: &[PathBuf]) -> ToolResult {
    // Start with the bundled skills.
    let mut lines: Vec<String> = Vec::new();
    let bundled = user_invocable_skills();
    for (name, desc) in &bundled {
        lines.push(format!("  {} — {} [bundled]", name, desc));
    }
    let bundled_names: Vec<&str> = bundled.iter().map(|(n, _)| *n).collect();

    // Then add disk skills, skipping any that shadow a bundled name.
    let mut disk_skills: Vec<(String, PathBuf)> = Vec::new();
    for dir in dirs {
        match tokio::fs::read_dir(dir).await {
            Ok(mut entries) => {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "md") {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            let name = stem.to_string();
                            // Deduplicate — project-level shadows user-level;
                            // bundled skills shadow everything.
                            if !disk_skills.iter().any(|(n, _)| n == &name)
                                && !bundled_names.contains(&name.as_str())
                            {
                                disk_skills.push((name, path));
                            }
                        }
                    }
                }
            }
            Err(_) => {} // directory doesn't exist, skip
        }
    }

    disk_skills.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, path) in &disk_skills {
        let desc = read_skill_description(path).await;
        lines.push(format!("  {} — {}", name, desc));
    }

    let total = bundled.len() + disk_skills.len();
    if total == 0 {
        return ToolResult::success(
            "No skills found. Create .md files in .claurst/commands/ to define skills.\n\
             Example: .claurst/commands/review.md"
                .to_string(),
        );
    }

    ToolResult::success(format!(
        "Available skills ({}):\n{}",
        total,
        lines.join("\n")
    ))
}

async fn find_and_read_skill(name: &str, dirs: &[PathBuf]) -> Option<String> {
    for dir in dirs {
        let path = dir.join(format!("{}.md", name));
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            return Some(content);
        }
    }
    None
}

async fn read_skill_description(path: &std::path::Path) -> String {
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return "(no description)".to_string();
    };
    let body = strip_frontmatter(&content);
    // First non-empty, non-heading line
    for line in body.lines() {
        let t = line.trim().trim_start_matches('#').trim();
        if !t.is_empty() {
            let truncated = if t.len() > 80 { &t[..80] } else { t };
            return truncated.to_string();
        }
    }
    "(no description)".to_string()
}

/// Remove YAML frontmatter delimited by `---` at the start of the file.
fn strip_frontmatter(content: &str) -> String {
    if content.starts_with("---") {
        // Find closing ---
        let after_open = &content[3..];
        if let Some(close_pos) = after_open.find("\n---") {
            // Skip past the closing delimiter and any leading newline
            let rest = &after_open[close_pos + 4..];
            return rest.trim_start_matches('\n').to_string();
        }
    }
    content.to_string()
}
