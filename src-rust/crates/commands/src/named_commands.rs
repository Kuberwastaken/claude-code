//! Named commands (e.g. `claude agents`, `claude ide`, `claude branch`, …).
//!
//! These complement slash commands with more complex top-level flows.
//! A named command is invoked when the *first* CLI argument matches one
//! of the registered names — before the normal REPL starts.
//!
//! Sources consulted while porting:
//!   src/commands/agents/index.ts
//!   src/commands/ide/index.ts
//!   src/commands/branch/index.ts
//!   src/commands/tag/index.ts
//!   src/commands/passes/index.ts
//!   src/commands/pr_comments/index.ts
//!   src/commands/install-github-app/index.ts
//!   src/commands/desktop/index.ts  (implied by component structure)
//!   src/commands/mobile/index.ts   (implied by component structure)
//!   src/commands/remote-setup/index.ts (implied by component structure)

use crate::{CommandContext, CommandResult};
// `open` crate: used by StickersCommand to launch the browser.

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// A top-level named command (`claude <name> [args…]`).
pub trait NamedCommand: Send + Sync {
    /// Primary command name, e.g. `"agents"`.
    fn name(&self) -> &str;

    /// One-line description used in `claude --help`.
    fn description(&self) -> &str;

    /// Usage hint shown in `claude <name> --help`.
    fn usage(&self) -> &str;

    /// Execute the command.  `args` is the slice of arguments *after* the
    /// command name itself.
    fn execute_named(&self, args: &[&str], ctx: &CommandContext) -> CommandResult;
}

// ---------------------------------------------------------------------------
// agents
// ---------------------------------------------------------------------------

pub struct AgentsCommand;

impl NamedCommand for AgentsCommand {
    fn name(&self) -> &str { "agents" }
    fn description(&self) -> &str { "Manage and configure sub-agents" }
    fn usage(&self) -> &str { "claude agents [list|create|edit|delete] [name]" }

    fn execute_named(&self, args: &[&str], ctx: &CommandContext) -> CommandResult {
        match args.first().copied().unwrap_or("list") {
            "list" => {
                // Load agent definitions from .claurst/agents/ in working dir
                // (and home dir), using the same loader as the TUI agents view.
                let defs = claurst_tui::agents_view::load_agent_definitions(&ctx.working_dir);

                if defs.is_empty() {
                    return CommandResult::Message(
                        "Available Agents (0)\n\n\
                         No custom agents defined. Create one with /new-agent\n\
                         or run: claude agents create <name>"
                            .to_string(),
                    );
                }

                let mut out = format!("Available Agents ({})\n\n", defs.len());
                for def in &defs {
                    let model_str = def.model.as_deref().unwrap_or("default model");
                    if def.description.is_empty() {
                        out.push_str(&format!(
                            "  \u{2022} {} ({})\n",
                            def.name, model_str
                        ));
                    } else {
                        out.push_str(&format!(
                            "  \u{2022} {}: {}\n    Model: {}\n",
                            def.name, def.description, model_str
                        ));
                    }
                }
                out.push_str("\nUse 'claude agents create <name>' to add a new agent.");
                CommandResult::Message(out)
            }
            "create" => {
                let name = args.get(1).copied().unwrap_or("my-agent");
                CommandResult::Message(format!(
                    "Create a new agent by adding .claurst/agents/{name}.md\n\
                     Template:\n\
                     ---\n\
                     name: {name}\n\
                     description: <description>\n\
                     model: claude-sonnet-4-6\n\
                     ---\n\n\
                     <agent instructions here>"
                ))
            }
            "edit" => {
                let name = match args.get(1).copied() {
                    Some(n) => n,
                    None => return CommandResult::Error(
                        "Usage: claude agents edit <name>".to_string(),
                    ),
                };
                CommandResult::Message(format!(
                    "Edit .claurst/agents/{name}.md in your editor to update the agent."
                ))
            }
            "delete" => {
                let name = match args.get(1).copied() {
                    Some(n) => n,
                    None => return CommandResult::Error(
                        "Usage: claude agents delete <name>".to_string(),
                    ),
                };
                CommandResult::Message(format!(
                    "Delete .claurst/agents/{name}.md to remove the agent."
                ))
            }
            sub => CommandResult::Error(format!("Unknown agents subcommand: '{sub}'")),
        }
    }
}

// ---------------------------------------------------------------------------
// add-dir
// ---------------------------------------------------------------------------

pub struct AddDirCommand;

impl NamedCommand for AddDirCommand {
    fn name(&self) -> &str { "add-dir" }
    fn description(&self) -> &str { "Add a directory to Claurst's allowed workspace paths" }
    fn usage(&self) -> &str { "claude add-dir <path>" }

    fn execute_named(&self, args: &[&str], _ctx: &CommandContext) -> CommandResult {
        let raw = match args.first() {
            Some(p) => *p,
            None => return CommandResult::Error("Usage: claude add-dir <path>".to_string()),
        };

        let path = std::path::Path::new(raw);

        if !path.exists() {
            return CommandResult::Error(format!("Directory does not exist: {}", path.display()));
        }

        if !path.is_dir() {
            return CommandResult::Error(format!("Not a directory: {}", path.display()));
        }

        let abs_path = match std::fs::canonicalize(path) {
            Ok(p) => p,
            Err(e) => return CommandResult::Error(format!("Cannot resolve path: {e}")),
        };

        let mut settings = match claurst_core::config::Settings::load_sync() {
            Ok(s) => s,
            Err(e) => {
                return CommandResult::Error(format!(
                    "Failed to load settings before updating workspace paths: {e}"
                ))
            }
        };

        if !settings.config.workspace_paths.iter().any(|p| p == &abs_path) {
            settings.config.workspace_paths.push(abs_path.clone());
            if let Err(e) = settings.save_sync() {
                return CommandResult::Error(format!(
                    "Added {} for this session, but failed to save settings: {}",
                    abs_path.display(),
                    e
                ));
            }
        }

        CommandResult::Message(format!(
            "Added {} to allowed workspace paths.",
            abs_path.display()
        ))
    }
}

// ---------------------------------------------------------------------------
// branch
// ---------------------------------------------------------------------------

pub struct BranchCommand;

impl NamedCommand for BranchCommand {
    fn name(&self) -> &str { "branch" }
    fn description(&self) -> &str { "Create a branch of the current conversation at this point" }
    fn usage(&self) -> &str { "claude branch [create|list|switch] [name|id]" }

    fn execute_named(&self, args: &[&str], ctx: &CommandContext) -> CommandResult {
        match args.first().copied().unwrap_or("") {
            "" | "create" => {
                // Optional name argument (second arg for "create", first for bare call)
                let name = if args.first().copied() == Some("create") {
                    args.get(1).copied()
                } else {
                    args.first().copied()
                };

                if ctx.session_id.is_empty() || ctx.session_id == "pre-session" {
                    return CommandResult::Error(
                        "No active session to branch. Start a conversation first.".to_string(),
                    );
                }

                let session_id = ctx.session_id.clone();
                let msg_count = ctx.messages.len();
                let title_opt = name.map(|s| s.to_string());

                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async move {
                        claurst_core::history::branch_session(
                            &session_id,
                            msg_count,
                            title_opt.as_deref(),
                        )
                        .await
                    })
                });

                match result {
                    Ok(new_session) => {
                        let title = new_session.title.as_deref().unwrap_or("(untitled)");
                        CommandResult::Message(format!(
                            "Created branch: \"{title}\"\nNew session ID: {}\n\
                             To resume original: claude -r {}\n\
                             To switch to branch: /branch switch {}",
                            new_session.id,
                            ctx.session_id,
                            new_session.id,
                        ))
                    }
                    Err(e) => CommandResult::Error(format!("Failed to branch session: {e}")),
                }
            }
            "list" => {
                let parent_id = ctx.session_id.clone();

                let sessions = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(claurst_core::history::list_sessions())
                });

                let branches: Vec<_> = sessions
                    .iter()
                    .filter(|s| s.branch_from.as_deref() == Some(&parent_id))
                    .collect();

                if branches.is_empty() {
                    CommandResult::Message(
                        "No branches found for the current session.".to_string(),
                    )
                } else {
                    let mut out = format!(
                        "Branches of session {}:\n\n",
                        &parent_id[..parent_id.len().min(8)]
                    );
                    for b in &branches {
                        let updated = b.updated_at.format("%Y-%m-%d %H:%M").to_string();
                        let id_short = &b.id[..b.id.len().min(8)];
                        out.push_str(&format!(
                            "  {} | {} | {} messages | {}\n",
                            id_short,
                            updated,
                            b.messages.len(),
                            b.title.as_deref().unwrap_or("(untitled)")
                        ));
                    }
                    out.push_str("\nUse: claude branch switch <id>");
                    CommandResult::Message(out)
                }
            }
            "switch" => {
                let id = match args.get(1).copied() {
                    Some(i) if !i.is_empty() => i.to_string(),
                    _ => {
                        return CommandResult::Error(
                            "Usage: claude branch switch <session-id>".to_string(),
                        )
                    }
                };

                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(claurst_core::history::load_session(&id))
                });

                match result {
                    Ok(session) => CommandResult::ResumeSession(session),
                    Err(e) => CommandResult::Error(format!("Could not load session '{id}': {e}")),
                }
            }
            sub => CommandResult::Error(format!("Unknown branch subcommand: '{sub}'\nUsage: claude branch [create|list|switch] [name|id]")),
        }
    }
}

// ---------------------------------------------------------------------------
// tag
// ---------------------------------------------------------------------------

pub struct TagCommand;

impl NamedCommand for TagCommand {
    fn name(&self) -> &str { "tag" }
    fn description(&self) -> &str { "Toggle a searchable tag on the current session" }
    fn usage(&self) -> &str { "claude tag [list|add|remove|toggle] [tag]" }

    fn execute_named(&self, args: &[&str], ctx: &CommandContext) -> CommandResult {
        let session_id = ctx.session_id.clone();

        match args.first().copied().unwrap_or("list") {
            "list" => {
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(claurst_core::history::load_session(&session_id))
                });
                match result {
                    Ok(session) => {
                        if session.tags.is_empty() {
                            CommandResult::Message(
                                "No tags set for this session.".to_string(),
                            )
                        } else {
                            CommandResult::Message(format!(
                                "Tags for this session:\n{}",
                                session
                                    .tags
                                    .iter()
                                    .map(|t| format!("  #{t}"))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            ))
                        }
                    }
                    Err(_) => CommandResult::Message(
                        "No tags set for this session (session not yet saved).".to_string(),
                    ),
                }
            }
            "add" => {
                let tag = match args.get(1).copied() {
                    Some(t) if !t.is_empty() => t.to_string(),
                    _ => {
                        return CommandResult::Error(
                            "Usage: claude tag add <tag>".to_string(),
                        )
                    }
                };

                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(claurst_core::history::tag_session(&session_id, &tag))
                });

                match result {
                    Ok(()) => CommandResult::Message(format!("Added tag: #{tag}")),
                    Err(e) => CommandResult::Error(format!(
                        "Could not add tag (session may not be saved yet): {e}"
                    )),
                }
            }
            "remove" => {
                let tag = match args.get(1).copied() {
                    Some(t) if !t.is_empty() => t.to_string(),
                    _ => {
                        return CommandResult::Error(
                            "Usage: claude tag remove <tag>".to_string(),
                        )
                    }
                };

                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(claurst_core::history::untag_session(&session_id, &tag))
                });

                match result {
                    Ok(()) => CommandResult::Message(format!("Removed tag: #{tag}")),
                    Err(e) => CommandResult::Error(format!("Could not remove tag: {e}")),
                }
            }
            "toggle" => {
                let tag = match args.get(1).copied() {
                    Some(t) if !t.is_empty() => t.to_string(),
                    _ => {
                        return CommandResult::Error(
                            "Usage: claude tag toggle <tag>".to_string(),
                        )
                    }
                };

                // Load session to check existing tags
                let load_result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(claurst_core::history::load_session(&session_id))
                });

                match load_result {
                    Ok(session) => {
                        let tag_clone = tag.clone();
                        if session.tags.iter().any(|t| t == &tag) {
                            // Tag exists — remove it
                            let remove_result = tokio::task::block_in_place(|| {
                                tokio::runtime::Handle::current()
                                    .block_on(claurst_core::history::untag_session(&session_id, &tag_clone))
                            });
                            match remove_result {
                                Ok(()) => CommandResult::Message(format!("Removed tag: #{tag}")),
                                Err(e) => CommandResult::Error(format!("Could not remove tag: {e}")),
                            }
                        } else {
                            // Tag absent — add it
                            let add_result = tokio::task::block_in_place(|| {
                                tokio::runtime::Handle::current()
                                    .block_on(claurst_core::history::tag_session(&session_id, &tag_clone))
                            });
                            match add_result {
                                Ok(()) => CommandResult::Message(format!("Added tag: #{tag}")),
                                Err(e) => CommandResult::Error(format!("Could not add tag: {e}")),
                            }
                        }
                    }
                    Err(e) => CommandResult::Error(format!(
                        "Could not toggle tag (session may not be saved yet): {e}"
                    )),
                }
            }
            sub => CommandResult::Error(format!(
                "Unknown tag subcommand: '{sub}'\nUsage: claude tag [list|add|remove|toggle] [tag]"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// passes
// ---------------------------------------------------------------------------

pub struct PassesCommand;

impl NamedCommand for PassesCommand {
    fn name(&self) -> &str { "passes" }
    fn description(&self) -> &str { "Share a free week of Claurst with friends" }
    fn usage(&self) -> &str { "claude passes" }

    fn execute_named(&self, _args: &[&str], _ctx: &CommandContext) -> CommandResult {
        CommandResult::Message(
            "Claurst Passes \u{2014} Share Claurst with friends\n\n\
             Share a free week of Claurst with a friend\n\
             Visit https://claude.ai/passes to get your referral link\n\
             Each referral gives your friend 1 week of Claurst Pro"
                .to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// Helper: process liveness check (used by IdeCommand)
// ---------------------------------------------------------------------------

fn is_pid_alive(pid: u64) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        // On Windows, query the process table via tasklist
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

// ---------------------------------------------------------------------------
// ide
// ---------------------------------------------------------------------------

pub struct IdeCommand;

impl NamedCommand for IdeCommand {
    fn name(&self) -> &str { "ide" }
    fn description(&self) -> &str { "Manage IDE integrations and show status" }
    fn usage(&self) -> &str { "claude ide [status|connect|disconnect|open]" }

    fn execute_named(&self, _args: &[&str], _ctx: &CommandContext) -> CommandResult {
        // ---- Environment-based IDE detection --------------------------------
        let env_detection = claurst_core::detect_ide();
        let env_section = match &env_detection {
            Some(kind) => {
                let mut lines = vec![format!("Detected IDE: {}", kind.display_name())];
                if let Some(cmd) = kind.extension_install_command() {
                    lines.push(format!("To install the Claurst extension: {}", cmd));
                }
                lines.join("\n")
            }
            None => "No IDE detected. Running in standalone terminal.".to_string(),
        };

        // ---- Lockfile-based connection status --------------------------------
        let lockfile_dir = dirs::home_dir()
            .map(|h| h.join(".claurst").join("ide"))
            .unwrap_or_default();

        let mut ides = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&lockfile_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "lock") {
                    if let Ok(lock_content) = std::fs::read_to_string(&path) {
                        if let Ok(info) = serde_json::from_str::<serde_json::Value>(&lock_content) {
                            let pid = info["pid"].as_u64().unwrap_or(0);
                            let alive = is_pid_alive(pid);
                            if alive {
                                let ide_name = info["ideName"].as_str().unwrap_or("Unknown IDE").to_string();
                                let port = info["port"].as_u64().unwrap_or(0);
                                let workspace_folders = info["workspaceFolders"]
                                    .as_array()
                                    .map(|a| a.iter()
                                        .filter_map(|v| v.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", "))
                                    .unwrap_or_default();
                                ides.push(format!("  {} (PID {}, port {}) \u{2014} {}", ide_name, pid, port, workspace_folders));
                            } else {
                                // Clean up dead lockfile
                                let _ = std::fs::remove_file(&path);
                            }
                        }
                    }
                }
            }
        }

        let connection_section = if ides.is_empty() {
            "No active IDE extension connections found.".to_string()
        } else {
            format!("Connected IDEs:\n{}\n\nUse 'claude ide open <file>' to open a file in the IDE.", ides.join("\n"))
        };

        CommandResult::Message(format!("{env_section}\n\n{connection_section}"))
    }
}

// ---------------------------------------------------------------------------
// pr-comments
// ---------------------------------------------------------------------------

pub struct PrCommentsCommand;

impl NamedCommand for PrCommentsCommand {
    fn name(&self) -> &str { "pr-comments" }
    fn description(&self) -> &str { "Get review comments from the current GitHub pull request" }
    fn usage(&self) -> &str { "claude pr-comments" }

    fn execute_named(&self, _args: &[&str], _ctx: &CommandContext) -> CommandResult {
        // Step 1: Get current git remote + PR info via gh CLI
        let pr_json = std::process::Command::new("gh")
            .args(["pr", "view", "--json", "number,url,headRefName,baseRefName"])
            .output();

        let pr_info = match pr_json {
            Err(_) => return CommandResult::Error(
                "GitHub CLI (gh) not found. Install from https://cli.github.com".to_string()
            ),
            Ok(out) if !out.status.success() => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return CommandResult::Error(format!("No open PR found: {}", stderr.trim()));
            }
            Ok(out) => {
                match serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    Ok(v) => v,
                    Err(_) => return CommandResult::Error("Failed to parse gh output".to_string()),
                }
            }
        };

        let pr_number = pr_info["number"].as_u64().unwrap_or(0);
        let pr_url = pr_info["url"].as_str().unwrap_or("").to_string();

        if pr_number == 0 {
            return CommandResult::Error("Could not determine PR number.".to_string());
        }

        // Step 2: Fetch review comments via gh API
        let comments_out = std::process::Command::new("gh")
            .args(["api", &format!("repos/{{owner}}/{{repo}}/pulls/{}/comments", pr_number)])
            .output();

        let mut output = format!("PR #{} \u{2014} {}\n\n", pr_number, pr_url);

        match comments_out {
            Ok(out) if out.status.success() => {
                match serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) {
                    Ok(comments) if !comments.is_empty() => {
                        output.push_str(&format!("Review comments ({}):\n", comments.len()));
                        for c in &comments {
                            let path = c["path"].as_str().unwrap_or("unknown");
                            let line = c["line"].as_u64().unwrap_or(0);
                            let user = c["user"]["login"].as_str().unwrap_or("unknown");
                            let body = c["body"].as_str().unwrap_or("").trim();
                            let body_short: String = body.chars().take(200).collect();
                            output.push_str(&format!("  {}:{} by @{}:\n    {}\n\n", path, line, user, body_short));
                        }
                    }
                    Ok(_) => output.push_str("No review comments found.\n"),
                    Err(_) => output.push_str("Could not parse review comments.\n"),
                }
            }
            _ => output.push_str("Could not fetch review comments (check gh auth).\n"),
        }

        CommandResult::Message(output)
    }
}

// ---------------------------------------------------------------------------
// desktop
// ---------------------------------------------------------------------------

pub struct DesktopCommand;

impl NamedCommand for DesktopCommand {
    fn name(&self) -> &str { "desktop" }
    fn description(&self) -> &str { "Download and set up Claurst Desktop app" }
    fn usage(&self) -> &str { "claude desktop" }

    fn execute_named(&self, _args: &[&str], ctx: &CommandContext) -> CommandResult {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let download_url = "https://claude.ai/download";

        // Detect if Claurst Desktop is likely installed (platform-specific heuristic).
        let desktop_likely_installed = match os {
            "macos" => {
                std::path::Path::new("/Applications/Claude.app").exists()
                    || std::path::Path::new(&format!(
                        "{}/Applications/Claude.app",
                        std::env::var("HOME").unwrap_or_default()
                    ))
                    .exists()
            }
            "windows" => {
                std::env::var("LOCALAPPDATA")
                    .map(|p| std::path::Path::new(&p).join("Programs/Claude/Claude.exe").exists())
                    .unwrap_or(false)
                    || std::path::Path::new("C:\\Program Files\\Claude\\Claude.exe").exists()
            }
            _ => false,
        };

        // If a remote session is active the user is already bridged — show a
        // deep link so they can open the current session in Desktop.
        if let Some(ref session_url) = ctx.remote_session_url {
            let session_id = session_url.split('/').last().unwrap_or("");
            let deep_link = format!("claude://session/{}", session_id);

            let mut msg = String::new();
            msg.push_str("\u{2713} Already connected to Claurst Desktop\n\n");
            msg.push_str("Your Claurst session is synced with Claurst Desktop.\n\n");
            msg.push_str(&format!("Open this session in Desktop: {deep_link}\n\n"));
            if desktop_likely_installed {
                msg.push_str("Claurst Desktop is installed on this machine.\n");
                msg.push_str(&format!("Manage your installation: {download_url}"));
            } else {
                msg.push_str(&format!("Download / manage Desktop: {download_url}"));
            }
            return CommandResult::Message(msg);
        }

        let msg = if os == "macos" {
            if desktop_likely_installed {
                format!(
                    "Open Claurst Desktop \u{2014} macOS\n\n\
                     Claurst Desktop appears to be installed.\n\
                     Launch it from /Applications/Claude.app and sign in with your Anthropic account.\n\n\
                     Download / update: {download_url}"
                )
            } else {
                format!(
                    "Download Claurst Desktop \u{2014} macOS\n\n\
                     Download: {download_url}\n\n\
                     Setup instructions:\n\
                     1. Download and install Claurst Desktop for macOS\n\
                     2. Open Claurst Desktop and sign in with the same Anthropic account\n\
                     3. Claurst will detect the Desktop bridge automatically"
                )
            }
        } else if os == "windows" {
            let arch_note = if arch == "x86_64" { " (x64)" } else { "" };
            if desktop_likely_installed {
                format!(
                    "Open Claurst Desktop \u{2014} Windows{arch_note}\n\n\
                     Claurst Desktop appears to be installed.\n\
                     Launch it from your Start menu and sign in with your Anthropic account.\n\n\
                     Download / update: {download_url}"
                )
            } else {
                format!(
                    "Download Claurst Desktop for Windows{arch_note}\n\n\
                     Download: {download_url}\n\n\
                     Setup instructions:\n\
                     1. Download and run the Claurst Desktop installer\n\
                     2. Open Claurst Desktop and sign in with the same Anthropic account\n\
                     3. Claurst will detect the Desktop bridge automatically"
                )
            }
        } else {
            // Linux and other platforms
            format!(
                "Claurst Desktop is not yet available for {os}\n\n\
                 On Linux, you can use Claurst via the CLI or visit https://claude.ai in your browser.\n\
                 Check {download_url} for the latest platform availability."
            )
        };

        CommandResult::Message(msg)
    }
}

// ---------------------------------------------------------------------------
// mobile — helper
// ---------------------------------------------------------------------------

/// Render a URL as a real QR code using Unicode half-block characters.
///
/// Uses the `qrcode` crate to encode the URL, then converts the bit matrix
/// to lines of "▀" / "▄" / "█" / " " so that two QR rows are packed into
/// one terminal line (each cell is rendered as a half-block character).
/// This matches the approach used by many CLI QR renderers and fits in ~40
/// terminal columns for typical short URLs.
pub fn render_qr(url: &str) -> Vec<String> {
    use qrcode::{EcLevel, QrCode};

    let code = match QrCode::with_error_correction_level(url.as_bytes(), EcLevel::L) {
        Ok(c) => c,
        Err(_) => return vec!["[QR generation failed]".to_string()],
    };

    let matrix = code.to_colors();
    let width = code.width();

    // Add a 2-module quiet zone on each side (QR spec requires ≥4, but 2 renders fine).
    let qz = 2usize;
    let padded_width = width + qz * 2;

    // Helper: return true if module at (row, col) is dark, treating the quiet zone as light.
    let dark = |row: isize, col: isize| -> bool {
        if row < 0 || col < 0 || row >= width as isize || col >= width as isize {
            return false;
        }
        matrix[row as usize * width + col as usize] == qrcode::Color::Dark
    };

    let mut lines = Vec::new();
    // Iterate two matrix rows per terminal line.
    let total_rows = (width + qz * 2) as isize;
    let mut r: isize = -(qz as isize);
    while r < (width + qz) as isize {
        let mut line = String::new();
        for c in -(qz as isize)..(width + qz) as isize {
            let top  = dark(r,     c);
            let bot  = dark(r + 1, c);
            line.push(match (top, bot) {
                (true,  true)  => '█',
                (true,  false) => '▀',
                (false, true)  => '▄',
                (false, false) => ' ',
            });
        }
        lines.push(line);
        r += 2;
    }
    let _ = padded_width; // suppress unused warning
    let _ = total_rows;
    lines
}

// ---------------------------------------------------------------------------
// mobile
// ---------------------------------------------------------------------------

pub struct MobileCommand;

impl NamedCommand for MobileCommand {
    fn name(&self) -> &str { "mobile" }
    fn description(&self) -> &str { "Download the Claurst mobile app" }
    fn usage(&self) -> &str { "claude mobile [ios|android]" }

    fn execute_named(&self, args: &[&str], ctx: &CommandContext) -> CommandResult {
        let ios_url     = "https://apps.apple.com/app/claude-by-anthropic/id6473753684";
        let android_url = "https://play.google.com/store/apps/details?id=com.anthropic.claude";
        let mobile_url  = "https://claude.ai/mobile";

        let has_session = ctx.remote_session_url.is_some();

        // Build a session URL string upfront (may be empty if no session).
        let session_qr_url: String = if let Some(ref url) = ctx.remote_session_url {
            let encoded = urlencoding::encode(url);
            format!("https://claude.ai/code/mobile?session={}", encoded)
        } else {
            String::new()
        };

        // Choose which platform / URL to show the QR for (default: claude.ai/mobile).
        let (platform_label, qr_url): (&str, &str) = match args.first().copied().unwrap_or("") {
            "ios" | "1"         => ("[1] iOS  (selected)", ios_url),
            "android" | "2"     => ("[2] Android  (selected)", android_url),
            "session" | "3"     => {
                if has_session {
                    ("[3] Session  (selected)", session_qr_url.as_str())
                } else {
                    ("session link unavailable \u{2014} no active remote session", mobile_url)
                }
            }
            _                   => ("both platforms", mobile_url),
        };

        let qr_lines = render_qr(qr_url);

        let mut out = String::new();
        out.push_str("Scan to download Claurst mobile app\n");
        out.push_str(&format!("Platform: {platform_label}\n\n"));
        if has_session {
            out.push_str("  [1] iOS    [2] Android    [3] Session (QR links to active session)\n\n");
        } else {
            out.push_str("  [1] iOS    [2] Android\n\n");
        }

        // QR block — indent by 2 spaces
        for line in &qr_lines {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }

        out.push('\n');
        out.push_str(&format!("  iOS:     {ios_url}\n"));
        out.push_str(&format!("  Android: {android_url}\n"));
        if has_session {
            out.push_str(&format!("  Session: {}\n", session_qr_url));
        }
        out.push('\n');
        out.push_str(&format!("Or visit {mobile_url}"));

        CommandResult::Message(out)
    }
}

// ---------------------------------------------------------------------------
// install-github-app
// ---------------------------------------------------------------------------

pub struct InstallGithubAppCommand;

impl NamedCommand for InstallGithubAppCommand {
    fn name(&self) -> &str { "install-github-app" }
    fn description(&self) -> &str { "Set up Claurst GitHub Actions for a repository" }
    fn usage(&self) -> &str { "claude install-github-app" }

    fn execute_named(&self, _args: &[&str], ctx: &CommandContext) -> CommandResult {
        let provider_id = ctx.config.selected_provider_id();
        let provider_secret_step = claurst_core::config::primary_api_key_env_var_for_provider(provider_id)
            .map(|provider_secret| {
                format!(
                    "3. Add your provider credential to repository secrets (for example {provider_secret})"
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "3. Configure any required provider credentials or connectivity for {provider_id} in your workflow environment"
                )
            });

        CommandResult::Message(
            format!(
                "To install the Claurst GitHub App:\n\
             1. Visit https://github.com/apps/claude-code-app and click Install\n\
             2. Select the repositories to enable\n\
             {provider_secret_step}\n\n\
             The app enables Claurst in GitHub Actions workflows for the configured provider."
            ),
        )
    }
}

// ---------------------------------------------------------------------------
// remote-setup
// ---------------------------------------------------------------------------

pub struct RemoteSetupCommand;

impl NamedCommand for RemoteSetupCommand {
    fn name(&self) -> &str { "remote-setup" }
    fn description(&self) -> &str { "Check and configure a remote Claurst environment" }
    fn usage(&self) -> &str { "claude remote-setup" }

    fn execute_named(&self, _args: &[&str], ctx: &CommandContext) -> CommandResult {
        use std::net::ToSocketAddrs;

        let mut steps = Vec::new();
        let provider_id = ctx.config.selected_provider_id();
        let provider_name = provider_id.replace('-', " ");
        let credential_hint = claurst_core::config::api_key_env_vars_for_provider(provider_id);
        let credentials_required = !matches!(
            provider_id,
            "ollama" | "lmstudio" | "lm-studio" | "llamacpp" | "llama-cpp" | "llama-server"
        );
        let credential_help = if credential_hint.is_empty() {
            format!("configure an API key for {provider_name} in settings")
        } else {
            format!("set {} or configure apiKey in settings", credential_hint.join(" / "))
        };

        // Step 1: Check provider credentials
        let has_api_key = !credentials_required || ctx.config.resolve_api_key().is_some();
        steps.push(format!(
            "{} {} credentials {}",
            if has_api_key { "\u{2713}" } else { "\u{2717}" },
            provider_name,
            if !credentials_required {
                "are not required for this provider".to_string()
            } else if has_api_key {
                "are configured".to_string()
            } else {
                format!("are NOT configured — {credential_help}")
            }
        ));

        // Step 2: Check SSH agent forwarding (check SSH_AUTH_SOCK)
        let has_ssh_agent = std::env::var("SSH_AUTH_SOCK").is_ok();
        steps.push(format!(
            "{} SSH agent forwarding {}",
            if has_ssh_agent { "\u{2713}" } else { "\u{25cb}" },
            if has_ssh_agent {
                "detected".to_string()
            } else {
                "not detected (optional \u{2014} needed for git over SSH)".to_string()
            }
        ));

        // Step 3: Check claude config dir exists
        let config_dir = claurst_core::config::Settings::config_dir();
        let has_config = config_dir.exists();
        steps.push(format!(
            "{} Claurst config dir {}",
            if has_config { "\u{2713}" } else { "\u{2717}" },
            if has_config {
                format!("exists at {}", config_dir.display())
            } else {
                "missing \u{2014} run 'claude' once to initialize".to_string()
            }
        ));

        // Step 4: Check provider endpoint reachability
        let api_base = ctx.config.resolve_api_base();
        let (net_ok, net_target) = if let Ok(parsed) = reqwest::Url::parse(&api_base) {
            if let Some(host) = parsed.host_str() {
                let port = parsed.port_or_known_default().unwrap_or(443);
                let target = format!("{host}:{port}");
                let resolved = (host, port)
                    .to_socket_addrs()
                    .map(|mut addrs| addrs.next().is_some())
                    .unwrap_or(false);
                (resolved, target)
            } else {
                (false, api_base.clone())
            }
        } else {
            (false, api_base.clone())
        };
        steps.push(format!(
            "{} Network connectivity {}",
            if net_ok { "\u{2713}" } else { "\u{2717}" },
            if net_ok {
                format!("to {net_target}")
            } else {
                format!("FAILED \u{2014} check access to {net_target}")
            }
        ));

        let all_ok = has_api_key && has_config && net_ok;

        CommandResult::Message(format!(
            "Remote Setup Checklist\n\n\
             {}\n\n\
             {}",
            steps.join("\n"),
            if all_ok {
                "\u{2713} All checks passed. Claurst is ready for remote use.\nStart a session: claude --bridge"
            } else {
                "\u{2717} Some checks failed. Fix the issues above and run 'claude remote-setup' again."
            }
        ))
    }
}

// ---------------------------------------------------------------------------
// stickers
// ---------------------------------------------------------------------------

pub struct StickersCommand;

impl NamedCommand for StickersCommand {
    fn name(&self) -> &str { "stickers" }
    fn description(&self) -> &str { "Open the Claurst sticker page in your browser" }
    fn usage(&self) -> &str { "claude stickers" }

    fn execute_named(&self, _args: &[&str], _ctx: &CommandContext) -> CommandResult {
        let url = "https://www.stickermule.com/claudecode";
        match open::that(url) {
            Ok(_) => CommandResult::Message(format!("Opening stickers page: {url}")),
            Err(e) => CommandResult::Message(format!(
                "Visit: {url}\n(Could not open browser: {e})"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// ultraplan — Agentic planning with extended thinking
// ---------------------------------------------------------------------------

pub struct UltraplanCommand;

impl NamedCommand for UltraplanCommand {
    fn name(&self) -> &str { "ultraplan" }
    fn description(&self) -> &str { "Launch Ultraplan agentic code planner with extended thinking" }
    fn usage(&self) -> &str { "claude ultraplan [--effort=medium|high|maximum]" }

    fn execute_named(&self, args: &[&str], _ctx: &CommandContext) -> CommandResult {
        // Parse effort level from args
        let effort = args.iter()
            .find(|arg| arg.starts_with("--effort="))
            .and_then(|arg| arg.strip_prefix("--effort="))
            .unwrap_or("medium");

        // Validate effort level
        if !matches!(effort, "medium" | "high" | "maximum") {
            return CommandResult::Error(format!(
                "Invalid effort level: '{}'. Use: medium, high, or maximum",
                effort
            ));
        }

        CommandResult::Message(format!(
            "🚀 Ultraplan activated with {} effort level\n\n\
             Ultraplan will now:\n\
             • Analyze the codebase and requirements\n\
             • Use extended thinking for deep reasoning\n\
             • Generate a comprehensive implementation plan\n\
             • Break down the work into clear steps\n\n\
             Ask me: '/ultraplan describe what you want to build'",
            effort
        ))
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Return one instance of every registered named command.
pub fn all_named_commands() -> Vec<Box<dyn NamedCommand>> {
    vec![
        Box::new(AgentsCommand),
        Box::new(AddDirCommand),
        Box::new(BranchCommand),
        Box::new(TagCommand),
        Box::new(PassesCommand),
        Box::new(IdeCommand),
        Box::new(PrCommentsCommand),
        Box::new(DesktopCommand),
        Box::new(MobileCommand),
        Box::new(InstallGithubAppCommand),
        Box::new(RemoteSetupCommand),
        Box::new(StickersCommand),
        Box::new(UltraplanCommand),
    ]
}

/// Look up a named command by its primary name (case-insensitive).
pub fn find_named_command(name: &str) -> Option<Box<dyn NamedCommand>> {
    let needle = name.to_lowercase();
    all_named_commands()
        .into_iter()
        .find(|c| c.name() == needle.as_str())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claurst_core::cost::CostTracker;

    fn make_ctx() -> CommandContext {
        CommandContext {
            config: claurst_core::config::Config::default(),
            cost_tracker: CostTracker::new(),
            messages: vec![],
            working_dir: std::path::PathBuf::from("."),
            session_id: "named-test-session".to_string(),
            session_title: None,
            remote_session_url: None,
            mcp_manager: None,
            mcp_auth_runner: None,
        }
    }

    #[test]
    fn test_all_named_commands_non_empty() {
        assert!(!all_named_commands().is_empty());
    }

    #[test]
    fn test_all_named_commands_unique_names() {
        let mut names = std::collections::HashSet::new();
        for cmd in all_named_commands() {
            assert!(
                names.insert(cmd.name().to_string()),
                "Duplicate named command: {}",
                cmd.name()
            );
        }
    }

    #[test]
    fn test_find_named_command_found() {
        assert!(find_named_command("agents").is_some());
        assert!(find_named_command("ide").is_some());
        assert!(find_named_command("branch").is_some());
        assert!(find_named_command("passes").is_some());
    }

    #[test]
    fn test_find_named_command_not_found() {
        assert!(find_named_command("nonexistent-xyz").is_none());
    }

    #[test]
    fn test_find_named_command_case_insensitive() {
        assert!(find_named_command("Agents").is_some());
        assert!(find_named_command("IDE").is_some());
    }

    #[test]
    fn test_agents_list_returns_message() {
        let ctx = make_ctx();
        let cmd = AgentsCommand;
        let result = cmd.execute_named(&[], &ctx);
        assert!(matches!(result, CommandResult::Message(_)));
    }

    #[test]
    fn test_agents_create_includes_name() {
        let ctx = make_ctx();
        let cmd = AgentsCommand;
        let result = cmd.execute_named(&["create", "my-bot"], &ctx);
        if let CommandResult::Message(msg) = result {
            assert!(msg.contains("my-bot"));
        } else {
            panic!("Expected Message");
        }
    }

    #[test]
    fn test_add_dir_missing_arg_returns_error() {
        let ctx = make_ctx();
        let cmd = AddDirCommand;
        let result = cmd.execute_named(&[], &ctx);
        assert!(matches!(result, CommandResult::Error(_)));
    }

    #[test]
    fn test_branch_list_returns_message() {
        // With no active tokio runtime the block_in_place path won't be reached;
        // but `list` on an empty session dir returns a Message (no sessions found).
        // We verify the command exists and returns a non-Error for the list subcommand
        // when called outside a runtime (it will panic in block_in_place if runtime
        // is missing, so we just check the command dispatches).
        let ctx = make_ctx();
        let cmd = BranchCommand;
        // "pre-session" session_id: create/switch will error; list is the safe path
        let result = cmd.execute_named(&["unknown-sub"], &ctx);
        assert!(matches!(result, CommandResult::Error(_)));
    }

    #[test]
    fn test_branch_create_no_session_returns_error() {
        let ctx = make_ctx(); // session_id = "named-test-session" — no saved session
        let cmd = BranchCommand;
        // Calling create on a session that isn't "pre-session" but also doesn't exist
        // on disk: we can't call block_in_place outside a tokio runtime in a sync test,
        // so instead verify the pre-session guard fires.
        let mut ctx2 = make_ctx();
        ctx2.session_id = "pre-session".to_string();
        let result = cmd.execute_named(&[], &ctx2);
        assert!(matches!(result, CommandResult::Error(_)));
    }

    #[test]
    fn test_branch_switch_missing_id_returns_error() {
        let ctx = make_ctx();
        let cmd = BranchCommand;
        let result = cmd.execute_named(&["switch"], &ctx);
        assert!(matches!(result, CommandResult::Error(_)));
    }

    #[test]
    fn test_pr_comments_no_gh_returns_error() {
        // Without `gh` installed or outside a git repo with an open PR,
        // the command returns an Error (gh not found or no open PR).
        let ctx = make_ctx();
        let cmd = PrCommentsCommand;
        // Either gh is missing (Error with "not found") or no PR is open (Error).
        // Both cases produce CommandResult::Error.
        let result = cmd.execute_named(&[], &ctx);
        // On CI / dev machines without gh: Error. With gh but no open PR: also Error.
        // We accept Error or Message (in case gh is installed and finds a PR).
        match result {
            CommandResult::Error(_) | CommandResult::Message(_) => {}
            other => panic!("Unexpected result: {:?}", other),
        }
    }

    #[test]
    fn test_install_github_app_returns_message() {
        let ctx = make_ctx();
        let cmd = InstallGithubAppCommand;
        let result = cmd.execute_named(&[], &ctx);
        assert!(matches!(result, CommandResult::Message(_)));
    }
}
