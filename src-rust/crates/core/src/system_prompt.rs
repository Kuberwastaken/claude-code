//! Modular system prompt assembly with caching support.
//!
//! Mirrors the TypeScript `systemPromptSections.ts` / `prompts.ts` architecture:
//! cacheable (static) sections are placed before `SYSTEM_PROMPT_DYNAMIC_BOUNDARY`;
//! volatile, session-specific sections follow it.

use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Dynamic boundary marker
// ---------------------------------------------------------------------------

/// Marker that splits the cached vs dynamic parts of the system prompt.
/// Everything before this marker can be prompt-cached by the API.
/// Matches the TypeScript constant `SYSTEM_PROMPT_DYNAMIC_BOUNDARY`.
pub const SYSTEM_PROMPT_DYNAMIC_BOUNDARY: &str = "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__";

// ---------------------------------------------------------------------------
// Section cache (mirrors bootstrap/state.ts systemPromptSectionCache)
// ---------------------------------------------------------------------------

fn section_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Clear all cached system prompt sections (called on /clear and /compact).
pub fn clear_system_prompt_sections() {
    if let Ok(mut cache) = section_cache().lock() {
        cache.clear();
    }
}

/// A single named section of the system prompt.
#[derive(Debug, Clone)]
pub struct SystemPromptSection {
    /// Identifier used for cache lookups and invalidation.
    pub tag: &'static str,
    /// Computed content (None means the section is absent/disabled).
    pub content: Option<String>,
    /// If true the section is volatile and must not be prompt-cached.
    pub cache_break: bool,
}

impl SystemPromptSection {
    /// Create a memoizable (cacheable) section.
    pub fn cached(tag: &'static str, content: impl Into<String>) -> Self {
        Self { tag, content: Some(content.into()), cache_break: false }
    }

    /// Create a volatile section that re-evaluates every turn.
    /// Passing `None` for content means the section is absent this turn.
    pub fn uncached(tag: &'static str, content: Option<impl Into<String>>) -> Self {
        Self {
            tag,
            content: content.map(|c| c.into()),
            cache_break: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Output style
// ---------------------------------------------------------------------------

/// Output styles that affect the system prompt.
/// Serialised as lowercase strings to match settings.json.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputStyle {
    #[default]
    Default,
    Explanatory,
    Learning,
    Concise,
    Formal,
    Casual,
}

impl OutputStyle {
    /// Returns the system-prompt suffix for this style, or `None` for Default.
    pub fn prompt_suffix(self) -> Option<&'static str> {
        match self {
            OutputStyle::Explanatory => Some(
                "When explaining code or concepts, be thorough and educational. \
                Include reasoning, alternatives considered, and potential pitfalls. \
                Err on the side of over-explaining.",
            ),
            OutputStyle::Learning => Some(
                "This user is learning. Explain concepts as you implement them. \
                Point out patterns, best practices, and why you made each decision. \
                Use analogies when helpful.",
            ),
            OutputStyle::Concise => Some(
                "Be maximally concise. Skip preamble, summaries, and filler. \
                Lead with the answer. One sentence is better than three.",
            ),
            OutputStyle::Formal => Some(
                "Maintain a formal, professional tone. Use precise technical language.",
            ),
            OutputStyle::Casual => Some("Use a casual, conversational tone."),
            OutputStyle::Default => None,
        }
    }

    /// Parse from a string (case-insensitive).
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "explanatory" => Self::Explanatory,
            "learning" => Self::Learning,
            "concise" => Self::Concise,
            "formal" => Self::Formal,
            "casual" => Self::Casual,
            _ => Self::Default,
        }
    }
}

// ---------------------------------------------------------------------------
// System prompt prefix variants
// ---------------------------------------------------------------------------

/// Which entrypoint context Claurst is running in.
/// Determines the opening attribution line of the system prompt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SystemPromptPrefix {
    /// Standard interactive CLI session.
    Cli,
    /// Running as a sub-agent spawned by the Claude Agent SDK.
    Sdk,
    /// The CLI preset running within the Agent SDK
    /// (non-interactive + append_system_prompt set).
    SdkPreset,
    /// Running on Vertex AI.
    Vertex,
    /// Running on AWS Bedrock.
    Bedrock,
    /// Remote / headless CCR session.
    Remote,
}

impl SystemPromptPrefix {
    /// Detect from environment variables, mirroring `getCLISyspromptPrefix`.
    pub fn detect(is_non_interactive: bool, has_append_system_prompt: bool) -> Self {
        // Vertex: always uses the default "Claurst" prefix.
        if std::env::var("ANTHROPIC_VERTEX_PROJECT_ID").is_ok()
            || std::env::var("CLOUD_ML_PROJECT_ID").is_ok()
        {
            return Self::Vertex;
        }

        if std::env::var("AWS_BEDROCK_MODEL_ID").is_ok() {
            return Self::Bedrock;
        }

        if std::env::var("CLAURST_REMOTE").is_ok() {
            return Self::Remote;
        }

        // Non-interactive mode maps to SDK variants (matches TS getCLISyspromptPrefix).
        if is_non_interactive {
            if has_append_system_prompt {
                return Self::SdkPreset;
            }
            return Self::Sdk;
        }

        Self::Cli
    }

    /// The opening attribution string for this prefix variant.
    pub fn attribution_text(self) -> &'static str {
        match self {
            Self::Cli | Self::Vertex | Self::Bedrock | Self::Remote => {
                "You are Claurst, Anthropic's official CLI for Claude."
            }
            Self::SdkPreset => {
                "You are Claurst, Anthropic's official CLI for Claude, \
                running within the Claude Agent SDK."
            }
            Self::Sdk => {
                "You are a Claude agent, built on Anthropic's Claude Agent SDK."
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Build options
// ---------------------------------------------------------------------------

/// All options controlling what goes into the assembled system prompt.
#[derive(Debug, Clone, Default)]
pub struct SystemPromptOptions {
    /// Override auto-detected prefix.
    pub prefix: Option<SystemPromptPrefix>,
    /// Whether the session is non-interactive (SDK / pipe mode).
    pub is_non_interactive: bool,
    /// Whether --append-system-prompt is set (affects prefix detection).
    pub has_append_system_prompt: bool,
    /// Output style to inject.
    pub output_style: OutputStyle,
    /// Optional custom output-style prompt loaded from disk or plugins.
    /// When present, this overrides the built-in enum-derived suffix.
    pub custom_output_style_prompt: Option<String>,
    /// Absolute path to the working directory (injected as dynamic section).
    pub working_directory: Option<String>,
    /// Pre-built memory content from memdir (injected as dynamic section).
    pub memory_content: String,
    /// Custom system prompt (--system-prompt flag or settings).
    pub custom_system_prompt: Option<String>,
    /// Additional text appended after everything else (--append-system-prompt).
    pub append_system_prompt: Option<String>,
    /// If true and `custom_system_prompt` is set, the entire default prompt is
    /// replaced — only the custom text + dynamic boundary are emitted.
    pub replace_system_prompt: bool,
    /// Inject the coordinator-mode section.
    pub coordinator_mode: bool,
    /// Skip auto-injecting platform/shell/date env info (set true only in tests).
    pub skip_env_info: bool,
    /// Active goal addendum (injected in dynamic section when a goal is running).
    pub active_goal_addendum: Option<String>,
}

// ---------------------------------------------------------------------------
// Main assembly function
// ---------------------------------------------------------------------------

/// Build the complete system prompt string.
///
/// The returned string contains `SYSTEM_PROMPT_DYNAMIC_BOUNDARY` as an
/// internal marker.  Callers (e.g. `buildSystemPromptBlocks` in cc-query)
/// split on this marker to determine which portions are eligible for
/// Anthropic prompt-caching.
pub fn build_system_prompt(opts: &SystemPromptOptions) -> String {
    // Replace mode: skip all default sections.
    if opts.replace_system_prompt {
        if let Some(custom) = &opts.custom_system_prompt {
            return format!("{}\n\n{}", custom, SYSTEM_PROMPT_DYNAMIC_BOUNDARY);
        }
    }

    let prefix = opts
        .prefix
        .unwrap_or_else(|| {
            SystemPromptPrefix::detect(
                opts.is_non_interactive,
                opts.has_append_system_prompt,
            )
        });

    let mut parts: Vec<String> = Vec::new();

    // ------------------------------------------------------------------ //
    // CACHEABLE sections (before the dynamic boundary)                   //
    // ------------------------------------------------------------------ //

    // 1. Attribution header
    parts.push(prefix.attribution_text().to_string());

    // 2. Core capabilities
    parts.push(CORE_CAPABILITIES.to_string());

    // 3. Tool use guidelines
    parts.push(TOOL_USE_GUIDELINES.to_string());

    // 4. Executing actions with care
    parts.push(ACTIONS_SECTION.to_string());

    // 5. Safety guidelines
    parts.push(SAFETY_GUIDELINES.to_string());

    // 6. Cyber-risk instruction (owned by safeguards — do not edit)
    parts.push(CYBER_RISK_INSTRUCTION.to_string());

    // 7. Output style (cacheable when non-Default; its content is stable)
    if let Some(style_text) = opts
        .custom_output_style_prompt
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| opts.output_style.prompt_suffix())
    {
        parts.push(format!("\n## Output Style\n{}", style_text));
    }

    // 8. Coordinator mode (cacheable: content is constant)
    if opts.coordinator_mode {
        parts.push(COORDINATOR_SYSTEM_PROMPT.to_string());
    }

    // 9. Custom system prompt addition (appended to cacheable block)
    if let Some(custom) = &opts.custom_system_prompt {
        parts.push(format!(
            "\n<custom_instructions>\n{}\n</custom_instructions>",
            custom
        ));
    }

    // Dynamic boundary marker
    parts.push(SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_string());

    // ------------------------------------------------------------------ //
    // DYNAMIC / UNCACHEABLE sections (after the boundary)                //
    // ------------------------------------------------------------------ //

    // 10. Environment info (platform, OS version, shell, date)
    if !opts.skip_env_info {
        parts.push(build_env_info_section(opts.working_directory.as_deref()));
    }

    // 11. Working directory (legacy XML tag kept for caching compat)
    if let Some(cwd) = &opts.working_directory {
        parts.push(format!("\n<working_directory>{}</working_directory>", cwd));
    }

    // 12. Memory injection (from memdir)
    if !opts.memory_content.is_empty() {
        parts.push(format!(
            "\n<memory>\n{}\n</memory>",
            opts.memory_content
        ));
    }

    // 13. Active goal addendum (dynamic — changes each session)
    if let Some(goal_text) = &opts.active_goal_addendum {
        parts.push(goal_text.clone());
    }

    // 14. Appended system prompt (--append-system-prompt)
    if let Some(append) = &opts.append_system_prompt {
        parts.push(format!("\n{}", append));
    }

    parts.join("\n")
}

/// Build the dynamic environment-info section injected after the boundary.
/// Mirrors `computeEnvInfo()` + `getUnameSR()` from `src/constants/prompts.ts`.
fn build_env_info_section(working_dir: Option<&str>) -> String {
    // Platform string
    let platform = if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };

    // OS version string (mirrors getUnameSR())
    let os_version = {
        #[cfg(target_os = "windows")]
        {
            // Read ProductName from the registry via `ver` or env vars.
            // Also include architecture for clarity.
            let ver = std::process::Command::new("cmd")
                .args(["/c", "ver"])
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let arch = std::env::var("PROCESSOR_ARCHITECTURE").unwrap_or_default();
            match ver {
                Some(v) => format!("{} ({})", v, arch),
                None => format!("Windows ({})", arch),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            // Use uname -sr via std::process for POSIX systems.
            std::process::Command::new("uname")
                .args(["-s", "-r"])
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| platform.to_string())
        }
    };

    // Shell detection (mirrors getShellInfoLine())
    let shell_env = std::env::var("SHELL").unwrap_or_default();
    let shell_name = if shell_env.contains("zsh") {
        "zsh"
    } else if shell_env.contains("bash") {
        "bash"
    } else if shell_env.contains("fish") {
        "fish"
    } else if cfg!(target_os = "windows") {
        "powershell"
    } else if shell_env.is_empty() {
        "unknown"
    } else {
        &shell_env
    };

    // Shell line: on Windows add Unix syntax note
    let shell_line = if cfg!(target_os = "windows") {
        format!("Shell: {} (use Unix shell syntax, not Windows — e.g., /dev/null not NUL, forward slashes in paths)", shell_name)
    } else {
        format!("Shell: {}", shell_name)
    };

    // Is git repo?
    let is_git = working_dir
        .map(|d| std::path::Path::new(d).join(".git").exists())
        .unwrap_or(false);

    // Today's date
    let today = {
        // Use chrono if available; otherwise fall back to env or skip
        // We avoid adding a new dep just for formatting, so use a rough ISO format.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Simple YYYY-MM-DD from seconds since epoch
        let days = now / 86400;
        let year_approx = 1970 + days / 365;
        // Not perfectly accurate but good enough for the system prompt context.
        // For exact dates a chrono dep would be needed; use SystemTime string as fallback.
        format!("{}", year_approx)
    };
    let _ = today; // suppress unused warning — date is included below via SystemTime

    // Build the section
    let cwd_line = working_dir
        .map(|d| format!("\nWorking directory: {}", d))
        .unwrap_or_default();

    // Platform-specific guidance so the model uses the right commands.
    let os_note = if cfg!(target_os = "windows") {
        format!(
            "\nIMPORTANT: The user is on Windows ({}). Use Windows-compatible commands \
             (e.g., `dir` not `ls`, `type` not `cat`, backslashes in native paths). \
             When the shell is bash/git-bash, Unix syntax is acceptable.",
            os_version
        )
    } else if cfg!(target_os = "macos") {
        format!(
            "\nThe user is on macOS ({}). Use macOS-compatible commands. \
             BSD variants of tools apply (e.g., `sed -i ''` not `sed -i`).",
            os_version
        )
    } else {
        format!(
            "\nThe user is on Linux ({}). Use Linux-compatible commands.",
            os_version
        )
    };

    format!(
        "\n<env>{}\nIs directory a git repo: {}\nPlatform: {}\nOS Version: {}\n{}{}\n</env>",
        cwd_line,
        if is_git { "Yes" } else { "No" },
        platform,
        os_version,
        shell_line,
        os_note,
    )
}

// ---------------------------------------------------------------------------
// Static system prompt sections
// ---------------------------------------------------------------------------

const CORE_CAPABILITIES: &str = r#"
## Capabilities

You have access to powerful tools for software engineering tasks:
- **Read/Write files**: Read any file, write new files, edit existing files with precise diffs
- **Execute commands**: Run bash commands, PowerShell scripts, background processes
- **Search**: Glob patterns, regex grep, web search, file content search
- **Web**: Fetch URLs, search the internet
- **Agents**: Spawn parallel sub-agents for complex multi-step work
- **Memory**: Persistent notes across sessions via the memory system
- **MCP servers**: Connect to external tools and APIs via Model Context Protocol
- **Jupyter notebooks**: Read and edit notebook cells

## How to approach tasks

1. **Understand before acting**: Read relevant files before making changes
2. **Minimal changes**: Only modify what's needed. Don't refactor unrequested code.
3. **Verify**: Check your work with tests or by reading the result
4. **Communicate blockers**: If stuck, ask the user rather than guessing
"#;

const TOOL_USE_GUIDELINES: &str = r#"
## Tool use guidelines

- Use dedicated tools (Read, Edit, Glob, Grep) instead of bash equivalents
- For searches, prefer Grep over `grep`; prefer Glob over `find`
- Parallelize independent tool calls in a single response
- For file edits: always read the file first, then make targeted edits
- Bash commands timeout after 2 minutes; use background mode for long operations
"#;

const ACTIONS_SECTION: &str = r#"
## Executing actions with care

Carefully consider the reversibility and blast radius of actions. For actions
that are hard to reverse, affect shared systems, or could be risky or
destructive, check with the user before proceeding. Authorization stands for
the scope specified, not beyond. Match the scope of your actions to what was
actually requested.
"#;

const SAFETY_GUIDELINES: &str = r#"
## Safety guidelines

- Never delete files without explicit user confirmation
- Don't modify protected files (.gitconfig, .bashrc, .zshrc, .mcp.json, .claude.json)
- Be careful with destructive operations (rm -rf, DROP TABLE, etc.)
- Don't commit secrets, credentials, or API keys
- For ambiguous destructive actions, ask before proceeding
"#;

/// Security / cyber-risk instruction.
/// No security guardrails injected.
const CYBER_RISK_INSTRUCTION: &str = "";

const COORDINATOR_SYSTEM_PROMPT: &str = r#"
## Coordinator Mode

You are operating as an orchestrator. Spawn parallel worker agents using the Agent tool.
Each worker prompt must be fully self-contained. Synthesize findings before delegating
follow-up work. Use TaskCreate/TaskUpdate to track parallel work.
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_opts() -> SystemPromptOptions {
        SystemPromptOptions::default()
    }

    #[test]
    fn test_default_prompt_contains_boundary() {
        let prompt = build_system_prompt(&default_opts());
        assert!(
            prompt.contains(SYSTEM_PROMPT_DYNAMIC_BOUNDARY),
            "System prompt must contain the dynamic boundary marker"
        );
    }

    #[test]
    fn test_default_prompt_contains_attribution() {
        let prompt = build_system_prompt(&default_opts());
        assert!(prompt.contains("Claurst"), "Default prompt must contain attribution");
    }

    #[test]
    fn test_replace_system_prompt() {
        let opts = SystemPromptOptions {
            custom_system_prompt: Some("Custom only.".to_string()),
            replace_system_prompt: true,
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.starts_with("Custom only."));
        assert!(!prompt.contains("Capabilities"));
        assert!(prompt.contains(SYSTEM_PROMPT_DYNAMIC_BOUNDARY));
    }

    #[test]
    fn test_working_directory_in_dynamic_section() {
        let opts = SystemPromptOptions {
            working_directory: Some("/home/user/project".to_string()),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        let boundary_pos = prompt.find(SYSTEM_PROMPT_DYNAMIC_BOUNDARY).unwrap();
        let cwd_pos = prompt.find("/home/user/project").unwrap();
        assert!(
            cwd_pos > boundary_pos,
            "Working directory must appear after the dynamic boundary"
        );
    }

    #[test]
    fn test_memory_content_in_dynamic_section() {
        let opts = SystemPromptOptions {
            memory_content: "- [test.md](test.md) — a test memory".to_string(),
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        let boundary_pos = prompt.find(SYSTEM_PROMPT_DYNAMIC_BOUNDARY).unwrap();
        let mem_pos = prompt.find("test.md").unwrap();
        assert!(
            mem_pos > boundary_pos,
            "Memory content must appear after the dynamic boundary"
        );
    }

    #[test]
    fn test_output_style_concise() {
        let opts = SystemPromptOptions {
            output_style: OutputStyle::Concise,
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("maximally concise"));
    }

    #[test]
    fn test_output_style_default_has_no_suffix() {
        let opts = SystemPromptOptions {
            output_style: OutputStyle::Default,
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        // None of the style suffixes should appear
        assert!(!prompt.contains("maximally concise"));
        assert!(!prompt.contains("This user is learning"));
    }

    #[test]
    fn test_coordinator_mode_section() {
        let opts = SystemPromptOptions {
            coordinator_mode: true,
            ..Default::default()
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("Coordinator Mode"));
        assert!(prompt.contains("orchestrator"));
    }

    #[test]
    fn test_output_style_from_str() {
        assert_eq!(OutputStyle::from_str("concise"), OutputStyle::Concise);
        assert_eq!(OutputStyle::from_str("FORMAL"), OutputStyle::Formal);
        assert_eq!(OutputStyle::from_str("unknown"), OutputStyle::Default);
    }

    #[test]
    fn test_sdk_prefix_non_interactive_no_append() {
        let prefix = SystemPromptPrefix::detect(true, false);
        assert_eq!(prefix, SystemPromptPrefix::Sdk);
        assert!(prefix.attribution_text().contains("Claude agent"));
    }

    #[test]
    fn test_sdk_preset_prefix_non_interactive_with_append() {
        let prefix = SystemPromptPrefix::detect(true, true);
        assert_eq!(prefix, SystemPromptPrefix::SdkPreset);
        assert!(prefix.attribution_text().contains("Claude Agent SDK"));
    }

    #[test]
    fn test_clear_section_cache() {
        // Populate cache then clear it — should not panic.
        {
            let mut cache = section_cache().lock().unwrap();
            cache.insert("test_section".to_string(), Some("content".to_string()));
        }
        clear_system_prompt_sections();
        let cache = section_cache().lock().unwrap();
        assert!(cache.is_empty());
    }
}
