//! Attachment pipeline — mirrors src/utils/attachments.ts
//!
//! Assembles all context attachments for a conversation turn:
//! IDE context, tasks, plans, skills, agents, MCP, file changes, memory.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// The kind of attachment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    HookSuccess,
    HookError,
    HookNonBlockingError,
    HookErrorDuringExecution,
    HookStoppedContinuation,
    SkillListing,
    AgentListing,
    McpInstructions,
    IdeContext,
    TaskContext,
    PlanContext,
    ChangedFiles,
    Memory,
    Generic,
}

/// A single context attachment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub content: String,
    /// Optional label for display (e.g., filename, server name).
    pub label: Option<String>,
}

impl Attachment {
    pub fn new(kind: AttachmentKind, content: impl Into<String>) -> Self {
        Self {
            kind,
            content: content.into(),
            label: None,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// Context passed to `get_attachments`.
pub struct AttachmentContext<'a> {
    pub project_root: &'a Path,
    pub working_dir: &'a Path,
    pub session_id: &'a str,
    pub last_turn_timestamp_ms: Option<u64>,
}

/// Assemble all context attachments for the current turn.
///
/// Returns a vec of attachments to inject as a pre-turn context message.
pub fn get_attachments(ctx: &AttachmentContext<'_>) -> Vec<Attachment> {
    let mut attachments = Vec::new();

    // 1. IDE context
    if let Some(ide) = get_ide_context() {
        attachments.push(Attachment::new(AttachmentKind::IdeContext, ide));
    }

    // 2. Changed files (since last turn)
    if let Some(ts) = ctx.last_turn_timestamp_ms {
        let changed = get_changed_files(ctx.project_root, ts);
        if !changed.is_empty() {
            let content = format!(
                "Files changed since last turn:\n{}",
                changed
                    .iter()
                    .map(|f| format!("  {}", f))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            attachments.push(Attachment::new(AttachmentKind::ChangedFiles, content));
        }
    }

    attachments
}

/// Get IDE context from the lockfile (if an IDE is connected).
///
/// Returns a formatted string like:
/// `IDE: VS Code, workspace: /path/to/project, selection: L10-L20 in foo.rs`
pub fn get_ide_context() -> Option<String> {
    let lockfile_dir = dirs::home_dir()?.join(".claurst").join("ide");
    let entries = std::fs::read_dir(&lockfile_dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "lock") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(info) = serde_json::from_str::<serde_json::Value>(&content) {
                    let pid = info["pid"].as_u64().unwrap_or(0);
                    if !is_pid_alive(pid) {
                        continue;
                    }
                    let ide_name = info["ideName"].as_str().unwrap_or("IDE");
                    let workspace = info["workspaceFolders"]
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let mut parts = vec![format!("IDE: {}", ide_name)];
                    if !workspace.is_empty() {
                        parts.push(format!("workspace: {}", workspace));
                    }
                    // Active file/selection if present
                    if let Some(file) = info["activeFile"].as_str() {
                        parts.push(format!("active file: {}", file));
                        if let (Some(start), Some(end)) = (
                            info["selectionStart"].as_u64(),
                            info["selectionEnd"].as_u64(),
                        ) {
                            if start != end {
                                parts.push(format!("selection: L{}-L{}", start, end));
                            }
                        }
                    }
                    return Some(parts.join(", "));
                }
            }
        }
    }
    None
}

/// Check if a PID corresponds to a running process.
fn is_pid_alive(pid: u64) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::path::Path::new(&format!("/proc/{}", pid)).exists()
    }
}

/// Get files changed since `since_ms` (Unix timestamp in ms) using git.
pub fn get_changed_files(project_root: &Path, since_ms: u64) -> Vec<String> {
    // Try git diff --name-only --diff-filter=M
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=AMDR", "HEAD"])
        .current_dir(project_root)
        .output();

    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect(),
        _ => {
            // Fallback: scan for files modified since timestamp using mtime
            let since_secs = since_ms / 1000;
            let mut files = Vec::new();
            scan_modified_files(project_root, since_secs, &mut files, 0);
            files
        }
    }
}

fn scan_modified_files(dir: &Path, since_secs: u64, out: &mut Vec<String>, depth: usize) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Skip hidden dirs and node_modules / target
        if name_str.starts_with('.') || name_str == "node_modules" || name_str == "target" {
            continue;
        }
        if path.is_dir() {
            scan_modified_files(&path, since_secs, out, depth + 1);
        } else if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                let mtime = modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if mtime >= since_secs {
                    out.push(path.to_string_lossy().to_string());
                }
            }
        }
    }
}

/// Build a hook result attachment message.
pub fn make_hook_result_attachment(hook_name: &str, output: &str, success: bool) -> Attachment {
    let kind = if success {
        AttachmentKind::HookSuccess
    } else {
        AttachmentKind::HookError
    };
    Attachment::new(kind, format!("[Hook: {}]\n{}", hook_name, output))
        .with_label(hook_name.to_string())
}

/// Compute the diff of available tools between two turns.
pub fn get_deferred_tools_delta(prev_tools: &[String], curr_tools: &[String]) -> Vec<String> {
    curr_tools
        .iter()
        .filter(|t| !prev_tools.contains(t))
        .cloned()
        .collect()
}
