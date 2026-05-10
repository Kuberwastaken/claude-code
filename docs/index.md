<div align="center">

# Claurst

<img src="../public/Ship.png" alt="Rustle on the ship" width="350" />

Claurst is a high-performance Rust reimplementation of Claude Code — a terminal-native AI coding agent with streaming responses, 40+ built-in tools, 15+ LLM provider integrations, a full ratatui TUI, and an extensible plugin system.

**Version:** 0.0.9 · **License:** GPL-3.0 · [GitHub](https://github.com/Kuberwastaken/claurst)

</div>

---

## What Claurst does

You give Claurst a task in natural language. It plans, reads and writes files, runs shell commands, searches the web, and iterates — all inside your terminal, with every step visible in real time.

```
$ claurst "add input validation to the signup form"
```

Claurst reads your codebase, implements the change across multiple files, runs your tests, and reports back — without you leaving the terminal.

---

## Key capabilities

### Agentic loop
Claurst runs a multi-turn loop: it streams a response from the model, executes any tool calls (file read, bash, web search, …), feeds the results back, and continues until the task is done or the turn limit is reached.

### 40+ built-in tools
- **File operations** — read, write, edit, patch, batch-edit
- **Shell** — bash with persistent working directory and environment
- **Search** — glob file patterns, grep contents, web search, web fetch
- **Git** — commit, branch, worktree
- **Notebooks** — read and edit Jupyter notebooks
- **Desktop automation** — screenshot, click, type (optional feature)
- **Task management** — create, track, and complete tasks

### 15+ LLM providers
Anthropic Claude (default), OpenAI, Google Gemini, AWS Bedrock, Azure OpenAI, Ollama, Groq, Mistral, DeepSeek, xAI, Cohere, OpenRouter, Together AI, Perplexity, GitHub Copilot, Cerebras, LM Studio, and LLaMA.cpp.

### AMOLED terminal UI
A ratatui-based TUI with real-time streaming, syntax-highlighted code blocks, diff viewer, permission dialogs, slash command autocomplete, session browser, and a full keybinding system.

### Plugin system
Extend Claurst with TOML-manifest plugins that add custom slash commands, MCP servers, hooks, output styles, and tool overlays.

### Multi-agent orchestration
Run named agents (`build`, `plan`, `explore`) or spawn parallel sub-agents in coordinator mode. Agents communicate via a shared task registry and message channels.

### Goal system
Set a durable objective with `/goal` and Claurst works autonomously across turns until the goal is verified complete — using the `GoalCompleteTool` for audited completion rather than just stopping.

### Managed agents (preview)
Configure a manager-executor architecture with `/managed-agents` where a manager model delegates subtasks to parallel executor agents with full budget split controls.

### Speech modes
Activate `/caveman` or `/rocky` to compress model responses by 40–85%, saving tokens in long sessions. Deactivate with `/normal`.

---

## Quick start

**1. Install**

```bash
# Linux / macOS
curl -fsSL https://github.com/Kuberwastaken/claurst/releases/latest/download/install.sh | bash
```

```powershell
# Windows (PowerShell)
irm https://github.com/Kuberwastaken/claurst/releases/latest/download/install.ps1 | iex
```

The installer auto-detects your platform/arch, drops `claurst` into
`~/.claurst/bin/`, and adds it to your `PATH`. See
[Installation](installation) for flags, manual download, and uninstall steps.

**2. Set your API key**

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

**3. Run interactively**

```bash
claurst
```

Or send a single prompt and exit:

```bash
claurst --print "explain the auth module"
```

---

## Configuration

Claurst reads `~/.claurst/settings.json` at startup. The most common settings:

```json
{
  "config": {
    "model": "claude-opus-4-6",
    "permission_mode": "default",
    "auto_compact": true,
    "compact_threshold": 0.8
  }
}
```

See [Configuration](configuration) for the full reference.

---

## Using a different provider

```bash
# Use Ollama with a local model
claurst --provider ollama --model llama3.2

# Use OpenAI
OPENAI_API_KEY=sk-... claurst --provider openai --model gpt-4o
```

See [Providers](providers) for setup instructions for every supported provider.

---

## Interactive vs headless

| Mode | Command | Use case |
|------|---------|----------|
| Interactive TUI | `claurst` | Day-to-day coding |
| Single prompt | `claurst "task"` | Quick one-shot tasks |
| Headless print | `claurst --print "task"` | Scripts, CI |
| JSON output | `claurst --output-format json "task"` | Machine consumption |
| Stream JSON | `claurst --output-format stream-json "task"` | Real-time piping |

---

## Slash commands

Inside the interactive TUI, type `/` to see all available commands. Common ones:

| Command | Description |
|---------|-------------|
| `/help` | Show all commands |
| `/model` | Switch model or provider |
| `/goal <objective>` | Set an autonomous multi-turn goal |
| `/managed-agents` | Configure manager-executor agents |
| `/compact` | Compress conversation history |
| `/cost` | Token usage and cost for this session |
| `/insights` | Session statistics and tool usage report |
| `/caveman` | Activate telegraphic speech mode (save tokens) |
| `/rocky` | Activate Rocky (Project Hail Mary) speech mode |
| `/normal` | Deactivate speech modes |
| `/rewind` | Go back to a previous message |
| `/copy` | Copy last response to clipboard |
| `/export` | Save session transcript |
| `/think-back` | View thinking traces from previous responses |
| `/ultrareview` | Exhaustive multi-dimensional code review |
| `/advisor <model>` | Set a secondary advisor model |
| `/sandbox-toggle` | Toggle sandboxed shell execution |
| `/update` | Check for and download updates |
| `/exit` | Quit |

See [Slash Commands](commands) for the complete reference.

---

## Next steps

- [Installation](installation) — download, build from source, system requirements
- [Authentication](auth) — API keys and OAuth
- [Configuration](configuration) — settings.json reference
- [Slash Commands](commands) — all 70+ commands
- [Tools Reference](tools) — all 40+ tools and permission levels
- [Providers](providers) — configuring each LLM provider
- [MCP Integration](mcp) — Model Context Protocol servers
- [Plugins](plugins) — building and using plugins
- [Agents](agents) — multi-agent orchestration
- [Hooks](hooks) — event-driven automation
- [Advanced Features](advanced) — extended thinking, sessions, and more
