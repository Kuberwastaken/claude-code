# Claurst Tools Reference

This document is the complete reference for every tool available to the Claurst agent. Tools are the mechanism by which the model interacts with the outside world — reading files, running commands, searching the web, and coordinating sub-agents.

---

## Table of Contents

1. [Tool System Overview](#tool-system-overview)
2. [Permission System](#permission-system)
3. [File Tools](#file-tools)
4. [Shell Execution Tools](#shell-execution-tools)
5. [Search Tools](#search-tools)
6. [Web Tools](#web-tools)
7. [Task Management Tools](#task-management-tools)
8. [MCP Integration Tools](#mcp-integration-tools)
9. [Agent Tools](#agent-tools)
10. [Notebook Tools](#notebook-tools)
11. [Planning Tools](#planning-tools)
12. [Worktree Tools](#worktree-tools)
13. [Utility Tools](#utility-tools)
14. [Cron Tools](#cron-tools)
15. [Code Intelligence Tools](#code-intelligence-tools)
16. [Advanced Tools](#advanced-tools)
17. [Tool Framework Internals](#tool-framework-internals)

---

## Tool System Overview

Every tool in Claurst implements a common `Tool` interface. This interface defines:

- **Identity** — name, aliases, MCP info
- **Input schema** — a Zod schema validating the input the model must provide
- **Capability flags** — `isReadOnly`, `isDestructive`, `isConcurrencySafe`
- **Permission check** — `checkPermissions()` called before execution
- **Execution** — `call()` performs the actual operation
- **UI rendering** — React/Ink components for TUI display

Tools are loaded eagerly at session start. The model receives tool descriptions and schemas and selects tools to call. Each tool call goes through permission resolution before `call()` is invoked.

### Tool Concurrency

Tools marked `isConcurrencySafe` may run in parallel with other tool calls. Most write tools are not concurrency-safe. Read-only tools are generally safe to parallelize.

---

## Permission System

### Permission Levels

Each tool is assigned a conceptual permission level based on what it can do:

| Level | Description | Examples |
|-------|-------------|---------|
| **None** | No external effects; purely passive | `SleepTool` |
| **ReadOnly** | Reads data; no writes or execution | `FileReadTool`, `GlobTool`, `WebFetchTool` |
| **Write** | Creates or modifies data | `FileWriteTool`, `FileEditTool`, `ConfigTool` |
| **Execute** | Runs code or spawns processes | `BashTool`, `TaskCreateTool`, `SendMessageTool` |
| **Dangerous** | Broad system access; high blast radius | `ComputerUseTool` |

### Permission Modes

The active permission mode controls how `checkPermissions()` behaves:

| Mode | Behavior |
|------|----------|
| `default` | Prompts the user for any tool that isn't pre-approved |
| `plan` | All write/execute tools are blocked; read-only tools run freely |
| `auto` | Non-destructive tools run without prompting; destructive tools prompt |
| `acceptEdits` | File edits are auto-approved; shell execution still prompts |
| `bypassPermissions` | All tools run without prompting (headless/CI use) |

### Interactive vs. Auto Mode

**Interactive mode** (default REPL): Claurst presents a confirmation prompt for any tool that lacks a pre-existing approval rule. The user can approve once, approve always (adding a permanent rule), or deny.

**Auto mode** (`--dangerously-skip-permissions` or `bypassPermissions`): No prompts are shown. All tool calls execute immediately. Use only in trusted, sandboxed environments.

### Permission Rules

Rules are stored per-project and per-user. A rule specifies:

- **Tool name** (or glob pattern matching tool names)
- **Path pattern** (optional, for file tools)
- **Decision**: `allow` or `deny`

Rules are evaluated in order; the first match wins. Manage rules with `/permissions`.

### Read-Before-Write Enforcement

File write tools check whether the file was read in the current session before allowing a write. This prevents overwriting files the model has not examined. The `readFileState` map in `ToolUseContext` tracks reads.

---

## File Tools

### FileReadTool

**Permission level:** ReadOnly

Read the contents of a file from the local filesystem. Returns file contents as a string. Supports optional line range to read a subset of a large file.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `file_path` | string | yes | Absolute path to the file |
| `offset` | integer | no | First line to read (1-indexed) |
| `limit` | integer | no | Maximum number of lines to read |

The tool tracks every read in `readFileState` with the file's modification time and content hash. Subsequent writes check this state.

Supports reading: text files, images (PNG, JPG, GIF, WEBP — returned as base64), PDF files (text extraction), and Jupyter notebooks.

---

### FileWriteTool

**Permission level:** Write

Write content to a file. Creates the file and any missing parent directories. Overwrites existing files entirely.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `file_path` | string | yes | Absolute path to write |
| `content` | string | yes | Full file content |

Requires the file to have been read first (or the file to not exist). The previous content is stored for `/undo` support.

---

### FileEditTool

**Permission level:** Write

Perform an exact string replacement within an existing file. Fails if `old_string` is not found or is not unique. Prefer this tool over `FileWriteTool` when making targeted edits, as it only transmits the diff rather than the entire file.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `file_path` | string | yes | Absolute path to the file |
| `old_string` | string | yes | Exact text to replace |
| `new_string` | string | yes | Replacement text |
| `replace_all` | boolean | no | Replace all occurrences (default: false) |

Whitespace and indentation must match exactly.

---

### BatchEditTool

**Permission level:** Write

Apply multiple `FileEditTool`-style edits in a single tool call. More efficient than calling `FileEditTool` repeatedly when making many changes to the same file or across multiple files.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `edits` | array | yes | Array of `{file_path, old_string, new_string, replace_all}` objects |

Edits within the same file are applied in order. If any individual edit fails (string not found, not unique), the batch is aborted and no changes are written.

---

### ApplyPatchTool

**Permission level:** Write

Apply a unified diff patch to one or more files. Accepts standard `diff -u` / `git diff` format patches.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `patch` | string | yes | Unified diff patch text |

Useful when the model needs to express changes in diff format rather than as string replacements.

---

## Shell Execution Tools

### BashTool

**Permission level:** Execute

Execute a shell command in a bash subprocess. The working directory persists between calls within a session. Shell state (variable assignments, `cd`, etc.) does not persist — each call starts from the configured working directory.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `command` | string | yes | Shell command to execute |
| `description` | string | no | Human-readable description shown in the TUI |
| `timeout` | integer | no | Timeout in milliseconds (max 600000) |
| `run_in_background` | boolean | no | Run asynchronously; result delivered via notification |

Output (stdout + stderr) is returned as a string. Commands that produce more than `maxResultSizeChars` of output are truncated.

Always quote file paths containing spaces. Use Unix shell syntax regardless of host OS.

When `run_in_background` is `true`, the task ID is returned immediately. Use `MonitorTool` to check status, retrieve output, or cancel the task.

---

### MonitorTool

**Permission level:** ReadOnly

Monitor background tasks started with `BashTool`'s `run_in_background=true`. Supports listing all tasks, checking the status or output of a specific task, and cancelling a running task.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | no | `list` (default), `status`, `output`, or `cancel` |
| `task_id` | string | no | Task ID to inspect or cancel. Required for `status`, `output`, `cancel` |

**Actions:**

| Action | Effect |
|--------|--------|
| `list` | Lists all background tasks with their IDs, status, and names |
| `status` | Shows the status and metadata for a specific task |
| `output` | Retrieves the stdout/stderr output collected so far |
| `cancel` | Sends a termination signal to a running task |

Task statuses: `running`, `completed`, `failed: <reason>`, `cancelled`.

---

### PtyBashTool

**Permission level:** Execute

Execute a command in a full pseudo-terminal (PTY). Required for interactive programs (editors, pagers, prompts) that need terminal capabilities such as ANSI codes, raw input, or window size detection. Behaves like `BashTool` for non-interactive commands.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `command` | string | yes | Command to run in PTY |
| `timeout` | integer | no | Timeout in milliseconds |

---

### PowerShellTool

**Permission level:** Execute

Execute a PowerShell command on Windows hosts. Equivalent to `BashTool` but uses `pwsh` (PowerShell Core) or `powershell.exe` as the shell.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `command` | string | yes | PowerShell command to execute |
| `timeout` | integer | no | Timeout in milliseconds |

Available only when running on Windows.

---

### ReplTool

**Permission level:** Execute

Maintain a persistent REPL session for a supported language (Python, Node.js, Ruby, etc.). State accumulates between calls — variables, imports, and definitions persist for the duration of the session.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `language` | string | yes | Language runtime (`python`, `node`, `ruby`, ...) |
| `code` | string | yes | Code to evaluate |

Useful for iterative data exploration or multi-step computations where re-running from scratch each time would be expensive.

---

## Search Tools

### GlobTool

**Permission level:** ReadOnly

Find files matching a glob pattern. Searches from a specified directory (defaults to the current working directory). Returns matching file paths sorted by modification time.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `pattern` | string | yes | Glob pattern (e.g., `**/*.rs`, `src/**/*.ts`) |
| `path` | string | no | Directory to search from |

---

### GrepTool

**Permission level:** ReadOnly

Search file contents using regular expressions, powered by ripgrep. Supports multiple output modes: matching lines with context, file paths only, or match counts.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `pattern` | string | yes | Regular expression pattern |
| `path` | string | no | Directory or file to search |
| `glob` | string | no | File glob filter (e.g., `*.rs`) |
| `type` | string | no | File type filter (e.g., `rust`, `py`, `js`) |
| `output_mode` | string | no | `content`, `files_with_matches`, or `count` |
| `-i` | boolean | no | Case-insensitive search |
| `-n` | boolean | no | Show line numbers |
| `context` | integer | no | Lines of context around each match |
| `multiline` | boolean | no | Enable multiline matching |
| `head_limit` | integer | no | Limit output lines (default 250) |

---

### ToolSearchTool

**Permission level:** ReadOnly

Search available tools by name or keyword to retrieve their full parameter schemas. Used internally by the model to discover deferred tools before calling them.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | yes | Tool name or keyword search |
| `max_results` | integer | no | Maximum results (default 5) |

---

## Web Tools

### WebFetchTool

**Permission level:** ReadOnly

Fetch the content of a URL. Returns the page content, typically converted to Markdown for readability. Supports HTML pages, plain text, JSON, and PDF documents.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `url` | string | yes | URL to fetch |
| `prompt` | string | no | Optional extraction prompt to focus content |

Network requests are subject to the host's firewall and proxy settings.

---

### WebSearchTool

**Permission level:** ReadOnly

Perform a web search and return a list of results with titles, URLs, and snippets. Requires a configured search API key (e.g., Brave Search).

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | yes | Search query |
| `num_results` | integer | no | Number of results to return |

---

## Task Management Tools

The task system allows the model to create and track long-running background work. There are two generations of the task API; V2 is preferred.

### TaskCreateTool (V2)

**Permission level:** Execute

Create a new background task. The task runs asynchronously; use `TaskGetTool` or `TaskOutputTool` to poll for completion.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `description` | string | yes | Human-readable task description |
| `command` | string | yes | Shell command or prompt to execute |
| `timeout` | integer | no | Maximum runtime in milliseconds |

Returns a `task_id` for use with other task tools.

---

### TaskGetTool (V2)

**Permission level:** ReadOnly

Get the current state of a task by ID.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `task_id` | string | yes | Task identifier |

Returns status (`pending`, `running`, `completed`, `failed`), progress, and partial output.

---

### TaskListTool (V2)

**Permission level:** ReadOnly

List all tasks in the current session with their statuses.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `filter` | string | no | Filter by status: `all`, `running`, `completed`, `failed` |

---

### TaskUpdateTool (V2)

**Permission level:** Execute

Update the parameters of a running or pending task.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `task_id` | string | yes | Task identifier |
| `description` | string | no | New description |

---

### TaskStopTool (V2)

**Permission level:** Execute

Terminate a running task.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `task_id` | string | yes | Task identifier to stop |

Sends SIGTERM to the task process. If it does not exit within a grace period, SIGKILL is sent.

---

### TaskOutputTool (V2)

**Permission level:** ReadOnly

Retrieve the accumulated stdout/stderr output from a task.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `task_id` | string | yes | Task identifier |
| `offset` | integer | no | Byte offset to read from (for streaming) |

---

### TodoWriteTool

**Permission level:** Write

Write or update the session TODO list. The TODO list is a structured set of tasks tracked across the session and displayed in the TUI sidebar.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | array | yes | Array of `{id, content, status, priority}` objects |

Status values: `pending`, `in_progress`, `completed`. Priority values: `low`, `medium`, `high`.

---

## MCP Integration Tools

Model Context Protocol (MCP) tools bridge Claurst to external MCP servers.

### ListMcpResourcesTool

**Permission level:** ReadOnly

List resources exposed by a connected MCP server.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `server_name` | string | yes | MCP server name as configured |

Returns a list of resource URIs with descriptions.

---

### ReadMcpResourceTool

**Permission level:** ReadOnly

Read the content of a specific resource from an MCP server.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `server_name` | string | yes | MCP server name |
| `uri` | string | yes | Resource URI to read |

---

### McpAuthTool

**Permission level:** Execute

Authenticate with an MCP server that requires credentials. Triggers the server's authentication flow and stores the resulting tokens.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `server_name` | string | yes | MCP server name |

---

## Agent Tools

Agent tools enable multi-agent coordination: spawning sub-agents, forming teams, and passing messages between them.

### SendMessageTool

**Permission level:** Execute

Send a message to another agent (sub-agent or coordinator). Used for inter-agent communication in multi-agent workflows.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `agent_id` | string | yes | Target agent identifier |
| `message` | string | yes | Message content |

---

### TeamCreateTool

**Permission level:** Execute

Create a team of sub-agents to work in parallel on a set of tasks. Each agent in the team receives its own context and toolset.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `team_name` | string | yes | Identifier for the team |
| `agents` | array | yes | Agent configuration objects |
| `coordination_strategy` | string | no | `parallel`, `sequential`, or `consensus` |

---

### TeamDeleteTool

**Permission level:** Execute

Dissolve a team and terminate all its member agents.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `team_name` | string | yes | Team to dissolve |

---

### RemoteTriggerTool

**Permission level:** Execute

Trigger a remote agent or scheduled workflow by name. Used by the cron/schedule system to fire agents at configured intervals.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `trigger_name` | string | yes | Named trigger to fire |
| `payload` | object | no | Optional input data for the trigger |

---

### SkillTool

**Permission level:** Execute

Invoke a named skill (bundled prompt-command) programmatically from within a tool call chain.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `skill` | string | yes | Skill name |
| `args` | string | no | Arguments to pass to the skill |

---

### GoalCompleteTool

**Permission level:** None

Mark the active goal as complete. This tool is surfaced to the model when a `/goal` is active. The model calls it only after performing a genuine completion audit — verifying the goal has been fully met rather than partially addressed.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `audit_summary` | string | yes | Concise summary of the goal-completion audit |
| `evidence` | string | yes | Specific evidence demonstrating the goal was achieved (files changed, tests passed, output produced, etc.) |

Calling this tool triggers the goal system to mark the goal as `Completed` and surfaces the audit results to the user. The model is expected to verify the goal thoroughly before calling — calling without genuine evidence is treated as an error.

See also: `/goal complete` command.

---

## Notebook Tools

### NotebookEditTool

**Permission level:** Write

Edit a Jupyter notebook (`.ipynb`) by modifying, inserting, or deleting cells. Operates on the notebook's JSON structure directly.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `notebook_path` | string | yes | Absolute path to the notebook |
| `cell_index` | integer | no | Cell to edit (0-indexed) |
| `new_source` | string | no | New cell source content |
| `cell_type` | string | no | `code`, `markdown`, or `raw` |
| `operation` | string | yes | `edit`, `insert`, or `delete` |

---

## Planning Tools

### EnterPlanModeTool

**Permission level:** Execute

Switch the agent into plan mode. In plan mode, all write and execute tools are blocked. The agent can only read files, search, and reason. Used to draft an approach before taking action.

No parameters.

Exits automatically when `/plan off` is invoked or when `ExitPlanModeTool` is called.

---

### ExitPlanModeTool

**Permission level:** Execute

Exit plan mode and return to the normal permission mode that was active before `/plan` was called.

No parameters.

---

## Worktree Tools

Worktree tools manage git worktrees, enabling the agent to work on multiple branches simultaneously in isolated directories.

### EnterWorktreeTool

**Permission level:** Execute

Create or attach to a git worktree for a given branch. Subsequent file operations run within the worktree's directory.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `branch` | string | yes | Branch name for the worktree |
| `path` | string | no | Directory for the worktree (auto-generated if omitted) |

---

### ExitWorktreeTool

**Permission level:** Execute

Detach from the current worktree and return to the main working directory.

No parameters.

---

## Utility Tools

### AskUserQuestionTool

**Permission level:** Execute

Pause execution and ask the user a question via an interactive prompt in the TUI. Returns the user's typed response. Use sparingly — prefer acting with best judgment and asking only when the choice is genuinely ambiguous.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `question` | string | yes | Question text to display |
| `options` | array | no | Multiple-choice options (renders as a menu) |

---

### BriefTool

**Permission level:** ReadOnly

Emit a short status message to the session output without triggering a full model response. Used in automated pipelines to surface progress updates.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `message` | string | yes | Status message to emit |

---

### SleepTool

**Permission level:** None

Pause execution for a specified duration. Useful in polling loops or when waiting for external processes.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `duration_ms` | integer | yes | Milliseconds to sleep |

Maximum sleep duration is 60000 ms (60 seconds) per call.

---

### ConfigTool

**Permission level:** Write

Read or write Claurst configuration values programmatically. Used by commands and tools that need to persist settings.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | yes | `get`, `set`, or `reset` |
| `key` | string | yes | Configuration key |
| `value` | any | no | Value to set (required for `set`) |

---

## Cron Tools

Cron tools manage scheduled agent triggers.

### CronCreateTool

**Permission level:** Write

Create a new scheduled trigger. The trigger fires at the specified cron schedule and executes the configured agent or command.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Trigger name (must be unique) |
| `schedule` | string | yes | Cron expression (e.g., `0 9 * * 1-5`) |
| `prompt` | string | yes | Agent prompt to run on schedule |
| `enabled` | boolean | no | Start enabled (default: true) |

---

### CronDeleteTool

**Permission level:** Write

Delete a scheduled trigger by name.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Trigger name to delete |

---

### CronListTool

**Permission level:** ReadOnly

List all scheduled triggers with their schedules and enabled states.

No parameters.

---

## Code Intelligence Tools

Code intelligence tools query language servers for semantic information about source code.

### LspTool

**Tool name:** `LSP`

**Permission level:** ReadOnly

Query a language server for code intelligence actions. Supports hover documentation, go-to-definition, find-references, document symbols, and diagnostics. Language servers must be configured in `settings.json` under the `lsp_servers` key.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | yes | `hover`, `definition`, `references`, `symbols`, or `diagnostics` |
| `file` | string | yes | Absolute or working-directory-relative path to the source file |
| `line` | integer | no | 1-based line number (required for `hover`, `definition`, `references`) |
| `column` | integer | no | 1-based column number (required for `hover`, `definition`, `references`) |

**Actions:**

| Action | Description |
|--------|-------------|
| `hover` | Returns documentation/type info for the symbol at `line`:`column` |
| `definition` | Returns the file and position where the symbol is defined |
| `references` | Lists all references to the symbol at `line`:`column` |
| `symbols` | Returns all symbols (functions, classes, variables) in the file |
| `diagnostics` | Returns LSP diagnostics (errors, warnings) for the file |

**Configuration** (`settings.json`):

```json
{
  "lsp_servers": [
    {
      "language": "rust",
      "command": "rust-analyzer",
      "args": []
    },
    {
      "language": "typescript",
      "command": "typescript-language-server",
      "args": ["--stdio"]
    }
  ]
}
```

If no LSP server is configured for a file's language, the tool returns an informative error. The tool resolves relative paths against the current working directory.

---

## Advanced Tools

### ComputerUseTool

**Permission level:** Dangerous

Control the desktop GUI — move the mouse, click, type, take screenshots, and interact with applications. Enables the agent to operate software that has no API or CLI interface.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | yes | `screenshot`, `click`, `type`, `key`, `move`, `scroll` |
| `coordinate` | array | no | `[x, y]` for mouse actions |
| `text` | string | no | Text to type |
| `key` | string | no | Key name for key events (e.g., `Return`, `ctrl+c`) |
| `duration` | integer | no | Hold duration in ms for mouse drags |

This tool has the highest blast radius of any tool in Claurst. It requires explicit permission and should only be enabled in controlled environments. All actions are logged in detail.

Requires a display server (X11, Wayland, or Windows Desktop). Not available in headless environments.

---

### StructuredOutput

**Tool name:** `StructuredOutput` (SyntheticOutputTool)

**Permission level:** None

Return structured JSON output as the agent's final response. This tool is surfaced only in non-interactive (SDK/headless) sessions and in hook handlers. The model must call it exactly once at the end of its response to deliver structured data to the caller.

The input schema is open — it accepts any JSON object. The specific expected schema is communicated via the system prompt for each session type.

**Example usage in a hook handler:**

```json
{
  "ok": true,
  "reason": "All tests passed."
}
```

**Example in an SDK session returning structured analysis:**

```json
{
  "summary": "Three security issues found.",
  "issues": [
    { "severity": "high", "description": "SQL injection in login handler" }
  ]
}
```

Calling this tool in an interactive session has no effect; the confirmation string is returned but the structured output is not surfaced to the TUI.

---

## Tool Framework Internals

### ToolUseContext

Every tool receives a `ToolUseContext` at call time. Key fields:

| Field | Type | Description |
|-------|------|-------------|
| `options` | object | Session options: loaded tools, commands, model config |
| `abortController` | AbortController | Abort signal; check `signal.aborted` in long-running operations |
| `getAppState()` | function | Read current TUI app state |
| `setAppState()` | function | Update TUI app state |
| `readFileState` | Map | Tracks file reads (path -> mtime + content) |
| `permissionContext` | object | Current permission mode and rules |
| `setToolJSX` | function | Inject a React/Ink component into the TUI |
| `onPermissionRequest()` | function | Callback to request a permission decision |
| `agentId` | string | Identifier of the calling agent (if sub-agent) |
| `isSubagent` | boolean | True when running as a sub-agent |

### ToolPermissionContext

| Field | Values | Description |
|-------|--------|-------------|
| `mode` | `default`, `plan`, `auto`, `acceptEdits`, `bypassPermissions` | Active permission mode |
| `allowedTools` | string[] | Explicitly pre-approved tool names |
| `deniedTools` | string[] | Explicitly blocked tool names |
| `rules` | PermissionRule[] | Path/tool pattern rules |

### PermissionDecision

The return value of `checkPermissions()`:

| Field | Type | Description |
|-------|------|-------------|
| `behavior` | `allow`, `deny`, `ask` | Resolved decision |
| `updatedInput` | object | Optionally-modified input (e.g., path normalization) |
| `reason` | string | Human-readable explanation for the decision |
