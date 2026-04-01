# Claude Code Multi-Backend Router

This package turns Claude Code into a switchable frontend for:

- fully local Ollama models
- local `llama.cpp` / `llama-server`
- cheap cloud OpenAI-compatible models routed through LiteLLM
- Moonshot / Kimi via Anthropic-compatible API mode

It also exposes a shared MCP server called `model-router` so both Claude Code and Cursor can:

- inspect the active backend
- switch between local, hybrid, cloud, Moonshot, and Anthropic modes
- delegate specific prompts to the shared backend stack

## What This Gives You

- `cc`: routed Claude Code wrapper that keeps the normal `claude` binary untouched
- `cursor-cc`: executable wrapper for Cursor terminals and tasks
- `claude-switch`: backend mode switcher
- `cursor-backend`: backend switcher alias for Cursor terminals and task runners
- `model-router`: shared MCP server for Claude Code and Cursor

## Scope

This package is intentionally opinionated and small.
It is a reference setup for one machine, one shell environment, and one shared Claude Code + Cursor workflow.

Use it when you want:

- a known-good local setup with Ollama, `llama.cpp`, LiteLLM, and optional cheap cloud fallbacks
- one command to switch backends without replacing your normal `claude` binary
- one MCP surface that both Claude Code and Cursor can call locally

Do not read it as a claim that this is the most feature-rich router available for Claude Code.
There are stronger standalone routing projects if you want a larger ecosystem, UI, plugin system, or more advanced request rewriting.

## Related Projects

- Anthropic LLM gateway docs: the official Claude Code docs already support using a gateway through `ANTHROPIC_BASE_URL`; this package builds on that pattern for local and open-model workflows.
- LiteLLM: the core proxy/routing layer used here; if you already have your own LiteLLM deployment and rules, you may only want the shell wrappers from this package.
- `musistudio/claude-code-router`: the closest full-featured open-source alternative if you want a standalone router product with richer routing logic, provider catalogs, UI, dynamic `/model`, and automation support.
- `starbaser/ccproxy`: a stronger fit if you specifically want request hooks, rule evaluation, and deep request or response transformation on top of LiteLLM.
- `1rgs/claude-code-proxy`: a smaller Anthropic-compatible proxy if you only need simple provider remapping and not the local shell or MCP workflow in this package.
- OpenHands, Continue, and Aider: better fits if you do not need Claude Code compatibility at all and are happy to adopt a different coding-agent frontend.

In short:

- keep this package if your priority is "working local starter kit for Claude Code and Cursor"
- use a dedicated router project if your priority is "largest routing feature set"
- use a different agent product if your priority is "best open-source coding agent overall" rather than preserving Claude Code semantics

## Routing

| Mode | haiku | sonnet | opus |
| --- | --- | --- | --- |
| `local` | `ollama/hermes3:8b` | `ollama/qwen3-coder:30b` | `ollama/qwen3-coder:30b-128k` |
| `llamacpp` | `llama.cpp qwen3-coder-30b` | `llama.cpp qwen3-coder-30b` | `llama.cpp qwen3-coder-30b` |
| `hybrid` | `ollama/hermes3:8b` | `ollama/qwen3-coder:30b` | `deepseek/deepseek-chat` |
| `cloud` | `deepseek/deepseek-chat` | `deepseek/deepseek-chat` | `deepseek/deepseek-reasoner` |
| `moonshot` | `kimi-k2.5` | `kimi-k2.5` | `kimi-k2.5` by default, overrideable to `kimi-k2-thinking` |
| `anthropic` | direct | direct | direct |

Versioned Claude model aliases are also registered for the common 3.5/3.7/4.5/4.6 names Claude Code emits.

## Install

1. Install the Python dependencies:

```bash
pip install "litellm[proxy]" mcp httpx
```

2. Install the router files into your home directory:

```bash
cd contrib/multi-backend-router
./install.sh
```

3. Open a new shell or run:

```bash
source ~/.zshrc
```

4. Optional: add cloud keys:

```bash
cp ~/.claude/litellm/env.example ~/.claude/litellm/env
```

5. Pick a mode and start using it:

```bash
claude-switch local
cc
```

## Cursor Setup

This package does not automatically rewrite your Cursor config. Add this MCP entry to `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "model-router": {
      "command": "python3",
      "args": [
        "/Users/YOUR_USER/.claude/litellm/mcp-model-router.py"
      ],
      "env": {
        "CLAUDE_LITELLM_HOME": "/Users/YOUR_USER/.claude/litellm",
        "PYTHONUNBUFFERED": "1"
      }
    }
  }
}
```

Then Cursor terminals and tasks can use:

```bash
cursor-backend local
cursor-cc
```

`cursor-agent mcp list-tools model-router` should show the shared MCP tools once enabled.

## Claude MCP Setup

For Claude Code itself, add the shared MCP server once:

```bash
claude mcp add -s user model-router python3 ~/.claude/litellm/mcp-model-router.py
```

## Files

- `configs/local.yaml`: local Ollama routing
- `configs/llamacpp.yaml`: LiteLLM -> local `llama-server`
- `configs/hybrid.yaml`: local routing with cloud fallbacks
- `configs/cloud.yaml`: cloud-only routing
- `start.sh`: starts the active backend stack
- `stop.sh`: stops owned LiteLLM / llama.cpp / Ollama helper processes
- `status.sh`: shows mode, health, loaded Ollama models, and key availability
- `shell-functions.zsh`: defines `cc`, `claude-status`, `cs`, and `cb`
- `mcp-model-router.py`: shared FastMCP server
- `bin/claude-switch`: backend switcher
- `bin/cursor-backend`: Cursor-friendly switcher
- `bin/cursor-cc`: Cursor-friendly routed Claude launcher
- `env.example`: cloud keys and local overrides template
- `requirements.txt`: Python deps for LiteLLM + MCP helper

## Compatibility Notes

- Routed local modes set a dummy local auth token automatically so they do not depend on Claude subscriber OAuth just to reach LiteLLM on localhost.
- `cc` and `cursor-cc` automatically set `CLAUDE_CODE_DISABLE_THINKING=1` in `local`, `llamacpp`, and `hybrid`.
  This avoids Anthropic-specific thinking payloads breaking Ollama and `llama.cpp` backends.
- Small local models can still be more tool-happy than Claude on trivial prompts.
  For deterministic smoke tests, use `--bare -p --tools ''`.
- Cursor Background Agents run remotely and cannot reach your local `127.0.0.1` Ollama/LiteLLM stack.
- Moonshot mode is direct and does not use LiteLLM.

## Useful Commands

```bash
claude-switch local
claude-switch llamacpp
claude-switch hybrid
claude-switch cloud
claude-switch moonshot
claude-switch anthropic
claude-status
cc
cc -p "explain this code path"
cursor-backend status
cursor-cc --bare -p --tools '' --model haiku -- "Reply with exactly OK."
cursor-agent mcp list-tools model-router
```

## Smoke Tests Used On This Mac

These are the bare-path checks that validated the routed stack locally:

```bash
cc --bare -p --tools '' --model haiku -- "Reply with exactly OK."
cursor-cc --bare -p --tools '' --model haiku -- "Reply with exactly OK."
```

Both completed successfully against the local proxy setup during packaging.
