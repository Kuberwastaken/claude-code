// Worktree tools: create and exit git worktrees for isolated work sessions.
//
// EnterWorktreeTool – create a new git worktree with an optional branch name,
//                     switching the session's working directory to it.
// ExitWorktreeTool  – exit the current worktree, optionally removing it, and
//                     restore the original working directory.
//
// These tools mirror the TypeScript EnterWorktreeTool / ExitWorktreeTool.

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

// ---------------------------------------------------------------------------
// Session-level state: only one active worktree per session.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WorktreeSession {
    pub original_cwd: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: Option<String>,
    pub original_head: Option<String>,
}

static WORKTREE_SESSION: Lazy<Arc<RwLock<Option<WorktreeSession>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

// ---------------------------------------------------------------------------
// EnterWorktreeTool
// ---------------------------------------------------------------------------

pub struct EnterWorktreeTool;

#[derive(Debug, Deserialize)]
struct EnterWorktreeInput {
    /// Optional branch name. If omitted, a timestamped branch is created.
    #[serde(default)]
    branch: Option<String>,
    /// Sub-path under the repo root where the worktree will be created.
    /// Defaults to `.worktrees/<branch>`.
    #[serde(default)]
    path: Option<String>,
    /// Optional shell command to run inside the new worktree directory after creation.
    /// Example: "npm install" or "cargo build".
    #[serde(default)]
    post_create_command: Option<String>,
}

#[async_trait]
impl Tool for EnterWorktreeTool {
    fn name(&self) -> &str {
        "EnterWorktree"
    }

    fn description(&self) -> &str {
        "Create a new git worktree and switch the session's working directory to it. \
         This gives you an isolated environment to experiment or work on a feature \
         without affecting the main working tree. \
         Use ExitWorktree to return to the original directory."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "branch": {
                    "type": "string",
                    "description": "Branch name to create. Defaults to a timestamped name like claurst-20240101-120000."
                },
                "path": {
                    "type": "string",
                    "description": "Optional path for the worktree directory. Defaults to .worktrees/<branch>."
                },
                "post_create_command": {
                    "type": "string",
                    "description": "Optional command to run inside the new worktree after creation (e.g. 'npm install')."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let params: EnterWorktreeInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        // Check if already in a worktree session
        {
            let session = WORKTREE_SESSION.read().await;
            if session.is_some() {
                return ToolResult::error(
                    "Already in a worktree session. Call ExitWorktree first.".to_string(),
                );
            }
        }

        if let Err(e) = ctx.check_permission(self.name(), "Create a git worktree", false) {
            return ToolResult::error(e.to_string());
        }

        // Determine branch name — use a human-readable timestamp if none supplied
        let branch = params.branch.clone().unwrap_or_else(|| {
            // Format: claurst-YYYYMMDD-HHMMSS
            use std::time::{SystemTime, UNIX_EPOCH};
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            // Manual UTC decomposition (no chrono dep in this crate)
            let s = secs % 60;
            let m = (secs / 60) % 60;
            let h = (secs / 3600) % 24;
            let days = secs / 86400;
            // Approximate Gregorian calendar for branch name purposes
            let year = 1970 + days / 365;
            let day_of_year = days % 365;
            let month = day_of_year / 30 + 1;
            let day = day_of_year % 30 + 1;
            format!(
                "claurst-{:04}{:02}{:02}-{:02}{:02}{:02}",
                year, month, day, h, m, s
            )
        });

        // Determine worktree path
        let worktree_path = if let Some(p) = params.path {
            ctx.working_dir.join(p)
        } else {
            ctx.working_dir.join(".worktrees").join(&branch)
        };

        // Verify we are inside a git repository before attempting worktree creation
        let head_result = run_git(&ctx.working_dir, &["rev-parse", "HEAD"]).await;
        let original_head = match &head_result {
            Ok(h) => Some(h.trim().to_string()),
            Err(e) => {
                let msg = e.to_lowercase();
                if msg.contains("not a git repository") || msg.contains("fatal") {
                    return ToolResult::error(format!(
                        "Cannot create worktree: the current directory '{}' is not inside a git repository.",
                        ctx.working_dir.display()
                    ));
                }
                None
            }
        };

        // Check if the target path already exists
        if worktree_path.exists() {
            return ToolResult::error(format!(
                "Cannot create worktree: the path '{}' already exists.                  Provide a different 'path' argument or remove the existing directory.",
                worktree_path.display()
            ));
        }

        // Create the worktree
        let worktree_str = worktree_path.to_string_lossy().to_string();
        let result = run_git(
            &ctx.working_dir,
            &["worktree", "add", "-b", &branch, &worktree_str],
        )
        .await;

        match result {
            Err(e) => {
                let msg = e.trim().to_string();
                let friendly = if msg.to_lowercase().contains("already exists") {
                    format!(
                        "Failed to create worktree: branch '{}' already exists.                          Use a different branch name or delete the existing branch first.",
                        branch
                    )
                } else if msg.to_lowercase().contains("not a git repository") {
                    format!(
                        "Failed to create worktree: '{}' is not inside a git repository.",
                        ctx.working_dir.display()
                    )
                } else {
                    format!("Failed to create worktree: {}", msg)
                };
                ToolResult::error(friendly)
            }
            Ok(_) => {
                debug!(
                    branch = %branch,
                    path = %worktree_path.display(),
                    "Created worktree"
                );

                // Save session state
                *WORKTREE_SESSION.write().await = Some(WorktreeSession {
                    original_cwd: ctx.working_dir.clone(),
                    worktree_path: worktree_path.clone(),
                    branch: Some(branch.clone()),
                    original_head,
                });

                // Run optional post-create command in the new worktree directory
                let post_create_output = if let Some(cmd) = params.post_create_command {
                    let shell_result = if cfg!(target_os = "windows") {
                        tokio::process::Command::new("cmd")
                            .args(["/C", &cmd])
                            .current_dir(&worktree_path)
                            .output()
                            .await
                    } else {
                        tokio::process::Command::new("sh")
                            .args(["-c", &cmd])
                            .current_dir(&worktree_path)
                            .output()
                            .await
                    };
                    match shell_result {
                        Ok(out) if out.status.success() => {
                            let stdout = String::from_utf8_lossy(&out.stdout);
                            format!(
                                "\nPost-create command '{}' completed successfully.{}",
                                cmd,
                                if stdout.trim().is_empty() {
                                    String::new()
                                } else {
                                    format!("\nOutput: {}", stdout.trim())
                                }
                            )
                        }
                        Ok(out) => {
                            let stderr = String::from_utf8_lossy(&out.stderr);
                            format!(
                                "\nPost-create command '{}' exited with error.\nStderr: {}",
                                cmd,
                                stderr.trim()
                            )
                        }
                        Err(e) => format!("\nCould not run post-create command '{}': {}", cmd, e),
                    }
                } else {
                    String::new()
                };

                ToolResult::success(format!(
                    "Created worktree at {} on branch '{}'.\n\
                     The working directory is now {}.\n\
                     Use ExitWorktree to return to {}.{}",
                    worktree_path.display(),
                    branch,
                    worktree_path.display(),
                    ctx.working_dir.display(),
                    post_create_output,
                ))
                .with_metadata(json!({
                    "worktree_path": worktree_path.to_string_lossy(),
                    "branch": branch,
                    "original_cwd": ctx.working_dir.to_string_lossy(),
                }))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ExitWorktreeTool
// ---------------------------------------------------------------------------

pub struct ExitWorktreeTool;

#[derive(Debug, Deserialize)]
struct ExitWorktreeInput {
    /// "keep" = leave the worktree on disk; "remove" = delete it.
    #[serde(default = "default_action")]
    action: String,
    /// Required if action=="remove" and there are uncommitted changes.
    #[serde(default)]
    discard_changes: bool,
}

fn default_action() -> String {
    "keep".to_string()
}

#[async_trait]
impl Tool for ExitWorktreeTool {
    fn name(&self) -> &str {
        "ExitWorktree"
    }

    fn description(&self) -> &str {
        "Exit the current worktree session created by EnterWorktree and restore the \
         original working directory. Use action='keep' to preserve the worktree on \
         disk, or action='remove' to delete it. Only operates on worktrees created \
         by EnterWorktree in this session."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["keep", "remove"],
                    "description": "\"keep\" leaves the worktree on disk; \"remove\" deletes it and its branch."
                },
                "discard_changes": {
                    "type": "boolean",
                    "description": "Set true when action=remove and the worktree has uncommitted/unmerged work to discard."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let params: ExitWorktreeInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        let session_guard = WORKTREE_SESSION.read().await;
        let session = match &*session_guard {
            Some(s) => s.clone(),
            None => {
                return ToolResult::error(
                    "No-op: there is no active EnterWorktree session to exit. \
                     This tool only operates on worktrees created by EnterWorktree \
                     in the current session."
                        .to_string(),
                );
            }
        };
        drop(session_guard);

        let worktree_str = session.worktree_path.to_string_lossy().to_string();

        // If action is "remove", check for uncommitted changes
        if params.action == "remove" && !params.discard_changes {
            let status = run_git(&session.worktree_path, &["status", "--porcelain"]).await;
            let changed_files = status
                .as_deref()
                .unwrap_or("")
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count();

            let commit_count = if let Some(ref head) = session.original_head {
                let rev = run_git(
                    &session.worktree_path,
                    &["rev-list", "--count", &format!("{}..HEAD", head)],
                )
                .await
                .unwrap_or_default();
                rev.trim().parse::<usize>().unwrap_or(0)
            } else {
                0
            };

            if changed_files > 0 || commit_count > 0 {
                let mut parts = Vec::new();
                if changed_files > 0 {
                    parts.push(format!("{} uncommitted file(s)", changed_files));
                }
                if commit_count > 0 {
                    parts.push(format!("{} commit(s) on the worktree branch", commit_count));
                }
                return ToolResult::error(format!(
                    "Worktree has {}. Removing will discard this work permanently. \
                     Confirm with the user, then re-invoke with discard_changes=true — \
                     or use action=\"keep\" to preserve the worktree.",
                    parts.join(" and ")
                ));
            }
        }

        // Clear session state
        *WORKTREE_SESSION.write().await = None;

        match params.action.as_str() {
            "keep" => {
                // Just remove the worktree from git's tracking list (prune),
                // but keep the directory on disk.
                let _ = run_git(
                    &session.original_cwd,
                    &[
                        "worktree",
                        "lock",
                        "--reason",
                        "kept by ExitWorktree",
                        &worktree_str,
                    ],
                )
                .await;

                ToolResult::success(format!(
                    "Exited worktree. Work preserved at {} on branch {}. \
                     Session is now back in {}.",
                    session.worktree_path.display(),
                    session.branch.as_deref().unwrap_or("(unknown)"),
                    session.original_cwd.display(),
                ))
            }
            "remove" => {
                // Remove the worktree
                let _ = run_git(
                    &session.original_cwd,
                    &["worktree", "remove", "--force", &worktree_str],
                )
                .await;

                // Delete the branch if we created it
                if let Some(ref branch) = session.branch {
                    let _ = run_git(&session.original_cwd, &["branch", "-D", branch]).await;
                }

                ToolResult::success(format!(
                    "Exited and removed worktree at {}. \
                     Session is now back in {}.",
                    session.worktree_path.display(),
                    session.original_cwd.display(),
                ))
            }
            other => ToolResult::error(format!(
                "Unknown action '{}'. Use 'keep' or 'remove'.",
                other
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

async fn run_git(cwd: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}
