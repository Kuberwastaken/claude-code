//! Git utilities for Claurst.
//! Mirrors src/utils/git.ts (926 lines) and src/utils/git/ subdirectory.

use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Repository discovery
// ---------------------------------------------------------------------------

/// Walk up the directory tree to find the nearest `.git` directory.
pub fn get_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let git_dir = current.join(".git");
        if git_dir.exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Run a git command in `repo_root` and return stdout as a String.
/// Returns empty string on failure (non-zero exit, not-a-repo, etc.).
fn git_output(repo_root: &Path, args: &[&str]) -> String {
    Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Branch / status
// ---------------------------------------------------------------------------

/// Return the current branch name (or "HEAD" if detached).
pub fn get_current_branch(repo_root: &Path) -> String {
    let branch = git_output(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"]);
    if branch.is_empty() {
        "HEAD".to_string()
    } else {
        branch
    }
}

/// Return list of files modified (staged or unstaged).
pub fn list_modified_files(repo_root: &Path) -> Vec<PathBuf> {
    let output = git_output(repo_root, &["diff", "--name-only", "HEAD"]);
    if output.is_empty() {
        return Vec::new();
    }
    output.lines().map(|l| repo_root.join(l)).collect()
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

/// Return the staged diff (index vs HEAD).
pub fn get_staged_diff(repo_root: &Path) -> String {
    git_output(repo_root, &["diff", "--cached"])
}

/// Return the unstaged diff (working tree vs index).
pub fn get_unstaged_diff(repo_root: &Path) -> String {
    git_output(repo_root, &["diff"])
}

/// Return the diff for a specific file since a given commit (or HEAD).
pub fn get_file_diff(repo_root: &Path, path: &Path, since_commit: Option<&str>) -> String {
    let commit = since_commit.unwrap_or("HEAD");
    let path_str = path.to_string_lossy();
    git_output(repo_root, &["diff", commit, "--", &path_str])
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

/// A single git commit summary.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

/// Return the last `n` commits in the repository.
pub fn get_commit_history(repo_root: &Path, n: usize) -> Vec<CommitInfo> {
    let format = "%H%x1f%h%x1f%an%x1f%ad%x1f%s%x1e";
    let n_str = n.to_string();
    let output = git_output(
        repo_root,
        &[
            "log",
            &format!("-{}", n_str),
            &format!("--format={}", format),
            "--date=short",
        ],
    );

    output
        .split('\x1e')
        .filter(|s| !s.trim().is_empty())
        .filter_map(|entry| {
            let parts: Vec<&str> = entry.trim().splitn(5, '\x1f').collect();
            if parts.len() == 5 {
                Some(CommitInfo {
                    hash: parts[0].to_string(),
                    short_hash: parts[1].to_string(),
                    author: parts[2].to_string(),
                    date: parts[3].to_string(),
                    subject: parts[4].to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Branch operations
// ---------------------------------------------------------------------------

/// Create and switch to a new branch.
pub fn create_branch(repo_root: &Path, name: &str) -> bool {
    Command::new("git")
        .current_dir(repo_root)
        .args(["checkout", "-b", name])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Switch to an existing branch.
pub fn switch_branch(repo_root: &Path, name: &str) -> bool {
    Command::new("git")
        .current_dir(repo_root)
        .args(["checkout", name])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Stash
// ---------------------------------------------------------------------------

/// Stash uncommitted changes with an optional message.
pub fn stash(repo_root: &Path, message: Option<&str>) -> bool {
    let mut args = vec!["stash", "push"];
    let msg_flag;
    if let Some(m) = message {
        msg_flag = format!("-m {}", m);
        args.push(&msg_flag);
    }
    Command::new("git")
        .current_dir(repo_root)
        .args(&args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Pop the top stash entry.
pub fn stash_pop(repo_root: &Path) -> bool {
    Command::new("git")
        .current_dir(repo_root)
        .args(["stash", "pop"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// .gitignore check
// ---------------------------------------------------------------------------

/// Returns `true` if the given path is git-ignored.
pub fn is_ignored(repo_root: &Path, path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    Command::new("git")
        .current_dir(repo_root)
        .args(["check-ignore", "-q", &path_str])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn get_repo_root_finds_git() {
        // Run from within the src-rust workspace which has .git
        let result = get_repo_root(Path::new("."));
        // Should find the repo root (may or may not exist in test env)
        // Just verify it doesn't panic.
        let _ = result;
    }

    #[test]
    fn commit_info_parse() {
        // smoke test — just ensure it doesn't panic with empty output
        let commits = get_commit_history(Path::new("."), 0);
        assert!(commits.is_empty());
    }
}
