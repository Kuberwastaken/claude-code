# Claude Code — Spec Index

> Quick-reference index across all spec documents.
> Total spec coverage: ~990 KB across 15 markdown files.

---

## Spec Files

| # | File | Size | What's Inside |
|---|------|------|---------------|
| — | [00_overview.md](00_overview.md) | 16 KB | Master architecture, repo structure, data flow, permission model, settings layers |
| 01 | [01_core_entry_query.md](01_core_entry_query.md) | 73 KB | `main.tsx`, `query.ts`, `QueryEngine.ts`, entry points, history, cost tracking, token budget |
| 02 | [02_commands.md](02_commands.md) | 71 KB | All 100+ slash commands with args, options, and implementation |
| 03 | [03_tools.md](03_tools.md) | 67 KB | All 40+ tools: input schemas, permissions, outputs, shared utilities |
| 04 | [04_components_core_messages.md](04_components_core_messages.md) | 93 KB | 130 top-level UI components + all message rendering components |
| 05 | [05_components_agents_permissions_design.md](05_components_agents_permissions_design.md) | 64 KB | Agent creation wizard, permission dialogs, design system, PromptInput, Spinner |
| 06 | [06_services_context_state.md](06_services_context_state.md) | 95 KB | Analytics, API client, session memory, autoDream, compact, voice, contexts, state |
| 07 | [07_hooks.md](07_hooks.md) | 84 KB | All 104 React hooks with params, return types, and behavior |
| 08 | [08_ink_terminal.md](08_ink_terminal.md) | 78 KB | Custom terminal framework: React reconciler, Yoga layout, screen buffer, ANSI tokenizer |
| 09 | [09_bridge_cli_remote.md](09_bridge_cli_remote.md) | 75 KB | Bridge protocol, JWT auth, SSE/WebSocket/Hybrid transports, remote sessions |
| 10 | [10_utils.md](10_utils.md) | 60 KB | ~564 utility files organized by category |
| 11 | [11_special_systems.md](11_special_systems.md) | 64 KB | Buddy/Tamagotchi, memdir, keybindings, skills, voice, plugins, migrations |
| 12 | [12_constants_types.md](12_constants_types.md) | 83 KB | Every constant, type, OAuth config, system prompts, tool limits, beta headers |
| 13 | [13_rust_codebase.md](13_rust_codebase.md) | 63 KB | Complete Rust rewrite: all 9 crates, 33 tools, query loop, TUI, bridge |
| 14 | [14_multi_provider_plan.md](14_multi_provider_plan.md) | 9 KB | Step-by-step implementation plan for multi-provider model support in the Rust codebase |

---

## Quick Lookup

### "Where is X documented?"

| Topic | Spec File | Section |
|-------|-----------|---------|
| Main entry point (`main.tsx`) | 01 | §1 |
| Query/turn execution loop | 01 | §2–3 |
| Token budget & compaction | 01 | §token-budget |
| Tool base class & framework | 03 | §1 |
| BashTool | 03 | §BashTool |
| FileEditTool | 03 | §FileEditTool |
| AgentTool (sub-agents) | 03 | §AgentTool |
| WebSearchTool | 03 | §WebSearchTool |
| MCPTool | 03 | §MCPTool |
| All slash commands | 02 | §per-command |
| `/compact` command | 02 | §compact |
| `/mcp` command | 02 | §mcp |
| `/plan` command | 02 | §plan |
| Permission dialog system | 05 | §permissions |
| Permission rules (settings) | 05 | §rules |
| PromptInput component | 05 | §PromptInput |
| Message rendering | 04 | §messages |
| Spinner component | 05 | §Spinner |
| Agent creation wizard | 05 | §agents |
| Claude API client | 06 | §api/claude |
| Analytics / telemetry | 06 | §analytics |
| Session memory | 06 | §SessionMemory |
| AutoDream consolidation | 06 | §autoDream |
| Rate limiting | 06 | §claudeAiLimits |
| Context compaction | 06 | §compact |
| React contexts | 06 | §context |
| Bootstrap state (80+ fields) | 06 | §bootstrap |
| Coordinator mode | 06 | §coordinator |
| All React hooks | 07 | §per-hook |
| Ink reconciler | 08 | §reconciler |
| Yoga layout engine | 08 | §layout |
| Screen buffer / rendering | 08 | §screen |
| ANSI/CSI/ESC handling | 08 | §termio |
| Bridge protocol | 09 | §bridge |
| JWT authentication | 09 | §jwtUtils |
| SSE transport | 09 | §SSETransport |
| WebSocket transport | 09 | §WebSocketTransport |
| Remote sessions | 09 | §remote |
| Buddy/Tamagotchi | 11 | §buddy |
| Gacha mechanics (PRNG) | 11 | §buddy-gacha |
| Memory directory system | 11 | §memdir |
| Keybinding parser | 11 | §keybindings |
| Skills system | 11 | §skills |
| Voice / STT | 11 | §voice |
| Plugin system | 11 | §plugins |
| Model migration history | 11 | §migrations |
| All constants | 12 | §constants |
| System prompt architecture | 12 | §prompts |
| OAuth configuration | 12 | §oauth |
| Beta feature headers | 12 | §betas |
| Cyber risk instruction | 12 | §cyberRisk |
| Tool name constants | 12 | §tools |
| All TypeScript types | 12 | §types |
| Rust rewrite overview | 13 | §1 |
| Rust tool implementations | 13 | §cc-tools |
| Rust query loop | 13 | §cc-query |
| Rust TUI | 13 | §cc-tui |
| Rust bridge | 13 | §cc-bridge |

---

## Key Numbers

| Metric | Value |
|--------|-------|
| Total TypeScript/TSX files | ~1,902 |
| Total lines of code | ~800K+ |
| Number of slash commands | 100+ |
| Number of tools | 40+ |
| Number of React hooks | 104 |
| Number of React components | 389 files |
| Number of services | 130 files |
| Number of utility files | ~564 |
| Ink terminal framework files | 96 |
| Bridge protocol files | 31 |
| Rust crates | 9 |
| Rust source files | 47 |
| Spec documentation size | ~990 KB |

---

## Architecture in One Paragraph

Claude Code is a terminal AI coding assistant built as a React application running in a custom terminal UI framework (Ink, a React reconciler targeting terminal output with Yoga flexbox layout). The main loop (`query.ts` + `QueryEngine.ts`) streams responses from the Claude API, executes tools with user permission, and manages a 200K-token context window with automatic compaction. It has 100+ slash commands, 40+ tools (file I/O, shell, web, agents, MCP), a multi-agent system for parallel task execution, a memory system for long-term context, voice input, IDE integration via a bridge protocol (WebSocket/SSE), and a plugin/skills marketplace. The codebase is being rewritten in Rust (`claude-code-rust/`) as a complete standalone reimplementation.

---

*Generated 2026-03-31 from Claude Code source analysis.*
