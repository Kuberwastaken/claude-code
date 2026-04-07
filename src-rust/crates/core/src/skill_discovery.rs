//! Skill discovery: load custom prompt-template skills from markdown files
//! on disk and (optionally) from git URLs.
//!
//! Search priority (first match wins for a given skill name):
//!   1. Project `.claurst/skills/` — walk up from `cwd`
//!   2. Project `.agents/skills/`  — walk up from `cwd`
//!   3. Global `~/.claurst/skills/`
//!   4. Configured extra paths from `SkillsConfig.paths`
//!   5. Git-URL repos from `SkillsConfig.urls` (cloned once, then cached)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A discovered skill loaded from a markdown file.
#[derive(Debug, Clone)]
pub struct DiscoveredSkill {
    /// Skill name (from `name:` frontmatter or file stem).
    pub name: String,
    /// One-line description (from `description:` frontmatter or default).
    pub description: String,
    /// The prompt body after stripping frontmatter.
    pub template: String,
    /// Absolute path to the source `.md` file.
    pub source_path: PathBuf,
}

// ---------------------------------------------------------------------------
// Frontmatter parsing
// ---------------------------------------------------------------------------

/// Parse a skill markdown file.
///
/// Expects optional YAML frontmatter delimited by `---`.
/// Returns `None` when the file is empty after trimming.
pub fn parse_skill_file(content: &str, path: &Path) -> Option<DiscoveredSkill> {
    let content = content.trim();
    if content.is_empty() {
        return None;
    }

    let (name, description, template) = if content.starts_with("---") {
        let after_open = &content[3..];
        // Accept both `\n---` and `\r\n---` as closing delimiter.
        if let Some(close_pos) = after_open.find("\n---") {
            let frontmatter = &after_open[..close_pos];
            let rest = after_open[close_pos + 4..].trim_start_matches(['\r', '\n']);

            let mut name: Option<String> = None;
            let mut description: Option<String> = None;

            for line in frontmatter.lines() {
                let line = line.trim();
                if let Some(v) = line.strip_prefix("name:") {
                    name = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
                } else if let Some(v) = line.strip_prefix("description:") {
                    description = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
                }
            }

            (name, description, rest.to_string())
        } else {
            // Malformed frontmatter — treat entire content as template.
            (None, None, content.to_string())
        }
    } else {
        (None, None, content.to_string())
    };

    let name = name.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string()
    });
    let description = description.unwrap_or_else(|| "Custom skill".to_string());

    if template.is_empty() && name.is_empty() {
        return None;
    }

    Some(DiscoveredSkill {
        name,
        description,
        template,
        source_path: path.to_path_buf(),
    })
}

// ---------------------------------------------------------------------------
// Directory scanning
// ---------------------------------------------------------------------------

/// Scan a single directory for `*.md` skill files.
fn scan_dir(dir: &Path) -> Vec<DiscoveredSkill> {
    let mut skills = Vec::new();
    if !dir.is_dir() {
        return skills;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::debug!(dir = %dir.display(), error = %err, "skill_discovery: read_dir failed");
            return skills;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    if let Some(skill) = parse_skill_file(&content, &path) {
                        skills.push(skill);
                    }
                }
                Err(err) => {
                    tracing::debug!(path = %path.display(), error = %err, "skill_discovery: read failed");
                }
            }
        }
    }

    skills
}

// ---------------------------------------------------------------------------
// Top-level discovery
// ---------------------------------------------------------------------------

/// Discover all skills from all configured sources.
///
/// Returns a `HashMap` of `skill_name → DiscoveredSkill` (first match wins;
/// duplicates from lower-priority sources are warned via `tracing::warn`).
pub fn discover_skills(
    cwd: &Path,
    config_skills: &crate::config::SkillsConfig,
) -> HashMap<String, DiscoveredSkill> {
    let mut all: HashMap<String, DiscoveredSkill> = HashMap::new();
    let mut warn_duplicates: Vec<String> = Vec::new();

    // Inline closure: insert a batch, warning on duplicates.
    let mut add = |skills: Vec<DiscoveredSkill>| {
        for skill in skills {
            if let Some(existing) = all.get(&skill.name) {
                warn_duplicates.push(format!(
                    "Duplicate skill '{}' found at {} (keeping {})",
                    skill.name,
                    skill.source_path.display(),
                    existing.source_path.display()
                ));
            } else {
                all.insert(skill.name.clone(), skill);
            }
        }
    };

    // ---- 1. Project skills: walk up from cwd --------------------------------
    {
        let mut dir: &Path = cwd;
        loop {
            add(scan_dir(&dir.join(".claurst").join("skills")));
            add(scan_dir(&dir.join(".agents").join("skills")));
            match dir.parent() {
                Some(parent) if parent != dir => dir = parent,
                _ => break,
            }
        }
    }

    // ---- 2. Global skills: ~/.claurst/skills/ --------------------------------
    if let Some(home) = dirs::home_dir() {
        add(scan_dir(&home.join(".claurst").join("skills")));
    }

    // ---- 3. Configured extra paths ------------------------------------------
    for path_str in &config_skills.paths {
        let path = Path::new(path_str);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        add(scan_dir(&path));
    }

    // ---- 4. Git URL skills (cached) -----------------------------------------
    for url in &config_skills.urls {
        if let Some(git_skills) = fetch_git_skills(url) {
            add(git_skills);
        }
    }

    // Emit warnings for any duplicate skill names encountered.
    for w in &warn_duplicates {
        tracing::warn!("{}", w);
    }

    all
}

// ---------------------------------------------------------------------------
// Git URL support
// ---------------------------------------------------------------------------

/// Clone or reuse a cached git repo and return skills found in it.
///
/// Cache location: `<system-cache>/claurst/skills/<repo-name>/`
/// On first access the repo is cloned with `--depth=1`.
/// Subsequent calls use the already-cloned cache directory as-is.
fn fetch_git_skills(url: &str) -> Option<Vec<DiscoveredSkill>> {
    let cache_dir = dirs::cache_dir()?.join("claurst").join("skills");

    // Use the last path segment of the URL as the local directory name.
    let repo_name = url.split('/').last()?.trim_end_matches(".git");

    if repo_name.is_empty() {
        tracing::warn!(url, "skill_discovery: cannot derive repo name from git URL");
        return None;
    }

    let repo_dir = cache_dir.join(repo_name);

    if !repo_dir.exists() {
        tracing::info!(url, dest = %repo_dir.display(), "skill_discovery: cloning skills repo");

        // Ensure the parent cache directory exists.
        if let Err(err) = std::fs::create_dir_all(&cache_dir) {
            tracing::warn!(
                dir = %cache_dir.display(),
                error = %err,
                "skill_discovery: could not create cache dir"
            );
            return None;
        }

        let repo_dir_str = repo_dir.to_str()?;
        let status = std::process::Command::new("git")
            .args(["clone", "--depth=1", url, repo_dir_str])
            .status();

        match status {
            Ok(s) if s.success() => {
                tracing::info!(url, "skill_discovery: clone succeeded");
            }
            Ok(s) => {
                tracing::warn!(url, exit_code = ?s.code(), "skill_discovery: git clone failed");
                return None;
            }
            Err(err) => {
                tracing::warn!(url, error = %err, "skill_discovery: could not spawn git");
                return None;
            }
        }
    }

    // Scan repo root and optional `skills/` subdirectory.
    let mut skills = scan_dir(&repo_dir);
    skills.extend(scan_dir(&repo_dir.join("skills")));
    Some(skills)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;

    fn make_temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn write_file(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    // ---- parse_skill_file ---------------------------------------------------

    #[test]
    fn test_parse_with_frontmatter() {
        let content =
            "---\nname: review\ndescription: Review code changes\n---\n\nPlease review $ARGUMENTS";
        let path = PathBuf::from("review.md");
        let skill = parse_skill_file(content, &path).unwrap();
        assert_eq!(skill.name, "review");
        assert_eq!(skill.description, "Review code changes");
        assert!(skill.template.contains("$ARGUMENTS"));
    }

    #[test]
    fn test_parse_no_frontmatter_uses_stem() {
        let content = "Do something useful.";
        let path = PathBuf::from("my-skill.md");
        let skill = parse_skill_file(content, &path).unwrap();
        assert_eq!(skill.name, "my-skill");
        assert_eq!(skill.description, "Custom skill");
        assert_eq!(skill.template, "Do something useful.");
    }

    #[test]
    fn test_parse_missing_name_uses_stem() {
        let content = "---\ndescription: No name field\n---\n\nBody text.";
        let path = PathBuf::from("fallback.md");
        let skill = parse_skill_file(content, &path).unwrap();
        assert_eq!(skill.name, "fallback");
        assert_eq!(skill.description, "No name field");
    }

    #[test]
    fn test_parse_empty_returns_none() {
        let skill = parse_skill_file("   ", &PathBuf::from("empty.md"));
        assert!(skill.is_none());
    }

    #[test]
    fn test_parse_quoted_frontmatter_values() {
        let content = "---\nname: \"quoted name\"\ndescription: 'single quoted'\n---\nBody.";
        let skill = parse_skill_file(content, &PathBuf::from("x.md")).unwrap();
        assert_eq!(skill.name, "quoted name");
        assert_eq!(skill.description, "single quoted");
    }

    // ---- scan_dir -----------------------------------------------------------

    #[test]
    fn test_scan_dir_finds_skills() {
        let tmp = make_temp_dir();
        write_file(
            tmp.path(),
            "review.md",
            "---\nname: review\n---\nReview $ARGUMENTS",
        );
        write_file(tmp.path(), "debug.md", "Debug help.");
        write_file(tmp.path(), "not-md.txt", "ignored");

        let skills = scan_dir(tmp.path());
        assert_eq!(skills.len(), 2);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"review"));
        assert!(names.contains(&"debug"));
    }

    #[test]
    fn test_scan_dir_nonexistent_returns_empty() {
        let skills = scan_dir(Path::new("/nonexistent/path/xyz"));
        assert!(skills.is_empty());
    }

    // ---- discover_skills ----------------------------------------------------

    #[test]
    fn test_discover_from_project_dir() {
        let tmp = make_temp_dir();
        let skills_dir = tmp.path().join(".claurst").join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        write_file(
            &skills_dir,
            "myskill.md",
            "---\nname: myskill\ndescription: Test\n---\nDo it.",
        );

        let config = crate::config::SkillsConfig::default();
        let discovered = discover_skills(tmp.path(), &config);
        assert!(discovered.contains_key("myskill"));
        assert_eq!(discovered["myskill"].description, "Test");
    }

    #[test]
    fn test_discover_extra_paths() {
        let tmp = make_temp_dir();
        let extra = make_temp_dir();
        write_file(
            extra.path(),
            "extra.md",
            "---\nname: extra\n---\nExtra skill.",
        );

        let config = crate::config::SkillsConfig {
            paths: vec![extra.path().to_str().unwrap().to_string()],
            urls: vec![],
        };
        let discovered = discover_skills(tmp.path(), &config);
        assert!(discovered.contains_key("extra"));
    }

    #[test]
    fn test_discover_deduplicates_first_wins() {
        let tmp = make_temp_dir();
        let proj_skills = tmp.path().join(".claurst").join("skills");
        std::fs::create_dir_all(&proj_skills).unwrap();
        write_file(
            &proj_skills,
            "dup.md",
            "---\nname: dup\ndescription: project\n---\nProject.",
        );

        let extra = make_temp_dir();
        write_file(
            extra.path(),
            "dup.md",
            "---\nname: dup\ndescription: extra\n---\nExtra.",
        );

        let config = crate::config::SkillsConfig {
            paths: vec![extra.path().to_str().unwrap().to_string()],
            urls: vec![],
        };
        let discovered = discover_skills(tmp.path(), &config);
        // Project-level wins over extra path.
        assert_eq!(discovered["dup"].description, "project");
    }
}
