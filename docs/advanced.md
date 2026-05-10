# Advanced Features

This document covers Claurst's advanced capabilities beyond basic coding assistance.

---

## Extended thinking

Extended thinking gives the model additional computation budget to reason through hard problems before it responds.

### Commands

```
/thinking          Toggle extended thinking on or off for the session
/effort <level>    Set the effort level: low, medium, high, or max
```

### CLI flags

```
claurst --thinking <tokens>    Set a specific token budget for thinking
claurst --effort <level>       Set the effort level (low/medium/high/max)
```

### Effort levels

| Level | Description |
|---|---|
| `low` | Minimal thinking; fastest responses |
| `medium` | Moderate reasoning; balanced speed and quality |
| `high` | Deep reasoning; best quality for most tasks |
| `max` | Maximum available budget; reserved for Opus-class models |

The `max` level is only supported by models that expose it in the API (currently the Opus 4.6 generation). Attempting to set `max` on an unsupported model will fall back to `high`.

Effort levels `low`, `medium`, and `high` persist to `~/.claude.json` across sessions. The `max` level is session-scoped for regular users. Numeric budget values (raw token counts) are always session-scoped.

**Environment variable override:** `CLAUDE_CODE_EFFORT_LEVEL` overrides the persisted setting for the current process. If this variable is set and conflicts with a `/effort` command, Claurst displays a warning.

### How the API maps levels

The effort parameter is passed directly to the Anthropic API for supported models. When no effort parameter is sent (the default), the API uses `high`. The displayed status bar reflects the effective level as `resolveAppliedEffort` computes it: env override, then session state, then persisted setting.

---

## Auto-compaction

The context window has a finite size. Auto-compaction automatically summarises the conversation history when token usage approaches the limit, keeping the session alive without interruption.

### How it works

Claurst tracks token usage after every model turn. When usage crosses the auto-compact threshold — which is the effective context window size minus a 13,000-token buffer — it runs `compactConversation` to summarise the history and replaces the messages with a compact summary plus any trailing context.

The `PreCompact` hook fires before compaction (exit code 2 blocks it). The `PostCompact` hook fires after.

### Controlling auto-compaction

**Disable for a process:**

```bash
DISABLE_AUTO_COMPACT=1 claurst
```

This disables automatic compaction while keeping `/compact` available manually.

**Disable compaction entirely:**

```bash
DISABLE_COMPACT=1 claurst
```

**Override the threshold window (for testing):**

```bash
CLAUDE_AUTOCOMPACT_PCT_OVERRIDE=80 claurst
```

Sets the threshold to 80% of the effective context window instead of the default buffer-based calculation.

**Toggle in global config:**

`autoCompactEnabled` in `~/.claude.json` (boolean, default `true`). The `/compact` command respects this; auto-compact also checks it via `getGlobalConfig().autoCompactEnabled`.

### Manual compaction

```
/compact [custom instructions]
```

Runs compaction immediately. Optionally pass custom instructions to guide the summary (e.g. `/compact focus on the database schema changes`).

---

## Context window management

### /context

```
/context
```

Displays the current token usage relative to the model's context window. Shows the percentage remaining and warns when approaching the warning or error thresholds.

- **Warning threshold:** 20,000 tokens before the effective window limit.
- **Error threshold:** 20,000 tokens before the effective window limit (triggers a more prominent visual).
- **Blocking limit:** 3,000 tokens before the effective window — further input is blocked until compaction.

### ctx-viz

```
/ctx_viz
```

Opens an interactive visualisation of which parts of the context are consuming the most tokens. Useful for identifying large files or long tool outputs that could be trimmed.

---

## Session management

Sessions are stored as JSONL files under `~/.claude/projects/<sanitized-cwd>/<session-id>.jsonl`. Each line in the file is a JSON object representing a message or event in the conversation.

The transcript directory is derived from the working directory at session start. Worktrees and path sanitisation mean the per-project folder name is a normalised representation of the absolute path.

### Commands

| Command | Description |
|---|---|
| `/resume [id or search]` | Resume a previous session by ID or fuzzy search term. Alias: `/continue`. |
| `/session` | Show the remote session URL and QR code (available in remote mode). |
| `/fork` | Fork the current session into a new branch with fresh UUIDs, preserving the full message history. |
| `/rename <title>` | Rename the current session. Appends a custom-title entry to the JSONL file. |
| `/export` | Export the current session transcript. |
| `/rewind` | Step back to an earlier point in the conversation. |

### JSONL transcript format

Every message in the transcript is a newline-delimited JSON object. The key fields present on most entries:

```jsonl
{"uuid":"<uuid>","parentUuid":"<parent-uuid>","type":"user","message":{...},"timestamp":1234567890}
{"uuid":"<uuid>","parentUuid":"<parent-uuid>","type":"assistant","message":{...},"timestamp":1234567891}
```

The `parentUuid` field forms a linked chain that allows Claurst to reconstruct the conversation tree. `/fork` rewrites all UUIDs while preserving the chain structure.

Special entry types include `summary` (compaction summaries), `custom-title` (from `/rename`), and various ephemeral progress indicators that are filtered out when reading the transcript for display.

### SDK access

The public SDK exports `getSessionMessages`, `listSessions`, `getSessionInfo`, `renameSession`, `tagSession`, and `forkSession` for programmatic access to session data.

---

## Worktree support

Subagents spawned via the `Agent` tool can operate in isolated git worktrees to avoid interfering with the main working tree.

### Tools

- `EnterWorktreeTool` — checks out a new worktree and switches the agent's working directory to it.
- `ExitWorktreeTool` — removes the worktree and returns the agent to the original directory.

### Custom worktree backends

For non-git repositories or specialised isolation requirements, the `WorktreeCreate` and `WorktreeRemove` hook events let you substitute an external worktree manager:

- `WorktreeCreate` receives `{"name": "<slug>"}` and must write the absolute path of the created directory to stdout.
- `WorktreeRemove` receives `{"worktree_path": "<path>"}` and is responsible for cleanup.

This means worktrees can be Docker containers, virtual machines, or any directory-backed isolation primitive.

---

## Plan mode

Plan mode restricts the model to read-only operations, allowing it to research a codebase and propose a plan before making any changes.

### Entering plan mode

```
/plan [description]
claurst --permission-mode plan
```

When in plan mode:
- Write and execute operations require explicit permission.
- The model can read files, search, and reason freely.
- Exiting plan mode (via `ExitPlanModeTool`) returns to the normal permission model.

The `EnterPlanModeTool` and `ExitPlanModeTool` internal tools manage transitions. A `plan_mode_exit` attachment is injected into the conversation when the mode changes to guide the model's next steps.

---

## Goal system

The goal system lets Claurst work autonomously across multiple turns toward a single, verifiable objective. Instead of prompting repeatedly, you set the goal once and Claurst iterates until the goal is complete, paused, or the built-in runaway guard fires.

### Setting a goal

```
/goal <objective>
/goal --tokens 250K Refactor the auth module to use JWT tokens
```

Once a goal is set, Claurst begins working immediately. It continues across turns without waiting for user input until one of these conditions is met:

- The model calls `GoalCompleteTool` with an audit summary and evidence
- You run `/goal pause` or `/goal clear`
- The runaway guard fires (200-turn hard limit)
- A token budget is set and exhausted

### Monitoring and controlling goals

```
/goal                  — show current goal status
/goal status           — show current goal status
/goal pause            — pause the active goal (you can resume it later)
/goal resume           — resume a paused goal
/goal clear            — delete the current goal entirely
/goal complete         — manually request a completion audit
```

### How completion works

When the model believes the objective has been met, it calls `GoalCompleteTool` rather than simply responding. This tool requires two arguments:

- `audit_summary` — a concise description of what was accomplished
- `evidence` — specific, verifiable evidence (files changed, tests passing, output produced)

Claurst displays both to the user before marking the goal complete. The model is expected to genuinely audit the outcome before calling; calling without real evidence is rejected.

### Goal status lifecycle

| Status | Meaning |
|--------|---------|
| `Active` | Goal is set and work is ongoing |
| `Paused` | Work paused by user; goal is preserved |
| `Completed` | Model called `GoalCompleteTool` with accepted audit |
| `Failed` | Runaway guard fired or budget exhausted |

### Disabling the goal system

```bash
CLAURST_GOALS=0 claurst
```

Set `CLAURST_GOALS=0` in the environment to completely disable goal-related commands and the `GoalCompleteTool`. Useful in environments where autonomous multi-turn execution is undesirable.

---

## Managed agents

Managed agents enable a **manager-executor** architecture where a manager model delegates subtasks to one or more executor agents running in parallel. The manager reasons about the high-level plan; executors carry out individual tasks.

### Enabling managed agents

```
/managed-agents enable
```

Or apply a built-in preset:

```
/managed-agents presets               — list available presets
/managed-agents preset <name>         — apply a preset (configures all parameters)
```

### Architecture

```
User → Manager model
         ├─ Executor 1 (e.g., implementing feature A)
         ├─ Executor 2 (e.g., writing tests for A)
         └─ Executor 3 (e.g., updating docs)
```

The manager model does not execute tools itself — it delegates to executor agents and synthesizes their outputs. Executors can run concurrently up to the `concurrent` limit.

### Configuration

```
/managed-agents configure manager-model  anthropic/claude-opus-4-6
/managed-agents configure executor-model anthropic/claude-sonnet-4-6
/managed-agents configure executor-turns 20
/managed-agents configure concurrent     3
/managed-agents configure isolation      on
```

Model format: `provider/model` (e.g., `anthropic/claude-opus-4-6`, `openai/gpt-4o`).

### Budget split policies

Control how the total token/cost budget is divided between the manager and executors:

| Policy | Command | Description |
|--------|---------|-------------|
| `shared` | `budget-split shared` | All agents draw from a single shared pool |
| `percentage` | `budget-split percentage:20` | Manager gets 20%, executors share the rest |
| `fixed` | `budget-split fixed:0.50:2.00` | Manager capped at $0.50, each executor at $2.00 |

```
/managed-agents budget 5.00           — set a total $5 budget (0 to clear)
```

### Viewing configuration

```
/managed-agents status
```

Configuration persists to `~/.claurst/settings.json` under `managed_agents`.

> **Preview feature.** The managed-agents API is under active development and may change in future releases.

---

## Speech modes

Speech modes change how the model structures its responses, trading naturalness for brevity. Useful in long sessions where verbose responses consume too many tokens or take too long to read.

### Caveman mode

Strips pleasantries, hedging phrases, transitional sentences, and articles. Output is dense and telegraphic.

```
/caveman             — full caveman mode (~75% token reduction)
/caveman lite        — remove pleasantries only (~40% reduction)
/caveman full        — compress sentences, drop articles (~75% reduction, default)
/caveman ultra       — maximum compression, imperative only (~85% reduction)
```

**Example (normal mode):**
> "I'd be happy to help with that. Let me take a look at the file and see what changes might be appropriate here."

**Example (caveman full):**
> "Look at file. Make changes."

### Rocky mode

The model adopts the communication style of Rocky from *Project Hail Mary* — an Eridian alien engineer with a distinctive grammar, heavy emphasis, and alien perspective on human tasks.

```
/rocky             — full Rocky mode (~75% token reduction)
/rocky lite        — grammar rules only, minimal emphasis (~40% reduction)
/rocky full        — full grammar + regular emphasis (~75% reduction, default)
/rocky ultra       — maximum personality, frequent emphasis, alien observations
```

**Example (rocky full):**
> "Read file. *Yes!* Change function. Return value wrong! Fix now. *Eridians not make this mistake.*"

### Deactivating speech modes

```
/normal
```

Resets to the model's standard response style. Any active mode (caveman or rocky) is immediately deactivated.

---

## Headless mode

Headless mode runs Claurst non-interactively, suitable for scripts, CI pipelines, and programmatic orchestration.

### --print flag

```bash
claurst --print "refactor this function to use async/await"
claurst -p "summarise the changes in this PR"
```

Processes the prompt and exits after printing the final response to stdout. No interactive UI is shown.

Input can also be piped via stdin:

```bash
cat my_prompt.txt | claurst --print
echo "explain this code" | claurst -p
```

### --output-format

```bash
claurst --print --output-format json "..."
claurst --print --output-format stream-json --verbose "..."
```

| Format | Description |
|---|---|
| (default) | Plain text output — only the final assistant message. |
| `json` | Full message array as JSON (requires `--verbose`). |
| `stream-json` | Newline-delimited JSON stream of messages as they arrive (requires `--verbose`). |

`stream-json` is the format used by the Agent SDK transport. It emits every message event as it arrives, making it suitable for real-time processing pipelines.

---

## Budget control

Limit resource consumption per invocation using CLI flags:

```bash
claurst --max-budget-usd 2.00 "..."   # Stop after spending $2.00
claurst --max-turns 10 "..."          # Stop after 10 model turns
claurst --max-tokens 50000 "..."      # Stop after 50,000 output tokens
```

When a limit is reached, Claurst exits with a corresponding error message:
- `Error: Reached max turns (<n>)`
- `Error: Exceeded USD budget (<amount>)`

These flags are intended for automated use where runaway sessions would be costly.

---

## The Buddy companion system

Every Claurst user gets a persistent companion derived deterministically from their user ID. The companion appears as a small sprite in the terminal UI and occasionally comments on activity.

### How companions are generated

The companion's visual traits (species, eyes, hat, rarity, shiny status, stats) are generated by hashing the user ID with a seeded PRNG (Mulberry32). This means the companion is always the same for a given user — it cannot be faked by editing config files, because the bones are regenerated from the hash on every read.

Only the soul (name, personality) is persisted to `~/.claude.json` under the `companion` key, and only after it has been "hatched" (named by the model on first encounter).

### Species

18 species are available: duck, goose, blob, cat, dragon, octopus, owl, penguin, turtle, snail, ghost, axolotl, capybara, cactus, robot, rabbit, mushroom, chonk.

### Rarity tiers

| Rarity | Weight | Stars |
|---|---|---|
| common | 60% | ★ |
| uncommon | 25% | ★★ |
| rare | 10% | ★★★ |
| epic | 4% | ★★★★ |
| legendary | 1% | ★★★★★ |

Rarity affects the floor value of the companion's stats. A legendary companion has a minimum stat floor of 50, while a common companion starts at 5.

### Stats

Each companion has five stats: DEBUGGING, PATIENCE, CHAOS, WISDOM, SNARK. One stat is the peak (higher rolls), one is the dump stat (lower rolls), and the rest are scattered around the rarity floor.

### Persistence

The stored companion format in `~/.claude.json`:

```json
{
  "companion": {
    "name": "Vortox",
    "personality": "a chaotic little axolotl who celebrates every bug as a feature",
    "hatchedAt": 1712345678901
  }
}
```

The bones (species, rarity, stats, eyes, hat, shiny) are never stored and are always recomputed from `hash(userId)`.

---

## Voice mode

```
/voice
```

Experimental voice input using the device microphone. When active, spoken input is transcribed and submitted as a prompt. The integration uses the Deepgram streaming STT API.

The `/voice` command is a toggle. `CLAUDE_CODE_ENABLE_VOICE=1` can be used to pre-enable voice mode.

---

## Vim keybindings

```
/vim
```

Toggles vim-style modal keybindings for the input buffer. When enabled, the prompt input operates in normal/insert/visual modes, allowing navigation and editing with standard vim motions.

The setting persists to user settings. The `--vim` CLI flag enables vim mode for the session without persisting.

---

## Bridge and remote sessions

Claurst can be controlled remotely through a web interface at claude.ai. This "bridge" mode keeps a WebSocket connection open that allows a remote UI to send prompts and receive streaming responses.

```
/session
```

Shows the current remote session URL and a QR code for scanning on mobile.

The bridge operates in two topologies:
- **In-process bridge** — the WebSocket lives inside the Claurst process. If the process dies, the connection is lost.
- **Daemon bridge** — the WebSocket lives in a parent daemon process. The agent can be respawned while the claude.ai session stays connected. This is the `connectRemoteControl` SDK primitive.

SSH sessions work similarly: `claurst --ssh` enables a remote-accessible session that can be connected to from another machine.

---

## AGENTS.md hierarchical memory

Claurst reads instruction files from the filesystem before every session and whenever a relevant file changes. The lookup order is:

1. **Managed** — `/etc/claude-code/CLAUDE.md` (administrator-controlled, always loaded).
2. **User** — `~/.claude/CLAUDE.md` and `~/.claude/rules/*.md` (personal global instructions).
3. **Project** — `CLAUDE.md`, `.claude/CLAUDE.md`, and `.claude/rules/*.md` in each directory from the filesystem root down to the current working directory. Files are loaded from the root toward the CWD so that parent-directory rules are visible when processing child-directory rules.

`AGENTS.md` is treated equivalently to `CLAUDE.md` for compatibility with other AI coding tools.

Files can `@include` other files using a frontmatter include directive. Included files are resolved relative to the including file.

The `InstructionsLoaded` hook event fires for every file that is loaded, with the `load_reason` field indicating why (e.g. `session_start`, `nested_traversal`, `path_glob_match`, `include`, `compact`).

The `/memory` command opens the memory management UI for viewing, editing, and organising instruction files.

---

## Security and permissions

### Permission modes

| Mode | Description |
|---|---|
| `default` | Prompts the user before executing dangerous or write operations. |
| `plan` | Read-only; write and execute require explicit approval. |
| `autoAccept` | Accepts all tool calls without prompting. Use with caution. |
| `bypassPermissions` | Skips the permission system entirely. Intended for trusted automation only. |

The active mode is set with `--permission-mode <mode>` or via the `PermissionRequest` hook.

### Tool risk classification

Every tool is classified into a risk tier that determines the default permission behaviour:

| Tier | Examples | Default behaviour |
|---|---|---|
| `forbidden` | Directly destructive operations | Always blocked |
| `dangerous` | Broad filesystem writes, network access | Prompt required |
| `execute` | Bash, shell commands | Prompt required |
| `write` | FileWrite, FileEdit, TodoWrite | Prompt in default mode |
| `readonly` | FileRead, Glob, Grep, WebFetch | Allowed automatically |

### Bash command risk classification

Within `BashTool`, commands are further classified by analysing the command string against known patterns. Commands matching dangerous patterns (e.g. `rm -rf`, `dd if=`, pipe chains with destructive intent) receive a higher risk rating and may be blocked depending on the active permission mode.

The `PermissionRequest` hook can intercept any tool call before the user prompt is displayed, allowing automated allow/deny decisions based on context.

---

## Output styles

```
/output-style [style]
```

Controls how Claurst formats its responses. Available styles vary by configuration; the command opens an interactive picker when called without arguments.

Output styles affect markdown rendering, code block formatting, and verbosity of tool call summaries in the terminal UI.

---

## Custom commands

Custom slash commands can be defined in `.claude/settings.json` under a `customCommands` key. A custom command is a template that expands to a prompt when invoked.

```json
{
  "customCommands": [
    {
      "name": "review",
      "description": "Review the staged git diff",
      "command": "Review the output of `git diff --staged`. Focus on correctness, edge cases, and naming."
    },
    {
      "name": "standup",
      "description": "Summarise today's work",
      "command": "Summarise what I worked on today based on the git log since midnight."
    }
  ]
}
```

Custom commands appear alongside built-in commands in the `/` menu.

---

## Formatters

Auto-formatters run automatically after file writes when configured. The typical setup uses a `PostToolUse` hook on `FileWrite` or `FileEdit`:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "FileWrite",
        "hooks": [
          {
            "type": "command",
            "command": "bash -c 'FILE=$(jq -r .inputs.file_path); prettier --write \"$FILE\" 2>/dev/null || true'"
          }
        ]
      }
    ]
  }
}
```

Common formatters:
- **Prettier** (`prettier --write <file>`) — TypeScript, JavaScript, JSON, CSS, Markdown.
- **Ruff** (`ruff format <file>`) — Python.
- **rustfmt** (`rustfmt <file>`) — Rust.
- **gofmt** (`gofmt -w <file>`) — Go.
- **clang-format** (`clang-format -i <file>`) — C/C++.

The hook exit code does not affect the tool call result; formatters should suppress non-zero exits for unknown file types (`|| true`).

---

## Environment management

### --add-dir

```bash
claurst --add-dir /path/to/additional/project "..."
```

Grants Claurst read access to an additional directory outside the working directory. Useful when a task spans multiple repositories or when config files live outside the project root.

Multiple `--add-dir` flags can be combined.

### Environment variables in config

Environment variables can be set in `.claude/settings.json` under an `env` key. These are injected into tool executions:

```json
{
  "env": {
    "NODE_ENV": "development",
    "DATABASE_URL": "postgres://localhost/mydb"
  }
}
```

The `CwdChanged` hook can also write environment exports to `$CLAUDE_ENV_FILE` to propagate dynamic values into subsequent Bash commands within the same session.

---

## LSP integration

The `LspTool` provides code intelligence by communicating with a Language Server Protocol server for the current file.

### Operations

| Operation | Description |
|---|---|
| `hover` | Get type information and documentation for the symbol at a position |
| `definition` | Find where a symbol is defined |
| `references` | Find all references to a symbol |
| `symbols` | List all symbols (functions, classes, variables) in the file |
| `diagnostics` | Get errors and warnings reported by the language server |

### Input schema

```json
{
  "action": "hover",
  "file": "src/main.rs",
  "line": 42,
  "column": 15
}
```

- `action` — required, one of the operations above
- `file` — required, absolute or working-directory-relative path
- `line` — 1-based line number (required for `hover`, `definition`, `references`)
- `column` — 1-based column number (required for `hover`, `definition`, `references`)

The `symbols` and `diagnostics` actions only require `action` and `file`.

### Configuration

LSP servers must be configured in `settings.json`:

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

If no LSP server is configured for the file's language, the tool returns an informative error. Path resolution is relative to the current working directory.
