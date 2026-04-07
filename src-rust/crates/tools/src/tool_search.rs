// ToolSearchTool: search for tools by name or keyword.
//
// This is used by the model to discover "deferred" tools that are not yet
// loaded into context. In the Rust port there is no deferred-tool mechanism
// (all tools are always available), but this tool still provides a useful
// search interface for the model to discover available capabilities.
//
// Supports two query modes:
//   - "select:ToolName"  → direct lookup by exact name
//   - "keyword search"   → fuzzy name + description match with scoring

use crate::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

pub struct ToolSearchTool;

#[derive(Debug, Deserialize)]
struct ToolSearchInput {
    query: String,
    #[serde(default = "default_max")]
    max_results: usize,
}

fn default_max() -> usize {
    5
}

/// A minimal catalog entry describing one tool.
#[derive(Debug, Clone)]
struct ToolEntry {
    name: &'static str,
    description: &'static str,
    keywords: &'static [&'static str],
}

/// Static catalog of all built-in tools with keywords for scoring.
static TOOL_CATALOG: &[ToolEntry] = &[
    ToolEntry {
        name: "Bash",
        description: "Execute shell commands",
        keywords: &["shell", "run", "command", "exec", "terminal"],
    },
    ToolEntry {
        name: "Read",
        description: "Read file contents",
        keywords: &["file", "read", "cat", "content"],
    },
    ToolEntry {
        name: "Write",
        description: "Write or create files",
        keywords: &["file", "write", "create", "save"],
    },
    ToolEntry {
        name: "Edit",
        description: "Edit existing files with string replacement",
        keywords: &["file", "edit", "modify", "replace", "patch"],
    },
    ToolEntry {
        name: "Glob",
        description: "Find files by pattern",
        keywords: &["find", "pattern", "search", "files", "glob"],
    },
    ToolEntry {
        name: "Grep",
        description: "Search file contents with regex",
        keywords: &["search", "regex", "grep", "find", "content"],
    },
    ToolEntry {
        name: "WebFetch",
        description: "Fetch web page content",
        keywords: &["web", "fetch", "http", "url", "browser"],
    },
    ToolEntry {
        name: "WebSearch",
        description: "Search the web",
        keywords: &["web", "search", "internet", "query"],
    },
    ToolEntry {
        name: "NotebookEdit",
        description: "Edit Jupyter notebook cells",
        keywords: &["notebook", "jupyter", "ipynb", "cell"],
    },
    ToolEntry {
        name: "TodoWrite",
        description: "Manage todo list",
        keywords: &["todo", "task", "list", "write"],
    },
    ToolEntry {
        name: "AskUserQuestion",
        description: "Ask the user a question",
        keywords: &["ask", "question", "user", "input", "clarify"],
    },
    ToolEntry {
        name: "EnterPlanMode",
        description: "Enter planning mode",
        keywords: &["plan", "mode", "planning"],
    },
    ToolEntry {
        name: "ExitPlanMode",
        description: "Exit planning mode",
        keywords: &["plan", "exit", "mode"],
    },
    ToolEntry {
        name: "Sleep",
        description: "Wait for a duration",
        keywords: &["sleep", "wait", "delay", "pause"],
    },
    ToolEntry {
        name: "PowerShell",
        description: "Execute PowerShell commands",
        keywords: &["powershell", "windows", "ps", "command"],
    },
    ToolEntry {
        name: "CronCreate",
        description: "Schedule a recurring cron task",
        keywords: &["cron", "schedule", "recurring", "timer"],
    },
    ToolEntry {
        name: "CronDelete",
        description: "Cancel a scheduled cron task",
        keywords: &["cron", "delete", "cancel", "remove"],
    },
    ToolEntry {
        name: "CronList",
        description: "List all cron tasks",
        keywords: &["cron", "list", "scheduled", "tasks"],
    },
    ToolEntry {
        name: "EnterWorktree",
        description: "Create and enter a git worktree",
        keywords: &["worktree", "git", "branch", "isolate"],
    },
    ToolEntry {
        name: "ExitWorktree",
        description: "Exit the current git worktree",
        keywords: &["worktree", "git", "exit", "restore"],
    },
    ToolEntry {
        name: "TaskCreate",
        description: "Create a background task",
        keywords: &["task", "create", "background", "async"],
    },
    ToolEntry {
        name: "TaskGet",
        description: "Get task details",
        keywords: &["task", "get", "status", "details"],
    },
    ToolEntry {
        name: "TaskUpdate",
        description: "Update a task's status",
        keywords: &["task", "update", "status", "progress"],
    },
    ToolEntry {
        name: "TaskList",
        description: "List all tasks",
        keywords: &["task", "list", "all", "tasks"],
    },
    ToolEntry {
        name: "TaskStop",
        description: "Stop a running task",
        keywords: &["task", "stop", "kill", "cancel"],
    },
    ToolEntry {
        name: "TaskOutput",
        description: "Get task output/logs",
        keywords: &["task", "output", "logs", "result"],
    },
    ToolEntry {
        name: "ListMcpResources",
        description: "List MCP server resources",
        keywords: &["mcp", "resource", "list", "server"],
    },
    ToolEntry {
        name: "ReadMcpResource",
        description: "Read an MCP resource",
        keywords: &["mcp", "resource", "read", "server"],
    },
    ToolEntry {
        name: "Agent",
        description: "Launch a sub-agent for complex tasks",
        keywords: &["agent", "subagent", "task", "parallel", "delegate"],
    },
    ToolEntry {
        name: "Brief",
        description: "Send a formatted message to the user",
        keywords: &[
            "brief",
            "message",
            "notify",
            "proactive",
            "status",
            "update",
        ],
    },
    ToolEntry {
        name: "Config",
        description: "Get or set Claurst configuration",
        keywords: &[
            "config",
            "settings",
            "model",
            "verbose",
            "permission",
            "configure",
        ],
    },
    ToolEntry {
        name: "SendMessage",
        description: "Send a message to another agent",
        keywords: &[
            "send",
            "message",
            "agent",
            "broadcast",
            "communicate",
            "inbox",
        ],
    },
    ToolEntry {
        name: "Skill",
        description: "Execute a skill prompt template",
        keywords: &["skill", "command", "template", "prompt", "slash", "custom"],
    },
];

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "ToolSearch"
    }

    fn description(&self) -> &str {
        "Search for available tools by name or keyword. Use 'select:ToolName' for direct \
         lookup or provide keywords for fuzzy search. Returns matching tool names and their \
         descriptions. Max 5 results by default."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Query: use 'select:ToolName' for direct selection, or keywords to search"
                },
                "max_results": {
                    "type": "number",
                    "description": "Maximum results to return (default: 5)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let params: ToolSearchInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        let query = params.query.trim();
        let max = params.max_results.min(20);

        // select: prefix — direct lookup
        if let Some(names_str) = query.strip_prefix("select:").map(str::trim) {
            let requested: Vec<&str> = names_str.split(',').map(str::trim).collect();
            let mut found = Vec::new();
            let mut missing = Vec::new();

            for name in requested {
                if let Some(entry) = TOOL_CATALOG
                    .iter()
                    .find(|e| e.name.eq_ignore_ascii_case(name))
                {
                    found.push(format!("{}: {}", entry.name, entry.description));
                } else {
                    missing.push(name.to_string());
                }
            }

            if found.is_empty() {
                return ToolResult::success(format!(
                    "No matching tools found for: {}",
                    missing.join(", ")
                ));
            }

            let mut out = found.join("\n");
            if !missing.is_empty() {
                out.push_str(&format!("\n\nNot found: {}", missing.join(", ")));
            }
            return ToolResult::success(out);
        }

        // Keyword search with scoring
        let q_lower = query.to_lowercase();
        let terms: Vec<&str> = q_lower.split_whitespace().collect();

        let mut scored: Vec<(usize, &ToolEntry)> = TOOL_CATALOG
            .iter()
            .filter_map(|entry| {
                let mut score = 0usize;
                let name_lower = entry.name.to_lowercase();
                let desc_lower = entry.description.to_lowercase();

                for term in &terms {
                    // Exact name match
                    if name_lower == *term {
                        score += 20;
                    } else if name_lower.contains(term) {
                        score += 10;
                    }

                    // Description match
                    if desc_lower.contains(term) {
                        score += 5;
                    }

                    // Keyword match
                    for &kw in entry.keywords {
                        if kw == *term {
                            score += 8;
                        } else if kw.contains(term) {
                            score += 3;
                        }
                    }
                }

                if score > 0 {
                    Some((score, entry))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.truncate(max);

        if scored.is_empty() {
            return ToolResult::success(format!(
                "No tools found matching '{}'. Try broader keywords or use 'select:ToolName'.",
                query
            ));
        }

        let lines: Vec<String> = scored
            .iter()
            .map(|(_, e)| format!("{}: {}", e.name, e.description))
            .collect();

        ToolResult::success(format!(
            "Tools matching '{}':\n\n{}\n\nTotal tools available: {}",
            query,
            lines.join("\n"),
            TOOL_CATALOG.len()
        ))
    }
}
