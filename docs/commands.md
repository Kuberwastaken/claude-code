# Claurst Slash Commands Reference

This document is the complete reference for every slash command available in Claurst, the Rust reimplementation of Claude Code CLI. Commands are invoked by typing `/command-name` at the REPL prompt.

---

## Table of Contents

1. [Command System Overview](#command-system-overview)
2. [Session & Navigation](#session--navigation)
3. [Model & Provider](#model--provider) — `/model`, `/providers`, `/connect`, `/thinking`, `/effort`, `/advisor`, `/fast`
4. [Configuration & Settings](#configuration--settings) — `/config`, `/keybindings`, `/permissions`, `/hooks`, `/privacy-settings`, `/mcp`, `/output-style`, `/theme`, `/statusline`, `/vim`, `/voice`, `/terminal-setup`
5. [Code & Git](#code--git) — `/commit`, `/diff`, `/undo`, `/review`, `/security-review`, `/init`, `/search`
6. [Search & Files](#search--files) — `/files`, `/context`
7. [Memory & Context](#memory--context) — `/memory`, `/usage`, `/cost`, `/stats`, `/status`, `/insights`
8. [Agents & Tasks](#agents--tasks) — `/agents`, `/tasks`, `/goal`, `/managed-agents`, `/agent`
9. [Planning & Review](#planning--review) — `/plan`, `/ultraplan`, `/ultrareview`
10. [MCP & Integrations](#mcp--integrations) — `/mcp`, `/skills`, `/plugin`, `/chrome`
11. [Authentication](#authentication) — `/login`, `/logout`, `/refresh`
12. [Display & Terminal](#display--terminal) — `/theme`, `/output-style`, `/statusline`, `/vim`, `/terminal-setup`, `/caveman`, `/rocky`, `/normal`, `/mobile`, `/color`, `/stickers`
13. [Diagnostics & Info](#diagnostics--info) — `/doctor`, `/version`, `/update`
14. [Export & Sharing](#export--sharing) — `/export`, `/copy`
15. [Advanced & Internal](#advanced--internal) — `/thinking`, `/connect`, `/fork`, `/effort`, `/summary`, `/brief`, `/sandbox-toggle`, `/think-back`, `/thinkback-play`
16. [Command Availability](#command-availability)

---

## Command System Overview

Commands are registered in a priority-ordered registry. When you type a command name, Claurst resolves it through this chain:

```
bundledSkills -> builtinPluginSkills -> skillDirCommands ->
workflowCommands -> pluginCommands -> pluginSkills -> COMMANDS()
```

### Command Types

| Type | Behavior |
|------|----------|
| `local` | Runs synchronously; returns text output directly |
| `local-jsx` | Renders an interactive TUI component (model picker, theme selector, etc.) |
| `prompt` | Expands to a prompt sent to the model via the main inference loop |

Commands support aliases — for example `/h`, `/?`, and `/help` all invoke the same handler.

### Usage Syntax

```
/command-name [arguments]
```

Arguments are passed as a single string after the command name. Most commands that accept arguments are documented with an `argumentHint` shown in the command palette.

---

## Session & Navigation

### /help
**Aliases:** `h`, `?`

Display all available commands with their descriptions. Respects `isHidden` flags — internal or rarely-needed commands are suppressed unless you are an Anthropic employee.

```
/help
/h
/?
```

---

### /clear
**Aliases:** `reset`, `new`

Clear the current conversation history and start a fresh session. The session file is retained on disk; only the in-memory message list is cleared.

```
/clear
```

---

### /exit
**Aliases:** `quit`

Exit the Claurst REPL. Equivalent to pressing `Ctrl+D`. Unsaved session state is flushed before exit.

```
/exit
/quit
```

---

### /resume
**Aliases:** `continue`

Resume a previous session from the session store. Displays a list of recent sessions with timestamps and summaries. Select one to restore its message history and file state.

```
/resume
/resume <session-id>
```

---

### /session
**Aliases:** `remote`

Manage active and stored sessions. Subcommands allow listing, switching, deleting, and attaching to remote sessions.

```
/session
/session list
/session delete <session-id>
/session attach <session-id>
```

---

### /fork

Fork the current session into a new independent session that begins from the current conversation state. Useful for exploring two different approaches without losing either.

```
/fork
/fork <new-session-name>
```

---

### /rename

Rename the current session. The new name is used in session listings and exports.

```
/rename <new-name>
```

---

### /rewind
**Aliases:** `checkpoint`

Rewind the conversation to a previous message. Displays a numbered list of messages; enter a number to truncate history to that point and resume from there.

```
/rewind
/rewind <message-index>
```

---

### /compact

Summarize and compress the conversation history to reduce context window usage. The model is asked to produce a dense summary of the prior exchange; that summary replaces the raw messages.

```
/compact
```

---

## Model & Provider

### /model

Open the interactive model picker. Displays a searchable list of available models from all configured providers. The selected model is used for all subsequent inference in the current session.

```
/model
/model claude-opus-4-5
/model claude-sonnet-4-6
```

---

### /providers

List all configured AI providers and their connection status. Shows provider name, base URL, and whether credentials are present.

```
/providers
```

---

### /connect

Connect to a remote AI provider or configure a custom provider endpoint. Supports OpenAI-compatible APIs, Anthropic direct, and others.

```
/connect
/connect <provider-name>
/connect openai https://api.openai.com/v1
```

---

### /thinking

Configure extended thinking for the current session. Extended thinking allows the model to reason through problems before responding, at the cost of additional tokens.

```
/thinking
/thinking on
/thinking off
```

See also `/effort` for a higher-level interface to thinking depth.

---

### /effort

Set the thinking effort level. This is a convenience wrapper over `/thinking` that maps human-readable levels to token budgets.

| Level | Description |
|-------|-------------|
| `low` | Minimal thinking; fastest responses |
| `medium` | Balanced thinking and speed |
| `high` | Deep reasoning; slower responses |
| `max` | Maximum token budget for thinking |

```
/effort low
/effort medium
/effort high
/effort max
```

---

### /advisor

Set or unset a secondary advisor model that provides supplementary suggestions alongside the main model. When set, the advisor model's context is available to improve main-model responses.

```
/advisor                          — show current advisor setting
/advisor claude-opus-4-6          — set advisor model by name
/advisor provider/model           — set advisor using provider/model format
/advisor off                      — disable the advisor
/advisor unset                    — disable the advisor
```

The advisor model persists to `~/.claurst/settings.json` under `advisorModel`. Model IDs must start with `claude-` or contain a `/` (provider/model format).

---

### /fast
**Aliases:** `speed`

Toggle fast mode. In fast mode, Claurst switches to the active provider's smaller, faster model for quick responses. Useful when you want rapid answers and deep reasoning is not required.

```
/fast          — toggle fast mode on/off
/fast on       — enable fast mode
/fast off      — disable fast mode
```

Setting persists to `~/.claurst/ui-settings.json`.

---

## Configuration & Settings

### /config
**Aliases:** `settings`

View or modify Claurst configuration values. Without arguments, renders an interactive settings panel. With arguments, acts as a key-value accessor.

```
/config
/config get <key>
/config set <key> <value>
/config reset <key>
```

Common keys:

| Key | Description |
|-----|-------------|
| `model` | Default model name |
| `theme` | Color theme name |
| `vim` | Vim mode enabled (`true`/`false`) |
| `outputStyle` | Output rendering style |
| `autoApprove` | Auto-approve tool calls |

---

### /keybindings

Open the interactive keybinding configurator. Displays all bound actions with their current shortcuts. Select an action to rebind it. Changes are written to `~/.claude/keybindings.json`.

```
/keybindings
```

See [keybindings.md](./keybindings.md) for the full keybindings reference.

---

### /permissions
**Aliases:** `allowed-tools`

View and manage tool permission rules. Permissions control which tools can run without prompting, which are blocked, and which always require confirmation.

```
/permissions
/permissions list
/permissions allow <tool-name>
/permissions deny <tool-name>
/permissions reset
```

---

### /hooks

Manage event hooks. Hooks are shell commands or scripts that execute when lifecycle events fire (e.g., before/after tool calls, on session start/end).

```
/hooks
/hooks list
/hooks add <event> <command>
/hooks remove <hook-id>
```

Available events: `pre-tool`, `post-tool`, `session-start`, `session-end`, `message-send`, `message-receive`.

---

### /privacy-settings

Open Claurst privacy settings. Launches a browser to the Anthropic privacy portal where you can review data usage preferences, conversation retention, and account privacy options.

```
/privacy-settings
```

---

### /mcp

Configure and manage Model Context Protocol (MCP) servers. MCP servers expose additional tools and resources to the agent.

```
/mcp
/mcp list
/mcp add <name> <command>
/mcp remove <name>
/mcp restart <name>
```

---

### /output-style

Select how the model's output is rendered in the terminal. Choices include `auto`, `plain`, `markdown`, `streaming`, and others depending on terminal capabilities.

```
/output-style
/output-style plain
/output-style markdown
```

---

### /theme

Open the interactive theme picker. Preview and select a color theme for the Claurst TUI.

```
/theme
/theme dark
/theme light
/theme solarized
```

---

### /statusline

Configure the status line displayed at the bottom of the TUI. Toggle individual elements such as model name, token count, session name, and git branch.

```
/statusline
/statusline toggle model
/statusline toggle tokens
```

---

### /vim

Toggle vim keybinding mode on or off. In vim mode the input field behaves like a vim editor (normal/insert/visual modes). Persisted to config.

```
/vim
/vim on
/vim off
```

---

### /voice

Configure voice input/output. Requires a supported audio backend. Subcommands control microphone selection, TTS voice, and push-to-talk behavior.

```
/voice
/voice on
/voice off
/voice mic <device>
/voice tts <voice-name>
```

---

### /terminal-setup

Run the terminal capability detection and setup wizard. Checks for true-color support, font ligatures, Unicode rendering, and configures Claurst accordingly.

```
/terminal-setup
```

---

## Code & Git

### /commit

Stage and commit changes to the current git repository. The model drafts a commit message based on the diff. You can review and edit the message before confirming.

```
/commit
/commit "optional message override"
```

---

### /diff

Show file diffs for changes made during the current session. Displays a unified diff of all files Claurst has written or edited since the session started.

```
/diff
/diff <file-path>
```

---

### /undo

Undo file changes made during the current session. Restores files to their state before Claurst's last write operation. Can be called multiple times to step further back.

```
/undo
/undo <file-path>
```

---

### /review

Initiate a code review pass over recent changes. The model examines all modified files and produces inline comments and a summary of issues found.

```
/review
/review <file-path>
/review --since HEAD~3
```

---

### /security-review

Run a security-focused review pass. The model looks specifically for vulnerabilities, credential exposure, injection risks, and other security concerns in modified files.

```
/security-review
/security-review <file-path>
```

---

### /init

Initialize Claurst project configuration in the current directory. Creates a `CLAUDE.md` file that acts as persistent project-level context injected at the start of every session.

```
/init
```

---

### /search

Search the codebase using natural language or regex patterns. Wraps the GrepTool and GlobTool with a higher-level interface.

```
/search <query>
/search "TODO" --type ts
/search "function.*export" --regex
```

---

## Search & Files

### /files

List all files currently tracked (read or written) in the active session. Useful for reviewing what context the model has access to.

```
/files
/files --written
/files --read
```

---

### /context

Analyze context window usage. Shows a breakdown of tokens consumed by system prompt, conversation history, file contents, and tool results. Helps identify what to compact or drop.

```
/context
```

---

## Memory & Context

### /memory

Manage session memory. Memory entries are short notes persisted across sessions. The model can read these at session start to maintain continuity.

```
/memory
/memory list
/memory add <note>
/memory delete <id>
/memory clear
```

---

### /usage

Display a detailed token usage breakdown for the current session. Shows input tokens, output tokens, cache reads, cache writes, and estimated cost per API call.

```
/usage
```

---

### /cost

Show the total token usage and estimated cost for the current session. Provides a quick summary without the per-call breakdown of `/usage`.

```
/cost
```

---

### /stats

Display session statistics: number of messages, tool calls, files modified, tokens used, session duration, and model used.

```
/stats
```

---

### /status

Show the current session status. Includes active model, permission mode, thinking config, connected MCP servers, and loaded plugins.

```
/status
```

---

### /insights

Generate an analytical report of the current session. Prints a structured breakdown of conversation statistics including turn count, token usage (input/output/total), average tokens per exchange, estimated cost, total tool calls, and the most frequently invoked tool.

```
/insights
```

Sample output:
```
Session Insights
──────────────────────────────────────
Conversation
├─ User turns          : 12
├─ Assistant turns     : 12
└─ Completed exchanges : 12

Tokens
├─ Input               : 48320
├─ Output              : 9140
├─ Total               : 57460
└─ Avg per exchange    : 4788

Cost
└─ Estimated USD       : $0.1823

Tools
├─ Total calls         : 34
└─ Most used           : Bash (18 calls)
```

---

## Agents & Tasks

### /agents

Manage sub-agents. Sub-agents are parallel model instances that can be spawned to work on independent tasks simultaneously.

```
/agents
/agents list
/agents stop <agent-id>
/agents output <agent-id>
```

---

### /tasks
**Aliases:** `bashes`

Manage tracked background tasks. Tasks are shell commands or model invocations running asynchronously. Monitor progress, fetch output, or stop tasks from this interface.

```
/tasks
/tasks list
/tasks output <task-id>
/tasks stop <task-id>
```

---

### /goal

Set a durable multi-turn autonomous goal. When a goal is active, Claurst continues working across turns until the goal is marked complete, paused, or a 200-turn runaway guard fires. Designed for complex, sustained tasks that would otherwise require repeated manual re-prompting.

```
/goal <objective>                    — set a new goal and begin working autonomously
/goal --tokens 250K <objective>      — set a goal with a soft token budget cap
/goal                                — show current goal status
/goal status                         — show current goal status
/goal pause                          — pause the active goal
/goal resume                         — resume a paused goal
/goal clear                          — delete the current goal
/goal complete                       — request a completion audit
```

When the model believes the goal has been achieved, it calls the `GoalComplete` tool with an audit summary and evidence. Goals can be disabled globally by setting `CLAURST_GOALS=0` in your environment.

See [Goal System](./advanced.md#goal-system) in the advanced guide.

---

### /managed-agents

Configure the manager-executor agent architecture, where a manager model delegates subtasks to one or more executor agents working in parallel. Includes budget controls and isolation options.

```
/managed-agents                                       — show current configuration
/managed-agents status                                — show current configuration
/managed-agents presets                               — list built-in presets
/managed-agents preset <name>                         — apply a named preset
/managed-agents setup                                 — show setup instructions
/managed-agents enable                                — enable managed agents
/managed-agents disable                               — disable managed agents
/managed-agents reset                                 — remove all managed-agent configuration
/managed-agents configure manager-model <model>       — set the manager model
/managed-agents configure executor-model <model>      — set the executor model
/managed-agents configure executor-turns <n>          — set executor max turns
/managed-agents configure concurrent <n>              — set max concurrent executors
/managed-agents configure isolation on|off            — toggle executor isolation
/managed-agents configure budget-split shared         — shared token pool
/managed-agents configure budget-split percentage:<n> — percentage split (manager gets n%)
/managed-agents configure budget-split fixed:<m>:<e>  — fixed USD caps (manager / executor)
/managed-agents budget <amount>                       — set total budget in USD (0 to clear)
```

Model format: `provider/model` (e.g., `anthropic/claude-opus-4-6`, `openai/gpt-4o`). Configuration persists to `~/.claurst/settings.json` under `managed_agents`.

> **Preview feature.** Behaviour may change across releases.

See [Managed Agents](./advanced.md#managed-agents) in the advanced guide.

---

### /agent

List all available named agents, or show details for a specific agent. Named agents are predefined configurations with their own system prompts, model bindings, and access levels. Useful for discovering what agents are available before starting a session.

```
/agent             — list all visible named agents with access levels
/agent <name>      — show full details for a specific named agent
```

To activate an agent, start Claurst with `--agent <name>`. See [agents.md](./agents.md) for defining custom agents.

---

## Planning & Review

### /plan

Enter plan mode (read-only). In plan mode the model can read files and reason about changes but cannot write, edit, or execute anything. Use this to draft an approach before allowing writes.

```
/plan
```

To exit plan mode, use `/plan off` or the `/exit-plan` internal action.

---

### /ultraplan

Extended planning mode with deeper reasoning. Like `/plan` but with an elevated thinking budget to allow more thorough analysis before acting.

```
/ultraplan
```

---

### /ultrareview

Run an exhaustive multi-dimensional code review over the current working directory or a specified path. Goes significantly beyond `/review` and `/security-review`, covering:

- **Security** — OWASP Top 10, injection vulnerabilities, cryptographic weaknesses, path traversal, race conditions, dependency risks
- **Performance** — algorithmic complexity, allocations, N+1 queries, blocking I/O, memory leaks
- **Maintainability** — function length, nesting depth, DRY violations, naming, dead code
- **Error handling** — swallowed errors, panic paths, missing input validation
- **Test coverage** — missing tests, brittle tests, missing edge cases
- **API design, documentation, accessibility, and architecture**

Each finding is tagged by category and severity.

```
/ultrareview
/ultrareview <path>
/ultrareview <PR-number>
```

---

## MCP & Integrations

### /mcp

Documented above under [Configuration & Settings](#configuration--settings).

---

### /skills

List and manage skills. Skills are bundled prompt-commands that extend Claurst's capabilities without writing code. They appear alongside built-in commands in the registry.

```
/skills
/skills list
/skills enable <skill-name>
/skills disable <skill-name>
/skills reload
```

---

### /plugin
**Aliases:** `plugins`, `marketplace`

Manage plugins. Plugins are loadable modules that can register new commands, tools, and hooks. Browse the marketplace or install from a local path.

```
/plugin
/plugin list
/plugin install <name>
/plugin install <path>
/plugin remove <name>
/plugin reload
```

---

### /chrome

Browser automation via Chrome DevTools Protocol (CDP). Connects to a running Chrome or Chromium instance and lets Claurst control it — navigate pages, click elements, fill forms, evaluate JavaScript, and take screenshots.

First, launch Chrome with remote debugging enabled:

```bash
chrome --remote-debugging-port=9222 --no-first-run
```

Then:

```
/chrome connect [--port 9222]      — connect to Chrome on the given port (default: 9222)
/chrome navigate <url>             — navigate to a URL
/chrome screenshot                 — take a screenshot, saved to a temp file
/chrome click <selector>           — click a CSS selector
/chrome fill <selector> <text>     — fill an input field
/chrome eval <js>                  — evaluate JavaScript and return the result
/chrome disconnect                 — disconnect from Chrome
```

Useful for testing web applications, scraping, or automating browser-based workflows without a separate browser-automation tool.

---

## Authentication

### /login

Authenticate with Anthropic via OAuth. Opens a browser window for the OAuth flow and stores the resulting credentials in the system keychain or `~/.claude/credentials`.

```
/login
```

---

### /logout

Clear stored authentication tokens. After logout, Claurst will prompt for credentials on next use.

```
/logout
```

---

### /refresh

Refresh the provider authentication state. Forces a token refresh without full re-authentication. Useful when a session token has expired mid-session.

```
/refresh
```

---

## Display & Terminal

### /theme

Documented above under [Configuration & Settings](#configuration--settings).

---

### /output-style

Documented above under [Configuration & Settings](#configuration--settings).

---

### /statusline

Documented above under [Configuration & Settings](#configuration--settings).

---

### /vim

Documented above under [Configuration & Settings](#configuration--settings).

---

### /terminal-setup

Documented above under [Code & Git](#code--git).

---

### /caveman

Activate caveman speech mode. In caveman mode the model strips pleasantries, hedging, articles, and transitional phrases from its responses, producing dense, telegraphic output. Useful for reducing verbosity and saving tokens on long sessions.

```
/caveman             — activate full caveman mode (~75% token reduction)
/caveman lite        — remove pleasantries only (~40% reduction)
/caveman full        — compress sentences and drop articles (default, ~75% reduction)
/caveman ultra       — maximum compression, imperative phrases only (~85% reduction)
```

Deactivate with `/normal`.

---

### /rocky

Activate Rocky speech mode. Rocky is the Eridian alien engineer from *Project Hail Mary* who communicates in a distinctive pidgin English with specific grammar rules and expressive emphasis. In rocky mode the model adopts Rocky's communication style.

```
/rocky             — activate full Rocky mode (~75% token reduction)
/rocky lite        — grammar rules only, minimal emphasis (~40% reduction)
/rocky full        — full Rocky grammar + regular emphasis (default, ~75% reduction)
/rocky ultra       — maximum Rocky personality, frequent emphasis, alien observations
```

Deactivate with `/normal`.

---

### /normal

Deactivate any active speech mode (caveman or rocky) and return the model to its standard response style.

```
/normal
```

---

### /mobile

Display a QR code and download links for the Claude mobile app. Supports a `session` subcommand that generates a QR code linking directly to an active remote Claurst session.

```
/mobile             — show QR code for claude.ai/mobile (works for both platforms)
/mobile ios         — show QR code for the iOS App Store
/mobile android     — show QR code for Google Play
/mobile session     — show QR code linking to the active remote session (requires --remote)
```

---

### /color

Set the prompt bar color for the current session. Accepts standard color names or hex values. The color resets when the session ends unless saved via `/config`.

```
/color               — open the interactive color picker
/color <name>        — set to a named color (e.g., blue, red, green)
/color #ff6b6b       — set to a hex color value
/color default       — reset to the theme default
```

---

### /stickers

Opens the Claurst sticker page (`stickermule.com/claudecode`) in your default browser. Falls back to printing the URL if no browser can be launched.

```
/stickers
```

---

## Diagnostics & Info

### /doctor

Run the Claurst diagnostics suite. Checks configuration integrity, provider connectivity, tool availability, MCP server health, and reports any issues.

```
/doctor
```

---

### /version
**Aliases:** `v`

Display the current Claurst version string and build metadata.

```
/version
/v
```

---

### /update
**Aliases:** `upgrade`

Check for available updates. Queries the GitHub releases API and displays the latest version. If a newer version exists, prints the download URL or upgrade instructions. Does not auto-update.

```
/update
/upgrade
```

---

## Export & Sharing

### /export

Export the current session transcript. Supported formats include Markdown, JSON, and plain text. The output is written to a file or printed to stdout.

```
/export
/export --format markdown
/export --format json --output session.json
/export --stdout
```

---

### /copy

Copy the most recent assistant response to the system clipboard. Pass a number to copy the Nth most-recent response. On Linux a `wl-clipboard` or `xclip` backend is used; on macOS and Windows the native clipboard API is used.

```
/copy         — copy the most recent response
/copy 2       — copy the second most recent response
/copy N       — copy the Nth most recent response
```

---

## Advanced & Internal

### /thinking

Documented above under [Model & Provider](#model--provider).

---

### /connect

Documented above under [Model & Provider](#model--provider).

---

### /fork

Documented above under [Session & Navigation](#session--navigation).

---

### /effort

Documented above under [Model & Provider](#model--provider).

---

### /summary

Generate a summary of the current session. The model produces a condensed description of what was accomplished. Primarily used internally for session metadata.

```
/summary
```

---

### /brief

Output a brief status message for use in non-interactive contexts. Renders minimal session info without the full TUI.

```
/brief
```

---

### /context

Documented above under [Search & Files](#search--files).

---

### /sandbox-toggle
**Aliases:** `sandbox`

Enable or disable sandboxed execution of shell commands. When sandbox mode is on, bash/shell commands run in an isolated environment to limit unintended side effects. Supported on macOS, Linux, and WSL2.

```
/sandbox-toggle                          — toggle sandbox mode on/off
/sandbox-toggle on                       — enable sandbox mode
/sandbox-toggle off                      — disable sandbox mode
/sandbox-toggle status                   — show current state and excluded patterns
/sandbox-toggle exclude <pattern>        — add a command pattern to the exclusion list
```

> A restart is recommended after toggling for full effect. On Windows (non-WSL), sandbox mode is not supported.

---

### /think-back
**Aliases:** `thinkback`

Display the extended-thinking traces from previous model responses in the current session. Only available when extended thinking was used for those responses. Pass a number to view the Nth most-recent trace.

```
/think-back         — show the most recent thinking trace
/think-back 2       — show the second most recent thinking trace
/thinkback          — alias
```

Thinking traces appear when the model uses extended thinking mode (see `/thinking`). If no traces are found, Claurst suggests enabling extended thinking.

---

### /thinkback-play

Replay a previous extended-thinking trace as a formatted, step-numbered walkthrough. Useful for reviewing the model’s reasoning path in detail.

```
/thinkback-play         — replay the most recent thinking trace
/thinkback-play 2       — replay the second most recent thinking trace
```

---

## Command Availability

Not all commands are available in all contexts.

### Remote Mode

When running with `--remote`, only a restricted set of commands is available:

`session`, `exit`, `clear`, `help`, `theme`, `vim`, `cost`, `usage`, `plan`, `keybindings`, `statusline`

### Bridge Mode

Over the Remote Control bridge (used by IDE integrations), only `local`-type commands are forwarded:

`compact`, `clear`, `cost`, `files`

### Internal-Only Commands

The following commands are only available when the `USER_TYPE` environment variable is set to `ant` (Anthropic internal builds):

`commit-push-pr`, `ctx_viz`, `good-claude`, `issue`, `init-verifiers`, `mock-limits`, `bridge-kick`, `ultraplan`, `summary`, `teleport`, `ant-trace`, `perf-issue`, `env`, `oauth-refresh`, `debug-tool-call`, `autofix-pr`, `bughunter`, `backfill-sessions`, `break-cache`

### Availability-Restricted Commands

Some commands are available only under certain account or platform conditions:

| Command | Restriction |
|---------|-------------|
| `/fast` | Available when a fast-mode model is configured for the active provider |
| `/privacy-settings` | Opens Anthropic privacy portal (useful for claude.ai accounts) |
| `/sandbox-toggle` | Functional on macOS, Linux, WSL2 only; no-op on native Windows |

### Feature-Flagged Commands

Some commands check `isEnabled()` at runtime. For example, voice-related commands check for audio device availability; the desktop command checks for a display server.
