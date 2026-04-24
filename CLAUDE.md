# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Claurst is an open-source, multi-provider terminal coding agent built in Rust. It is a clean-room reimplementation of Claude Code's behavior (not a source port). The behavioral specifications live in `spec/`; the Rust implementation lives in `src-rust/`.

## Repository Layout

- `src-rust/` — Rust workspace (11 crates). All build/test commands run from here.
- `spec/` — Behavioral specification documents used for the clean-room implementation.
- `docs/` — User-facing documentation (HTML + Markdown).
- `public/` — Static assets for the documentation site.
- `index.html` — Documentation landing page.

## Build & Development

All cargo commands are run from `src-rust/`:

```bash
cd src-rust

# Debug build
cargo build --package claurst

# Release build
cargo build --release --package claurst

# Build without voice/ALSA support (for headless servers, Raspberry Pi, Debian Trixie)
cargo build --release --package claurst --no-default-features

# Run tests for all workspace crates
cargo test

# Run tests for a specific crate
cargo test --package claurst-core
cargo test --package claurst-tui

# Run the binary
cargo run --package claurst

# Run a one-shot headless query
cargo run --package claurst -- -p "explain this codebase"
```

On Linux, the default build requires `libasound2-dev` and `pkg-config` for the optional voice feature.

## Workspace Architecture

The project uses a Cargo workspace with 11 crates. The dependency graph flows roughly:

```
cli (binary entry point)
  ├── core
  ├── api
  ├── tools
  ├── query
  ├── tui
  ├── commands
  ├── mcp
  ├── bridge
  ├── plugins
  └── acp
```

### Crate Responsibilities

- **`cli`** (`crates/cli/`) — Binary entry point. Parses CLI args with `clap`, initializes `tracing`, loads config, and dispatches to either headless mode (`-p`) or the interactive TUI. Also contains OAuth flow implementations.
- **`core`** (`crates/core/`) — The largest crate. Holds shared types, configuration, session state, permissions, auth storage, feature flags/gates, git utilities, keybindings, system prompt construction, LSP integration, voice input, and the main `lib.rs` (~160KB) that ties together much of the app logic.
- **`api`** (`crates/api/`) — LLM provider abstraction. Contains provider trait definitions, model registry, request/response types, streaming parsers, and provider-specific adapters (Anthropic, OpenAI, Ollama, DeepSeek, Groq, Mistral, GitHub Copilot, etc.).
- **`tools`** (`crates/tools/`) — Tool implementations that the agent can invoke: file read/write, bash execution (with PTY support on Unix), grep, web fetch, and computer-use tools (gated behind `computer-use` feature). Defines the `Tool` trait and `ToolContext`.
- **`query`** (`crates/query/`) — Query/turn execution engine. Orchestrates a single turn: builds context, sends messages to the API, streams the response, handles tool calls, and loops until the turn completes.
- **`tui`** (`crates/tui/`) — Terminal UI built on `ratatui` + `crossterm`. Renders chat messages, diff viewers, settings screens, session browser, model picker, plugin views, onboarding, and the companion "Rustle" character. Contains snapshot tests under `crates/tui/tests/`.
- **`commands`** (`crates/commands/`) — Slash command registry (`/clear`, `/compact`, `/commit`, etc.) and named commands (`claude agents`, `claude migrate`, etc.). Named commands are registered in `named_commands.rs` and invoked before the REPL starts.
- **`mcp** (`crates/mcp/`) — Model Context Protocol support. Manages MCP server connections and exposes external tools through the native `Tool` trait.
- **`bridge`** (`crates/bridge/`) — IDE bridge and remote session protocol (WebSocket/SSE). Enables VS Code / JetBrains integration and cloud session sync.
- **`plugins`** (`crates/plugins/`) — Plugin system for extending functionality.
- **`buddy`** (`crates/buddy/`) — Companion "Rustle" system (Tamagotchi-style state and interactions).
- **`acp`** (`crates/acp/`) — Agent Communication Protocol utilities.

## Feature Flags

`claurst-core` defines a large set of experimental feature flags (all off by default except `ultraplan`). The full set is enabled with the `dev_full` feature. The `cli` and `tui` crates pass these through as needed.

Key flags:
- `voice` — Enables real microphone capture via `cpal`. Disabled with `--no-default-features`.
- `dev_full` — Enables all 36 experimental features (ultraplan, agent triggers, memory extraction, bridge mode, bash classifier, etc.).

Example:
```bash
cargo build --release --package claurst --features claurst-core/dev_full
```

## Configuration & Data

- User settings: `~/.claurst/settings.json`
- Session history: `~/.claurst/sessions/`
- Auth/tokens: stored in the OS keychain via `auth_store.rs`
- Local project config: `.claurst/` directory in the project root (gitignored)

### Migrating from Claude Code

Use the `migrate` named command to import Claude Code configuration into Claurst:

```bash
# Preview what would be migrated (dry-run)
claude migrate --dry-run

# Perform the migration (merges with existing Claurst config)
claude migrate

# Overwrite existing Claurst config instead of merging
claude migrate --overwrite

# Also copy session history
claude migrate --sessions
```

This migrates:
- Global `~/.claude/settings.json` → `~/.claurst/settings.json`
- Project `.claude/settings.json` → `.claurst/settings.json`
- `~/.claude/CLAUDE.md` / `AGENTS.md` → `~/.claurst/` (preserving filename)
- `~/.claude/skills/` → `~/.claurst/skills/`
- Project `.claude/skills/` → `.claurst/skills/`
- MCP servers (object format is converted to array format)
- Skills config, agents, providers, hooks, formatters, custom commands, etc.

Claurst uses `~/.claurst/` — it does **not** automatically read Claude Code's `~/.claude/` directory.

## Devcontainer

A VS Code devcontainer is configured in `.devcontainer/`. It uses the `rust:1-bullseye` image with `libasound2-dev`, `libxdo-dev`, and `pkg-config` preinstalled. Run "Reopen in Container" from VS Code to use it.

## Release Process

Releases are built via `.github/workflows/release.yml` (manual workflow dispatch from `main` only). It builds for 5 targets (Windows x86_64, Linux x86_64/aarch64, macOS x86_64/aarch64) and creates a GitHub Release with auto-generated notes. The version in `src-rust/Cargo.toml` must match the tag.
