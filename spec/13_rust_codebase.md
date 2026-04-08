# Claude Code — Rust Codebase

## Overview

The Rust codebase at `claude-code-rust/` is a **complete standalone rewrite** of the TypeScript Claude Code CLI in async Rust. It is not an FFI binding layer, not a partial port, and shares no runtime code with the TypeScript implementation. It re-implements the same tool names and semantics, permission model, CLAUDE.md discovery, auto-compact logic, MCP (Model Context Protocol) client, bridge protocol, and cron scheduler — all in async Rust using the Tokio runtime.

### Architecture

```
claude-code-rust/
├── Cargo.toml                  # Workspace root
└── crates/
    ├── core/       (cc-core)       # Shared types, config, permissions, history, hooks
    ├── api/        (cc-api)        # API client + SSE streaming
    ├── tools/      (cc-tools)      # All tool implementations (33 tools)
    ├── query/      (cc-query)      # Agentic query loop, compact, cron scheduler
    ├── tui/        (cc-tui)        # ratatui terminal UI
    ├── commands/   (cc-commands)   # Slash command implementations
    ├── mcp/        (cc-mcp)        # MCP (Model Context Protocol) client
    ├── bridge/     (cc-bridge)     # Bridge to claude.ai web UI
    └── cli/        (claude-code)   # Binary entry point
```

**Dependency flow:**
```
cli → query → tools → core
         ↓         ↗
        api  →  core
         ↓
       commands → core
         ↓
        tui   → core
         ↓
        mcp   → core
         ↓
       bridge → core
```

---

## Workspace Root: `Cargo.toml`

**Path:** `claude-code-rust/Cargo.toml`

Cargo workspace with `resolver = "2"`, edition `2021`, version `1.0.0` across all member crates.

### Workspace Members
| Member Path | Package Name | Type |
|---|---|---|
| `crates/core` | `cc-core` | Library |
| `crates/api` | `cc-api` | Library |
| `crates/tools` | `cc-tools` | Library |
| `crates/query` | `cc-query` | Library |
| `crates/tui` | `cc-tui` | Library |
| `crates/commands` | `cc-commands` | Library |
| `crates/mcp` | `cc-mcp` | Library |
| `crates/bridge` | `cc-bridge` | Library |
| `crates/cli` | `claude-code` | Binary (`[[bin]] name = "claude"`) |

### Key Shared Dependencies

| Crate | Version | Features |
|---|---|---|
| `tokio` | 1.44 | `full` |
| `reqwest` | 0.12 | `json`, `stream`, `rustls-tls` |
| `ratatui` | 0.29 | default |
| `crossterm` | 0.28 | `event-stream` |
| `clap` | 4 | `derive`, `env`, `string` |
| `serde` | 1 | `derive` |
| `serde_json` | 1 | default |
| `anyhow` | 1 | default |
| `thiserror` | 2 | default |
| `tracing` | 0.1 | default |
| `tracing-subscriber` | 0.3 | `env-filter` |
| `uuid` | 1 | `v4` |
| `chrono` | 0.4 | `serde` |
| `regex` | 1 | default |
| `glob` | 0.3 | default |
| `walkdir` | 2 | default |
| `similar` | 2 | default (declared, not heavily used) |
| `once_cell` | 1 | default |
| `parking_lot` | 0.12 | default |
| `dashmap` | 6 | default |
| `tokio-util` | 0.7 | `codec`, `sync` |
| `async-trait` | 0.1 | default |
| `schemars` | 0.8 | `derive` |
| `nix` | 0.29 | `process`, `signal`, `user` |
| `base64` | 0.22 | default |
| `sha2` | 0.10 | default |
| `hex` | 0.4 | default |

---

## Crate: `cc-core`

**Path:** `crates/core/src/lib.rs`

Central shared crate. Defines all types consumed by every other crate. Contains 9 inline submodules.

### Module: `error`

**`ClaudeError` enum** (implements `std::error::Error` via `thiserror`):
- `Api(String)` — Generic API error
- `ApiStatus { status_code: u16, message: String }` — HTTP status error
- `Auth(String)` — Authentication failure
- `PermissionDenied(String)` — Tool permission denied
- `Tool(String)` — Tool execution error
- `Io(#[from] std::io::Error)` — I/O error
- `Json(#[from] serde_json::Error)` — JSON parse error
- `Http(#[from] reqwest::Error)` — HTTP client error
- `RateLimit { retry_after: Option<u64> }` — 429 rate limit
- `ContextWindowExceeded` — Context window full
- `MaxTokensReached` — max_tokens hit
- `Cancelled` — User/signal cancellation
- `Config(String)` — Config load/save error
- `Mcp(String)` — MCP protocol error
- `Other(String)` — Catch-all

**Key methods:**
- `is_retryable(&self) -> bool` — true for `RateLimit` and `ApiStatus` with code 529
- `is_context_limit(&self) -> bool` — true for `ContextWindowExceeded` and `MaxTokensReached`

### Module: `types`

**`Role` enum:** `User`, `Assistant`

**`ContentBlock` enum** (serde untagged):
- `Text { text: String }`
- `Image { source: ImageSource }`
- `ToolUse { id: String, name: String, input: Value }`
- `ToolResult { tool_use_id: String, content: ToolResultContent, is_error: Option<bool> }`
- `Thinking { thinking: String, signature: String }`
- `RedactedThinking { data: String }`
- `Document { source: DocumentSource, citations: Option<CitationsConfig> }`

**`MessageContent` enum** (serde untagged):
- `Text(String)`
- `Blocks(Vec<ContentBlock>)`

**`Message` struct:**
- Fields: `role: Role`, `content: MessageContent`
- `Message::user(text)` — convenience constructor
- `Message::assistant(text)` — convenience constructor
- `Message::user_blocks(blocks: Vec<ContentBlock>)` — multi-block user message
- `Message::assistant_blocks(blocks)` — multi-block assistant message
- `get_text() -> Option<&str>` — first Text block or Text content
- `get_all_text() -> String` — concatenate all text blocks
- `get_tool_use_blocks() -> Vec<ContentBlock>` — filter ToolUse blocks
- `get_thinking_blocks() -> Vec<ContentBlock>` — filter Thinking blocks
- `has_tool_use() -> bool`
- `content_blocks() -> &[ContentBlock]`

**`UsageInfo` struct:**
- Fields: `input_tokens: u32`, `output_tokens: u32`, `cache_creation_input_tokens: u32`, `cache_read_input_tokens: u32`
- `total_input() -> u32` — sum of input + cache tokens
- `total() -> u32` — sum of all tokens
- Implements `Default`

**`ToolDefinition` struct:** `{ name: String, description: String, input_schema: Value }`

**Supporting types:** `MessageCost`, `ImageSource { type, media_type, data }`, `DocumentSource`, `CitationsConfig`, `ToolResultContent` (Text/Blocks enum)

### Module: `config`

**`Config` struct** — runtime configuration:
- `api_key: Option<String>`
- `api_base: Option<String>`
- `model: String`
- `max_tokens: u32`
- `permission_mode: PermissionMode`
- `verbose: bool`
- `output_format: OutputFormat`
- `max_turns: u32`
- `system_prompt: Option<String>`
- `append_system_prompt: Option<String>`
- `no_claude_md: bool`
- `auto_compact: bool`
- `thinking_budget: Option<u32>`
- `mcp_servers: Vec<McpServerConfig>`
- `hooks: HashMap<HookEvent, Vec<HookEntry>>`

**Key methods:**
- `resolve_api_key() -> Option<String>` — checks `config.api_key` then `ANTHROPIC_API_KEY` env var
- `resolve_api_base() -> String` — checks `ANTHROPIC_BASE_URL` env var, falls back to constant
- `effective_model() -> &str`
- `effective_max_tokens() -> u32` — global fallback (prefers `effective_max_tokens_for_model()` in `claurst_api` for per-model resolution)

**`PermissionMode` enum:**
- `Default` — allow read-only operations automatically
- `AcceptEdits` — allow all edits automatically
- `BypassPermissions` — allow everything without prompting
- `Plan` — read-only planning mode

**`OutputFormat` enum:** `Text`, `Json`, `StreamJson`

**`HookEvent` enum:** `PreToolUse`, `PostToolUse`, `Stop`, `UserPromptSubmit`, `Notification`

**`HookEntry` struct:** `{ command: String, tool_filter: Option<String>, blocking: bool }`

**`McpServerConfig` struct:** `{ name: String, command: String, args: Vec<String>, env: HashMap<String, String>, url: Option<String>, server_type: McpServerType }`

**`Settings` struct** — persisted user preferences at `~/.claude/settings.json`:
- `async fn load() -> Result<Settings>` — deserializes JSON, returns default on missing file
- `async fn save(&self) -> Result<()>` — serializes to JSON, creates parent dirs

### Module: `constants`

All constants are `pub const`:

| Constant | Value |
|---|---|
| `APP_NAME` | `"claude"` |
| `DEFAULT_MODEL` | `"claude-opus-4-6"` |
| `SONNET_MODEL` | `"claude-sonnet-4-6"` |
| `HAIKU_MODEL` | `"claude-haiku-4-5-20251001"` |
| `DEFAULT_MAX_TOKENS` | `32_000` | Fallback when model not in registry; per-model limits used otherwise |
| `MAX_TOKENS_HARD_LIMIT` | `65_536` |
| `DEFAULT_COMPACT_THRESHOLD` | `0.9` |
| `MAX_TURNS_DEFAULT` | `10` |
| `ANTHROPIC_API_BASE` | `"https://api.anthropic.com"` |
| `ANTHROPIC_API_VERSION` | `"2023-06-01"` |
| `ANTHROPIC_BETA_HEADER` | `"interleaved-thinking-2025-05-14,token-efficient-tools-2025-02-19,files-api-2025-04-14"` |
| `CLAUDE_MD_FILENAME` | `"CLAUDE.md"` |
| `SETTINGS_FILENAME` | `"settings.json"` |
| `HISTORY_FILENAME` | `"history.json"` |
| `CONFIG_DIR_NAME` | `".claude"` |

**Tool name constants:**
- `TOOL_NAME_BASH = "Bash"`
- `TOOL_NAME_FILE_EDIT = "Edit"`
- `TOOL_NAME_FILE_READ = "Read"`
- `TOOL_NAME_FILE_WRITE = "Write"`
- `TOOL_NAME_GLOB = "Glob"`
- `TOOL_NAME_GREP = "Grep"`
- `TOOL_NAME_WEB_FETCH = "WebFetch"`
- `TOOL_NAME_WEB_SEARCH = "WebSearch"`
- `TOOL_NAME_NOTEBOOK_EDIT = "NotebookEdit"`
- `TOOL_NAME_AGENT = "Task"` (sub-agent)
- `TOOL_NAME_TODO_WRITE = "TodoWrite"`
- `TOOL_NAME_ASK_USER = "AskUserQuestion"`
- `TOOL_NAME_ENTER_PLAN_MODE = "EnterPlanMode"`
- `TOOL_NAME_EXIT_PLAN_MODE = "ExitPlanMode"`
- `TOOL_NAME_POWERSHELL = "PowerShell"`
- `TOOL_NAME_SLEEP = "Sleep"`
- `TOOL_NAME_CRON_CREATE = "CronCreate"`
- `TOOL_NAME_CRON_DELETE = "CronDelete"`
- `TOOL_NAME_CRON_LIST = "CronList"`
- `TOOL_NAME_ENTER_WORKTREE = "EnterWorktree"`
- `TOOL_NAME_EXIT_WORKTREE = "ExitWorktree"`
- `TOOL_NAME_LIST_MCP_RESOURCES = "ListMcpResources"`
- `TOOL_NAME_READ_MCP_RESOURCE = "ReadMcpResource"`
- `TOOL_NAME_TOOL_SEARCH = "ToolSearch"`
- `TOOL_NAME_BRIEF = "Brief"`
- `TOOL_NAME_CONFIG = "Config"`
- `TOOL_NAME_SEND_MESSAGE = "SendMessage"`
- `TOOL_NAME_SKILL = "Skill"`

### Module: `context`

**`ContextBuilder`** — builds system context strings injected into the system prompt:

- `build_system_context(working_dir: &Path) -> String`
  - Platform (OS + architecture)
  - Current working directory
  - Git status (runs `git status --short`)
  - Last 5 git commits (runs `git log --oneline -5`)

- `build_user_context(working_dir: &Path, no_claude_md: bool) -> String`
  - Current date/time (from `chrono::Local::now()`)
  - CLAUDE.md discovery: walks from `working_dir` up to filesystem root, collecting any `CLAUDE.md` files; also reads `~/.claude/CLAUDE.md`
  - Returns concatenated content of all discovered CLAUDE.md files

### Module: `permissions`

**`PermissionDecision` enum:** `Allow`, `AllowPermanently`, `Deny`, `DenyPermanently`

**`PermissionRequest` struct:** `{ tool_name: String, description: String, details: Option<String>, is_read_only: bool }`

**`PermissionHandler` trait:**
- `check_permission(&self, tool_name: &str) -> PermissionDecision`
- `request_permission(&self, request: &PermissionRequest) -> PermissionDecision`

**`AutoPermissionHandler`** — automatic non-interactive handler:
- `BypassPermissions` → `Allow` all requests
- `AcceptEdits` → `Allow` all requests
- `Plan` → `Allow` only if `is_read_only == true`, else `Deny`
- `Default` → `Allow` only if `is_read_only == true`, else `Deny`

### Module: `history`

**`ConversationSession` struct:**
```
id: String (UUID v4)
created_at: DateTime<Utc>
updated_at: DateTime<Utc>
messages: Vec<Message>
model: String
title: Option<String>
working_dir: String
```

**Functions:**
- `save_session(session: &ConversationSession) -> Result<()>` — writes to `~/.claude/conversations/<id>.json`
- `load_session(id: &str) -> Result<Option<ConversationSession>>` — reads from path above
- `list_sessions() -> Result<Vec<ConversationSession>>` — reads all `.json` files in `~/.claude/conversations/`, sorts by `updated_at` descending
- `delete_session(id: &str) -> Result<()>` — removes the file

### Module: `cost`

**`ModelPricing` struct:** `{ input_per_mtok: f64, output_per_mtok: f64, cache_creation_per_mtok: f64, cache_read_per_mtok: f64 }`

**Pricing constants:**
| Model | Input ($/MTok) | Output ($/MTok) |
|---|---|---|
| `OPUS` | $15.00 | $75.00 |
| `SONNET` | $3.00 | $15.00 |
| `HAIKU` | $0.80 | $4.00 |

**`CostTracker` struct** — lock-free using `AtomicU64`:
- `input_tokens: AtomicU64`
- `output_tokens: AtomicU64`
- `cache_creation_tokens: AtomicU64`
- `cache_read_tokens: AtomicU64`
- `add_usage(input, output, cache_creation, cache_read)` — atomic adds
- `total_cost_usd(model: &str) -> f64` — loads atomics, looks up pricing by model substring match
- `summary(model: &str) -> String` — human-readable cost + token counts
- Implements `Default`

### Module: `hooks`

**`HookContext` struct:** `{ event: String, tool_name: Option<String>, tool_input: Option<Value>, tool_output: Option<String>, is_error: Option<bool>, session_id: Option<String> }`

**`HookOutcome` enum:** `Allowed`, `Blocked(String)`, `Modified(Value)`

**`run_hooks(hooks, event, context, working_dir) -> HookOutcome`** (async):
- Iterates `Vec<HookEntry>` for the given `HookEvent`
- Applies `tool_filter` (glob match against `tool_name`)
- Spawns shell command via `tokio::process::Command`
- Sends `HookContext` as JSON on stdin
- If `blocking: true` and exit code != 0, returns `HookOutcome::Blocked(stderr)`
- Otherwise returns `HookOutcome::Allowed`

### Relationship to TypeScript

`cc-core` corresponds to the scattered TypeScript files: `src/constants/`, `src/context.ts`, `src/history.ts`, `src/cost-tracker.ts`, `src/costHook.ts`, `src/schemas/hooks.ts`, and parts of `src/services/api/`. The permission modes, hook events, and config structure mirror the TypeScript `Config` type exactly.

---

## Crate: `cc-api`

**Path:** `crates/api/src/lib.rs`

Complete async Messages API client with SSE streaming support.

### Module: `types`

**`CreateMessageRequest` struct:** built via `CreateMessageRequestBuilder`:
- `model: String`
- `max_tokens: u32`
- `messages: Vec<ApiMessage>`
- `system: Option<SystemPrompt>`
- `tools: Option<Vec<ApiToolDefinition>>`
- `temperature: Option<f32>`
- `top_p: Option<f32>`
- `top_k: Option<u32>`
- `stop_sequences: Option<Vec<String>>`
- `thinking: Option<ThinkingConfig>`
- `stream: bool` (always set to `true` internally)

**`CreateMessageRequestBuilder`** — fluent builder:
- `CreateMessageRequest::builder(model, max_tokens) -> Self`
- `.messages(Vec<ApiMessage>)`
- `.system(SystemPrompt)` or `.system_text(String)`
- `.tools(Vec<ApiToolDefinition>)`
- `.temperature(f32)`, `.top_p(f32)`, `.top_k(u32)`
- `.stop_sequences(Vec<String>)`
- `.thinking(ThinkingConfig)`
- `.build() -> CreateMessageRequest`

**`ThinkingConfig`:** `{ type: "enabled", budget_tokens: u32 }`
- `ThinkingConfig::enabled(budget: u32) -> Self`

**`SystemPrompt` enum** (serde untagged):
- `Text(String)` — simple text system prompt
- `Blocks(Vec<SystemBlock>)` — structured blocks with cache control

**`SystemBlock`:** `{ type: "text", text: String, cache_control: Option<CacheControl> }`

**`CacheControl`:** `{ type: "ephemeral" }`
- `CacheControl::ephemeral() -> Self`

**`ApiMessage`:** `{ role: String, content: Value }`
- `From<&Message> for ApiMessage` — converts `cc_core::types::Message` to API format

**`ApiToolDefinition`:** `{ name: String, description: String, input_schema: Value, cache_control: Option<CacheControl> }`
- `From<&ToolDefinition> for ApiToolDefinition`
- Last tool in the list gets `cache_control: Some(CacheControl::ephemeral())` (prompt caching)

**`CreateMessageResponse`:** `{ id, type, role, content, model, stop_reason, stop_sequence, usage }`

**`ApiErrorResponse`:** `{ type: String, error: ApiErrorDetail }`

### Module: `streaming`

**`StreamEvent` enum** (serde `#[serde(tag = "type")]`):
- `MessageStart { message: CreateMessageResponse }`
- `MessageDelta { delta: MessageDeltaData, usage: Option<StreamUsage> }`
- `MessageStop`
- `ContentBlockStart { index: usize, content_block: PartialContentBlock }`
- `ContentBlockDelta { index: usize, delta: ContentDelta }`
- `ContentBlockStop { index: usize }`
- `Ping`
- `Error { error_type: String, message: String }`

**`ContentDelta` enum:**
- `TextDelta { text: String }`
- `InputJsonDelta { partial_json: String }`
- `ThinkingDelta { thinking: String }`
- `SignatureDelta { signature: String }`

**`StreamHandler` trait:**
- `fn on_event(&self, event: &StreamEvent)` — called for each SSE event

**`NullStreamHandler`** — no-op implementation for headless mode

**`StreamAccumulator`** — collects stream events into a complete message:
- `on_event(&mut self, event: &StreamEvent)` — processes all event types
- `finish(self) -> (Message, UsageInfo, Option<String>)` — returns (assistant_message, usage, stop_reason)

Internal `PartialBlock` enum during accumulation:
- `Text(String)`
- `ToolUse { id: String, name: String, json_buf: String }`
- `Thinking { thinking_buf: String, signature_buf: String }`

### Module: `sse_parser`

**`SseFrame` struct:** `{ event: Option<String>, data: Option<String> }`

**`SseLineParser`** — stateful line-by-line SSE parser:
- `feed_line(&mut self, line: &str) -> Option<SseFrame>`
- Handles `event:`, `data:`, and blank-line frame boundaries per SSE spec

### Module: `client`

**`ClientConfig` struct:**
- `api_key: String`
- `api_base: String` (default: `ANTHROPIC_API_BASE`)
- `timeout_secs: u64` (default: 600)
- `max_retries: u32` (default: 5)

**`AnthropicClient` struct:**
- `AnthropicClient::new(config: ClientConfig) -> Result<Self>` — validates API key, builds `reqwest::Client` with rustls-tls, sets `anthropic-version` and `anthropic-beta` headers
- `AnthropicClient::from_config(cfg: &Config) -> Result<Self>` — resolves key/base from Config

**`create_message(request) -> Result<CreateMessageResponse>`** — non-streaming POST to `/v1/messages`

**`create_message_stream(request, handler) -> Result<mpsc::Receiver<StreamEvent>>`** (async):
1. Sets `stream: true` on request
2. Spawns `tokio::spawn` background task calling `process_sse_stream()`
3. Returns `mpsc::Receiver<StreamEvent>` with channel buffer 256
4. Background task reads response body line by line via `SseLineParser`
5. Calls `frame_to_event()` to parse each frame
6. Sends to channel + calls `handler.on_event()`

**`send_with_retry(request_fn) -> Result<reqwest::Response>`** — exponential backoff:
- Max 5 retries
- Initial delay: 1 second
- Multiplier: 2× per retry, capped at 60 seconds
- Honors `Retry-After` response header (overrides backoff delay)
- Retries on 429 (`RateLimit`) and 529 (`ApiStatus` overloaded)

**`frame_to_event(frame: SseFrame) -> Option<StreamEvent>`** — dispatches by `frame.event`:
- `"ping"` → `StreamEvent::Ping`
- `"message_start"` → deserialize `data` into `StreamEvent::MessageStart`
- `"content_block_start"` → `StreamEvent::ContentBlockStart`
- `"content_block_delta"` → `StreamEvent::ContentBlockDelta`
- `"content_block_stop"` → `StreamEvent::ContentBlockStop`
- `"message_delta"` → `StreamEvent::MessageDelta`
- `"message_stop"` → `StreamEvent::MessageStop`
- `"error"` → `StreamEvent::Error`

### Module: `model_registry`

**`ModelRegistry`** — model catalogue with bundled snapshot and optional models.dev refresh:
- `ModelRegistry::new()` — pre-populated with Anthropic, OpenAI, and Google models
- `get(provider_id, model_id) -> Option<&ModelEntry>` — exact key lookup
- `find_entry_by_model(model) -> Option<&ModelEntry>` — accepts `"provider/model"` or bare name; searches exact match then prefix match
- `find_provider_for_model(model) -> Option<ProviderId>` — family-based heuristic + registry lookup
- `best_model_for_provider(provider_id) -> Option<String>` — flagship priority scoring

**`ModelEntry`:** `{ info: ModelInfo, cost_input, cost_output, tool_calling, reasoning, vision, family, status }`

**`ModelInfo`:** `{ id: ModelId, provider_id: ProviderId, name: String, context_window: u32, max_output_tokens: u32 }`

**Per-model `max_tokens` resolution:**

`effective_max_tokens_for_model(config, registry, model) -> u32`:
- User set `--max-tokens` + model known → `min(user_value, model.max_output_tokens)`
- User set `--max-tokens` + model unknown → user value as-is
- No override + model known → `model.max_output_tokens`
- No override + model unknown → `DEFAULT_MAX_TOKENS` (32 000)

Bundled model output limits:
| Model | max_output_tokens |
|---|---|
| Claude Opus 4.6 | 32 000 |
| Claude Sonnet 4.6 | 16 000 |
| Claude Haiku 4.5 | 8 096 |
| GPT-4o / GPT-4o mini | 16 384 |
| o3 / o4-mini | 100 000 |
| Gemini 2.5 Pro / Flash | 65 536 |
| Gemini 2.0 Flash | 8 192 |

`effective_model_for_config(config, registry) -> String` — resolves model: explicit user override → registry best-for-provider → hardcoded default.

### Relationship to TypeScript

Corresponds to `src/services/api/claude.ts`, `src/services/api/client.ts`, and `src/services/api/errorUtils.ts`. Implements the same SSE streaming protocol and retry logic. Prompt caching via `CacheControl::ephemeral()` mirrors the TypeScript cache control implementation. Beta header is identical.

---

## Crate: `cc-tools`

**Path:** `crates/tools/src/`

Implements all 33 built-in tools. Each tool is a zero-sized struct implementing the `Tool` trait.

### Core Types (`lib.rs`)

**`ToolResult` struct:**
- `content: String`
- `is_error: bool`
- `metadata: Option<Value>` — optional structured data for TUI rendering
- `ToolResult::success(content)` / `ToolResult::error(content)` / `.with_metadata(meta)`

**`PermissionLevel` enum:** `None`, `ReadOnly`, `Write`, `Execute`, `Dangerous`

**`ToolContext` struct:**
- `working_dir: PathBuf`
- `permission_mode: PermissionMode`
- `permission_handler: Arc<dyn PermissionHandler>`
- `cost_tracker: Arc<CostTracker>`
- `session_id: String`
- `non_interactive: bool`
- `mcp_manager: Option<Arc<cc_mcp::McpManager>>`
- `config: cc_core::config::Config`
- `resolve_path(&self, path: &str) -> PathBuf` — resolves relative paths against `working_dir`
- `check_permission(tool_name, description, is_read_only) -> Result<(), ClaudeError>`

**`Tool` trait** (async_trait):
- `fn name(&self) -> &str`
- `fn description(&self) -> &str`
- `fn permission_level(&self) -> PermissionLevel`
- `fn input_schema(&self) -> Value` — JSON Schema for tool parameters
- `async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult`
- `fn to_definition(&self) -> ToolDefinition` — default impl from above methods

**`all_tools() -> Vec<Box<dyn Tool>>`** — returns all 33 tools

**`find_tool(name: &str) -> Option<Box<dyn Tool>>`** — finds by exact name match

### Tool: `BashTool` (`bash.rs`)

**Name:** `"Bash"`
**Permission level:** `Execute`

Input schema: `{ command: string, timeout: optional u64 (seconds) }`

**Algorithm:**
1. Checks permission via `ctx.check_permission()`
2. On Windows: `cmd /C <command>`. On Unix: `bash -c <command>`
3. Default timeout: 120 seconds. Maximum: 600 seconds
4. Collects stdout and stderr with `tokio::io::BufReader`
5. Truncates output >100,000 characters with notice
6. Non-zero exit → `ToolResult::error` with combined stdout+stderr+exit_code
7. Zero exit → `ToolResult::success` with stdout (stderr appended if non-empty)

### Tool: `FileReadTool` (`file_read.rs`)

**Name:** `"Read"`
**Permission level:** `ReadOnly`

Input schema: `{ file_path: string, offset: optional u32 (1-based line), limit: optional u32 }`

**Algorithm:**
1. Resolves path via `ctx.resolve_path()`
2. Default limit: 2000 lines
3. Reads entire file, splits on newlines
4. Applies offset (1-based) and limit
5. Formats output as `{line_number}\t{content}`
6. Returns error on binary files (detected via `std::io::ErrorKind::InvalidData`)
7. Returns stub message for images and PDFs

### Tool: `FileEditTool` (`file_edit.rs`)

**Name:** `"Edit"`
**Permission level:** `Write`

Input schema: `{ file_path: string, old_string: string, new_string: string, replace_all: optional bool }`

**Algorithm:**
1. Validates `old_string != new_string`
2. Reads current file content
3. Counts occurrences of `old_string`
4. If `replace_all == false` (default) and count > 1: returns error (ambiguous)
5. If `replace_all == true`: uses `str::replace()` (replaces all)
6. If `replace_all == false` and count == 1: uses `str::replacen(old, new, 1)`
7. Writes updated content back to file

### Tool: `FileWriteTool` (`file_write.rs`)

**Name:** `"Write"`
**Permission level:** `Write`

Input schema: `{ file_path: string, content: string }`

**Algorithm:**
1. Resolves path
2. Creates parent directories via `tokio::fs::create_dir_all()`
3. Writes content to file
4. Reports line count and byte count in success message

### Tool: `GlobTool` (`glob_tool.rs`)

**Name:** `"Glob"`
**Permission level:** `ReadOnly`

Input schema: `{ pattern: string, path: optional string }`

**Algorithm:**
1. Resolves base path (defaults to `working_dir`)
2. Constructs full glob pattern by joining base + pattern
3. Uses `glob::glob()` crate for pattern matching
4. Sorts results by modification time (most recent first)
5. Returns max 250 results
6. Returns newline-separated list of relative paths

### Tool: `GrepTool` (`grep_tool.rs`)

**Name:** `"Grep"`
**Permission level:** `ReadOnly`

Input schema: `{ pattern: string, path: optional string, glob: optional string, type: optional string, output_mode: optional enum, context: optional u32, head_limit: optional u32, offset: optional u32, -i: optional bool, -n: optional bool, -A: optional u32, -B: optional u32, -C: optional u32, multiline: optional bool }`

**Algorithm:**
1. Compiles `RegexBuilder` with `case_insensitive` and `multi_line` flags
2. Uses `walkdir::WalkDir` to traverse directory tree
3. Skips hidden directories, `node_modules/`, `target/`, `__pycache__/`, `.git/`
4. Filters by glob pattern or file type extension mapping
5. Three output modes:
   - `files_with_matches` — list of file paths (default)
   - `content` — matching lines with optional context (-A/-B/-C)
   - `count` — match counts per file
6. Applies `head_limit` and `offset` pagination

**Type shortcuts** (e.g., `type="js"` → `["js", "jsx", "mjs", "cjs"]`):
- `js`, `ts`, `py`, `rs`, `go`, `java`, `rb`, `cpp`, `c`, `cs`, `php`, `swift`, `kt`, `html`, `css`, `json`, `yaml`, `md`

### Tool: `WebFetchTool` (`web_fetch.rs`)

**Name:** `"WebFetch"`
**Permission level:** `ReadOnly`

Input schema: `{ url: string, prompt: optional string }`

**Algorithm:**
1. `reqwest` GET with 30-second timeout, 10 redirect limit
2. User-Agent: `"Claude-Code/1.0"`
3. If HTML content-type: runs `strip_html()` — manual state machine removing tags, scripts, styles; converts `&amp;`, `&lt;`, `&gt;`, `&nbsp;` entities
4. Truncates content >100,000 characters
5. Returns text content

### Tool: `WebSearchTool` (`web_search.rs`)

**Name:** `"WebSearch"`
**Permission level:** `ReadOnly`

Input schema: `{ query: string, num_results: optional u32 (default 5) }`

**Algorithm:**
1. Checks `BRAVE_SEARCH_API_KEY` env var:
   - If set: calls Brave Search API at `https://api.search.brave.com/res/v1/web/search`
   - Returns title + URL + description for each result
2. Fallback: DuckDuckGo Instant Answer API at `https://api.duckduckgo.com/?q=...&format=json`
3. Returns up to `num_results` formatted results

### Tool: `NotebookEditTool` (`notebook_edit.rs`)

**Name:** `"NotebookEdit"`
**Permission level:** `Write`

Input schema: `{ notebook_path: string, cell_id: optional string, cell_index: optional u32, source: optional string, cell_type: optional string, mode: string (replace|insert|delete) }`

**Algorithm:**
1. Parses `.ipynb` JSON with `serde_json`
2. Cell lookup: by UUID string OR by `cell-N` index pattern
3. **replace mode:** updates `source`, resets `outputs = []`, `execution_count = null`
4. **insert mode:** inserts new cell at given index or after cell_id; generates 8-char hex cell ID from `timestamp XOR random`
5. **delete mode:** removes cell by id/index
6. Writes updated notebook back to file

### Tool: `TaskCreateTool`, `TaskGetTool`, `TaskUpdateTool`, `TaskListTool`, `TaskStopTool`, `TaskOutputTool` (`tasks.rs`)

Global store: `TASK_STORE: Lazy<Arc<DashMap<String, Task>>>`

**`Task` struct:** `{ id: String, subject: String, description: String, status: TaskStatus, owner: Option<String>, blocks: Vec<String>, blocked_by: Vec<String>, metadata: HashMap<String, Value>, output: Vec<String>, created_at: DateTime<Utc>, updated_at: DateTime<Utc> }`

**`TaskStatus` enum:** `Pending`, `InProgress`, `Completed`, `Deleted`, `Running`, `Failed`

| Tool | Name | Description |
|---|---|---|
| `TaskCreateTool` | `"TaskCreate"` | Creates task with UUID, stores in `TASK_STORE` |
| `TaskGetTool` | `"TaskGet"` | Returns task JSON by ID |
| `TaskUpdateTool` | `"TaskUpdate"` | Updates task fields; `status=deleted` removes from store |
| `TaskListTool` | `"TaskList"` | Lists all non-deleted tasks with optional status filter |
| `TaskStopTool` | `"TaskStop"` | Sets task status to `Failed` |
| `TaskOutputTool` | `"TaskOutput"` | Appends text to task `output` vector |

### Tool: `CronCreateTool`, `CronDeleteTool`, `CronListTool` (`cron.rs`)

Global store: `CRON_STORE: Lazy<Arc<RwLock<HashMap<String, CronTask>>>>`

**`CronTask` struct:** `{ id: String, cron: String, prompt: String, recurring: bool, durable: bool, created_at: DateTime<Utc> }`

| Tool | Name | Description |
|---|---|---|
| `CronCreateTool` | `"CronCreate"` | Creates scheduled task; validates cron expression; if `durable=true`, persists to `.claude/scheduled_tasks.json`; max 50 jobs |
| `CronDeleteTool` | `"CronDelete"` | Removes task by ID from store (and disk if durable) |
| `CronListTool` | `"CronList"` | Lists all scheduled tasks with human-readable schedule |

**`cron_matches(cron: &str, now: &DateTime<Local>) -> bool`:**
- Parses 5-field cron: minute, hour, day-of-month, month, day-of-week
- Supports: `*`, `*/N` (step), `N-M` (range), `N,M,...` (list)

**`validate_cron(cron: &str) -> Result<()>`** — validates field ranges (minute 0–59, hour 0–23, etc.)

**`cron_to_human(cron: &str) -> String`** — describes schedule in plain English

**`pop_due_tasks() -> Vec<CronTask>`** — returns tasks matching current time, removes non-recurring tasks from store

### Tool: `TodoWriteTool` (`todo_write.rs`)

**Name:** `"TodoWrite"`
**Permission level:** `None`

Input schema: `{ todos: Array<{ id: string, content: string, status: string, priority: string }> }`

Replaces entire todo list. Returns summary with counts of pending/in_progress/completed items.

### Tool: `AskUserQuestionTool` (`ask_user.rs`)

**Name:** `"AskUserQuestion"`
**Permission level:** `None`

Input schema: `{ question: string, options: optional Array<string> }`

In `non_interactive` mode: returns error "Cannot prompt user in non-interactive mode".
Otherwise: returns `ToolResult::success("")` with metadata `{ type: "ask_user", question, options }` for TUI layer to handle.

### Tool: `EnterPlanModeTool` (`enter_plan_mode.rs`)

**Name:** `"EnterPlanMode"`
**Permission level:** `None`

Returns `ToolResult::success` with metadata `{ type: "enter_plan_mode" }`. Signals the session to switch to Plan permission mode.

### Tool: `ExitPlanModeTool` (`exit_plan_mode.rs`)

**Name:** `"ExitPlanMode"`
**Permission level:** `None`

Input schema: `{ summary: optional string }`

Returns success with metadata `{ type: "exit_plan_mode", summary }`. Signals return from Plan mode.

### Tool: `PowerShellTool` (`powershell.rs`)

**Name:** `"PowerShell"`
**Permission level:** `Execute`

Input schema: `{ command: string, timeout: optional u64 }`

Same execution pattern as `BashTool`. On Windows uses `powershell -NoProfile -NonInteractive -Command`. On other platforms uses `pwsh`.

### Tool: `EnterWorktreeTool`, `ExitWorktreeTool` (`worktree.rs`)

Global: `WORKTREE_SESSION: Lazy<Arc<RwLock<Option<WorktreeSession>>>>`

**`WorktreeSession`:** `{ branch: String, path: PathBuf, original_dir: PathBuf }`

**`EnterWorktreeTool`** (`"EnterWorktree"`):
- Input: `{ branch: string, path: optional string }`
- Runs `git worktree add -b <branch> <path>`
- Saves session to `WORKTREE_SESSION`

**`ExitWorktreeTool`** (`"ExitWorktree"`):
- Input: `{ action: "keep" | "remove", discard_changes: optional bool }`
- `keep`: locks worktree, clears session
- `remove`: checks for uncommitted changes (requires `discard_changes=true` to override), runs `git worktree remove --force <path>`, then `git branch -D <branch>`

### Tool: `SendMessageTool` (`send_message.rs`)

**Name:** `"SendMessage"`
**Permission level:** `None`

Global: `INBOX: Lazy<DashMap<String, Vec<AgentMessage>>>`

Input schema: `{ to: string, message: string, metadata: optional Value }`

- Delivers message to named recipient in `INBOX`
- `to = "*"` broadcasts to all existing keys
- `drain_inbox(recipient: &str) -> Vec<AgentMessage>` — removes and returns all messages
- `peek_inbox(recipient: &str) -> Vec<AgentMessage>` — returns without removing

### Tool: `SkillTool` (`skill_tool.rs`)

**Name:** `"Skill"`
**Permission level:** `None`

Input schema: `{ skill: string, arguments: optional string }`

**Algorithm:**
1. `skill = "list"` → enumerates `.claude/commands/*.md` and `~/.claude/commands/*.md`, extracts description from YAML frontmatter or first heading
2. Otherwise: resolves `<skill>.md` file from project then user commands directory
3. Strips YAML frontmatter (`---` block)
4. Substitutes `$ARGUMENTS` with provided arguments string
5. Returns file content as `ToolResult::success`

### Tool: `SleepTool` (`sleep.rs`)

**Name:** `"Sleep"`
**Permission level:** `None`

Input schema: `{ duration: f64 (seconds) }`

Calls `tokio::time::sleep(Duration::from_secs_f64(duration))`. Maximum 300 seconds.

### Tool: `ToolSearchTool` (`tool_search.rs`)

**Name:** `"ToolSearch"`
**Permission level:** `None`

Input schema: `{ query: string, max_results: optional u32 (default 5) }`

Static `TOOL_CATALOG: &[(&str, &str, &[&str])]` — 32 entries of `(name, description, keywords)`.

**Scoring algorithm:**
- `select:Name` syntax → score 100 for exact name match
- Otherwise for each catalog entry:
  - exact name match: +20
  - name contains query: +10
  - description contains query: +5
  - keyword exact match: +8
  - keyword contains query: +3
- Returns top `max_results` entries with non-zero score

### Tool: `BriefTool` (`brief.rs`)

**Name:** `"Brief"`
**Permission level:** `None`

Input schema: `{ message: string, status: optional string, attachments: optional Array<string> (file paths) }`

Resolves attachment metadata (file size, is_image flag from extension). Returns `ToolResult::success("")` with metadata `{ message, status, sentAt, attachments: [{ path, size, isImage }] }`.

### Tool: `ConfigTool` (`config_tool.rs`)

**Name:** `"Config"`
**Permission level:** `None`

Input schema: `{ action: "get" | "set", key: string, value: optional Value }`

Reads/writes `~/.claude/settings.json`. Supported keys: `model`, `max_tokens`, `verbose`, `permission_mode`, `auto_compact`. Returns current value on `get`, writes and confirms on `set`.

### Tool: `ListMcpResourcesTool`, `ReadMcpResourceTool` (`mcp_resources.rs`)

| Tool | Name | Description |
|---|---|---|
| `ListMcpResourcesTool` | `"ListMcpResources"` | Calls `ctx.mcp_manager.list_all_resources()`, returns JSON |
| `ReadMcpResourceTool` | `"ReadMcpResource"` | Input: `{ uri: string }`. Calls `ctx.mcp_manager.read_resource(uri)` |

Both return error if `ctx.mcp_manager` is `None`.

### Relationship to TypeScript

`cc-tools` corresponds to the TypeScript tool implementations in `src/` (e.g., bash is in the tool system, file operations in ReadTool/EditTool/WriteTool, etc.). Tool names are identical to the TypeScript constants. The `ToolContext` mirrors the TypeScript `ToolUseContext`.

---

## Crate: `cc-query`

**Path:** `crates/query/src/`

The core agentic query loop crate. Contains 4 source files.

### Module: `lib.rs` — Main Query Loop

**`QueryOutcome` enum:**
- `EndTurn { message: Message, usage: UsageInfo }` — model issued `end_turn`
- `MaxTokens { partial_message: Message, usage: UsageInfo }` — hit token limit
- `Cancelled` — cancellation token fired
- `Error(ClaudeError)` — unrecoverable error

**`QueryConfig` struct:**
- `model: String`
- `max_tokens: u32`
- `max_turns: u32` (default: `MAX_TURNS_DEFAULT = 10`)
- `system_prompt: Option<String>`
- `append_system_prompt: Option<String>`
- `thinking_budget: Option<u32>`
- `temperature: Option<f32>`
- `QueryConfig::default()` uses `DEFAULT_MODEL` + `DEFAULT_MAX_TOKENS`
- `QueryConfig::from_config(cfg: &Config)` — reads model + max_tokens from Config
- `QueryConfig::from_config_with_registry(cfg, registry)` — resolves model via registry and sets `max_tokens` per-model using `effective_max_tokens_for_model()`

**`QueryEvent` enum:**
- `Stream(StreamEvent)` — raw API stream event
- `ToolStart { tool_name, tool_id }` — tool beginning execution
- `ToolEnd { tool_name, tool_id, result, is_error }` — tool completed
- `TurnComplete { turn: u32, stop_reason: String }` — model turn finished
- `Status(String)` — informational message
- `Error(String)` — error notification

**`run_query_loop(client, messages, tools, tool_ctx, config, cost_tracker, event_tx, cancel_token) -> QueryOutcome`** (async):

Main agentic loop:
1. Increments turn counter; returns `EndTurn` if `> max_turns`
2. Checks `cancel_token.is_cancelled()` → `Cancelled`
3. Converts `messages` → `Vec<ApiMessage>`, tools → `Vec<ApiToolDefinition>`
4. Calls `build_system_prompt(config)` to construct `SystemPrompt`
5. Builds `CreateMessageRequest` (with thinking config if `budget` provided)
6. Creates `ChannelStreamHandler` or `NullStreamHandler`
7. Calls `client.create_message_stream()`, receives `mpsc::Receiver<StreamEvent>`
8. Inner loop: `tokio::select!` on cancellation or stream events; feeds `StreamAccumulator`
9. On `MessageStop` or channel close: calls `accumulator.finish()`
10. Tracks costs via `cost_tracker.add_usage()`
11. Appends assistant message to `messages`
12. Calls `auto_compact_if_needed()` if stop reason is `end_turn` or `tool_use`
13. Dispatches on `stop_reason`:
    - `"end_turn"` / `"stop_sequence"` / unknown → fires `Stop` hook → returns `EndTurn`
    - `"max_tokens"` → returns `MaxTokens`
    - `"tool_use"` → executes all tool_use blocks (see below), appends results, `continue`

**Tool execution in `tool_use` turn:**
1. For each `ContentBlock::ToolUse { id, name, input }`:
2. Emits `QueryEvent::ToolStart`
3. Fires `PreToolUse` hooks via `cc_core::hooks::run_hooks()`; if `HookOutcome::Blocked` → `ToolResult::error("Blocked by hook: ...")`
4. Otherwise calls `execute_tool(name, input, tools, ctx)`
5. Fires `PostToolUse` hooks
6. Emits `QueryEvent::ToolEnd`
7. Pushes `ContentBlock::ToolResult` to result_blocks
8. Appends `Message::user_blocks(result_blocks)` to conversation

**`execute_tool(name, input, tools, ctx) -> ToolResult`** (async):
- Finds tool by name in slice, calls `tool.execute(input, ctx)`
- Unknown tool → `ToolResult::error("Unknown tool: {name}")`

**`build_system_prompt(config) -> SystemPrompt`:**
- Joins `system_prompt` and `append_system_prompt` with `\n\n`
- Empty → default `"You are Claude, an AI assistant by Anthropic."`

**`run_single_query(client, messages, config) -> Result<Message>`** (async):
- Single API call, no tool loop, `NullStreamHandler`
- Returns complete assistant message

**`ChannelStreamHandler`** — implements `StreamHandler`:
- `on_event(&self, event)` forwards to `mpsc::UnboundedSender<QueryEvent>`

### Module: `compact.rs` — Auto-Compact

**Constants:**
- `AUTOCOMPACT_BUFFER_TOKENS = 13_000`
- `WARNING_THRESHOLD_BUFFER_TOKENS = 20_000`
- `AUTOCOMPACT_TRIGGER_FRACTION = 0.90`
- `KEEP_RECENT_MESSAGES = 10`
- `MAX_CONSECUTIVE_FAILURES = 3`

**`AutoCompactState` struct:**
- `compaction_count: u32`
- `consecutive_failures: u32`
- `disabled: bool` — circuit breaker; set after 3 consecutive failures

**`TokenWarningState` enum:** `Ok`, `Warning`, `Critical`

**`context_window_for_model(model: &str) -> u32`:**
- `200_000` for models matching "opus-4", "sonnet-4", "haiku-4", "claude-3-5"
- `100_000` otherwise

**`calculate_token_warning_state(input_tokens, model) -> TokenWarningState`:**
- Uses `WARNING_THRESHOLD_BUFFER_TOKENS` to determine Warning vs Critical

**`should_auto_compact(state, input_tokens, model) -> bool`:**
- Returns false if `state.disabled`
- Returns true if `input_tokens / context_window > AUTOCOMPACT_TRIGGER_FRACTION`

**`summarise_head(client, messages_to_summarize, model) -> Result<String>`** (async):
- Calls API with prompt asking to summarize the provided conversation
- Returns summary wrapped in `<compact-summary>...</compact-summary>` XML tags

**`compact_conversation(client, messages, model) -> Result<Vec<Message>>`** (async):
- Splits conversation: head = `messages[0..total-KEEP_RECENT_MESSAGES]`, tail = last 10 messages
- Calls `summarise_head()` on head
- Returns `[Message::user(summary)] + tail`

**`auto_compact_if_needed(client, messages, input_tokens, model, state) -> Option<Vec<Message>>`** (async):
- Checks `should_auto_compact()`, calls `compact_conversation()`
- On success: resets `consecutive_failures`, increments `compaction_count`
- On failure: increments `consecutive_failures`; disables if `>= MAX_CONSECUTIVE_FAILURES`
- Returns `Some(new_messages)` on success, `None` if not needed or failed

### Module: `agent_tool.rs` — Sub-Agent Tool

**`AgentTool`** implements `Tool`:
- **Name:** `"Task"` (constant `TOOL_NAME_AGENT`)
- **Permission level:** `Execute`

Input schema: `{ description: string, prompt: string, tools: optional Array<string>, system_prompt: optional string, max_turns: optional u32, model: optional string }`

**Algorithm:**
1. Creates dedicated `AnthropicClient` from `ANTHROPIC_API_KEY` env var
2. Filters tool list: if `tools` field provided, uses that subset; always excludes `TOOL_NAME_AGENT` (prevents recursion)
3. Calls `run_query_loop()` with:
   - `event_tx = None` (no TUI forwarding for sub-agent)
   - New `ToolContext` with same working_dir, permission_mode, etc.
4. Returns final assistant message text as `ToolResult::success()`

### Module: `cron_scheduler.rs` — Background Cron

**`start_cron_scheduler(tools, tool_ctx, cancel_token) -> JoinHandle<()>`:**
- Spawns `tokio::spawn(run_scheduler_loop(...))`

**`run_scheduler_loop(tools, tool_ctx, cancel_token)`** (async loop):
1. Computes seconds until next minute boundary: `sleep(60 - now.second() + 1)`
2. Calls `cc_tools::cron::pop_due_tasks()` to get matching tasks
3. For each due task: spawns `run_query_loop()` with:
   - Single user message from `task.prompt`
   - `event_tx = None` (background, no UI)
   - `cancel_token` clone
4. Loop continues until cancellation

### Relationship to TypeScript

`cc-query` corresponds to `src/query.ts`, `src/query/`, `src/services/compact/autoCompact.ts`, `src/coordinator/`, and parts of `src/services/autoDream/`. The `AgentTool` corresponds to the TypeScript `Task` tool.

---

## Crate: `cc-tui`

**Path:** `crates/tui/src/lib.rs`

Terminal UI built on `ratatui` + `crossterm`. Replaces the TypeScript `ink`/React rendering layer.

### `App` struct

```
config: Config
cost_tracker: Arc<CostTracker>
messages: Vec<(Role, String)>
input: String
input_history: Vec<String>
history_index: Option<usize>
scroll_offset: u16
is_streaming: bool
streaming_text: String
status_message: Option<String>
should_quit: bool
show_help: bool
```

### Key Methods

**`handle_key_event(&mut self, key: KeyEvent) -> Option<String>`:**
- `Ctrl+C`: if streaming → cancels; if input empty → quits; else clears input
- `Ctrl+D`: if input empty → quits
- Character input: appended to `self.input`
- `Backspace`: removes last char from `self.input`
- `Enter`: returns `Some(input)` for caller to process (empty input ignored)
- `Up`/`Down`: navigates `input_history`
- `PageUp`/`PageDown`: adjusts `scroll_offset`
- `F1` / `?`: toggles help overlay

**`handle_query_event(&mut self, event: QueryEvent)`:**
- `Stream(ContentBlockDelta::TextDelta)` → appends to `streaming_text`
- `ToolStart { tool_name, .. }` → sets `status_message = "Running {tool_name}..."`
- `ToolEnd { .. }` → clears `status_message`
- `TurnComplete { .. }` → moves `streaming_text` into `messages`, clears streaming state
- `Status(msg)` → sets `status_message`
- `Error(msg)` → sets `status_message` with error prefix

**`take_input(&mut self) -> String`:**
- Returns and clears `self.input`
- Pushes to `input_history` (dedup at head)

**`add_message(&mut self, role: Role, text: String)`** — appends to messages vec

### Module: `render`

**`render_app(f: &mut Frame, app: &App)`:**
- Splits terminal into 3 vertical chunks via `Layout::vertical`:
  1. Messages area (flex fill)
  2. Input area (3 rows)
  3. Status bar (1 row)
- **Messages:** renders each `(role, text)` pair — `Role::User` in Cyan, `Role::Assistant` in Green. If streaming, appends partial `streaming_text` in Yellow italic
- **Input area:** bordered `Block` titled "Input"; shows `self.input` with cursor `_` appended
- **Status bar:** shows `{model} | {cost_summary}` in Dark Gray

### Module: `widgets`

**`render_permission_dialog(f: &mut Frame, question: &str, options: &[String])`:**
- Centered popup dialog
- Shows question text
- Lists numbered options
- Renders as `Clear` + `Block` + `Paragraph` overlay

**`render_spinner(f: &mut Frame, area: Rect, frame_count: u64)`:**
- Cycles through braille spinner characters: `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`
- Indexed by `frame_count % 10`

### Module: `input`

**`is_slash_command(input: &str) -> bool`** — returns true if starts with `"/"`

**`parse_slash_command(input: &str) -> (&str, &str)`** — splits `"/name args"` → `("name", "args")`

### Terminal Setup

**`setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>>`:**
1. `enable_raw_mode()` (crossterm)
2. `execute!(stdout, EnterAlternateScreen)` (crossterm)
3. Creates `Terminal::new(CrosstermBackend::new(stdout))`

**`restore_terminal(terminal: &mut Terminal<...>)`:**
1. `disable_raw_mode()`
2. `execute!(stdout, LeaveAlternateScreen)`
3. `terminal.show_cursor()`

### Relationship to TypeScript

`cc-tui` replaces the entire TypeScript `src/ink/` rendering system, `src/components/`, and React/Ink component tree. The ratatui approach is fundamentally different (immediate-mode rendering vs React reconciler), but provides equivalent visual functionality: message history, streaming text, input box, status bar, permission dialogs.

---

## Crate: `cc-commands`

**Path:** `crates/commands/src/lib.rs`

Slash command implementations for the interactive REPL.

### Core Types

**`CommandContext` struct:**
```
config: Config
cost_tracker: Arc<CostTracker>
messages: Vec<Message>
working_dir: PathBuf
```

**`CommandResult` enum:**
- `Message(String)` — display text to user
- `UserMessage(String)` — inject as user message into conversation
- `ConfigChange(Config)` — update running config
- `ClearConversation` — clear message history
- `SetMessages(Vec<Message>)` — replace message history
- `Exit` — terminate session
- `Silent` — no output
- `Error(String)` — display error

**`SlashCommand` trait** (async_trait):
- `fn name(&self) -> &str`
- `fn aliases(&self) -> Vec<&str>` (default: empty)
- `fn description(&self) -> &str`
- `fn help(&self) -> &str`
- `fn hidden(&self) -> bool` (default: false)
- `async fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult`

### Command Registry

**`all_commands() -> Vec<Box<dyn SlashCommand>>`** — returns all built-in commands

**`find_command(name: &str) -> Option<Box<dyn SlashCommand>>`** — matches by name or alias

**`execute_command(input: &str, ctx: &CommandContext) -> Option<CommandResult>`** (async):
- Parses slash command from input
- Finds matching command
- Returns `None` if no match (pass-through to query loop)

### Implemented Commands

| Struct | Name | Aliases | Description |
|---|---|---|---|
| `HelpCommand` | `help` | `h`, `?` | List available slash commands |
| `ClearCommand` | `clear` | `cls` | Clear conversation history |
| `CompactCommand` | `compact` | — | Manually compact conversation |
| `CostCommand` | `cost` | — | Show current session cost |
| `ExitCommand` | `exit` | `quit`, `q` | Exit the REPL |
| `ModelCommand` | `model` | — | Show/change current model |
| `ConfigCommand` | `config` | — | Show/update configuration |
| `VersionCommand` | `version` | — | Show version information |
| `ResumeCommand` | `resume` | — | Resume previous conversation |
| `StatusCommand` | `status` | — | Show session status |
| `DiffCommand` | `diff` | — | Show file diffs |
| `MemoryCommand` | `memory` | — | Manage CLAUDE.md memories |
| `BugCommand` | `bug` | — | File a bug report |
| `DoctorCommand` | `doctor` | — | Run diagnostics |
| `LoginCommand` | `login` | — | Authenticate |
| `LogoutCommand` | `logout` | — | Clear authentication |
| `InitCommand` | `init` | — | Initialize project CLAUDE.md |
| `ReviewCommand` | `review` | — | Code review workflow |
| `HooksCommand` | `hooks` | — | Manage event hooks |
| `McpCommand` | `mcp` | — | Manage MCP servers |
| `PermissionsCommand` | `permissions` | — | Show/edit permissions |
| `PlanCommand` | `plan` | — | Enter/exit plan mode |
| `TasksCommand` | `tasks` | — | View background tasks |
| `SessionCommand` | `session` | — | Session management |
| `ThinkingCommand` | `thinking` | — | Toggle extended thinking |
| `ExportCommand` | `export` | — | Export conversation |
| `SkillsCommand` | `skills` | — | List/manage skills |
| `RewindCommand` | `rewind` | — | Rewind conversation state |
| `StatsCommand` | `stats` | — | Show usage statistics |
| `FilesCommand` | `files` | — | List context files |
| `RenameCommand` | `rename` | — | Rename current session |
| `EffortCommand` | `effort` | — | Set effort/thinking level |
| `SummaryCommand` | `summary` | — | Summarize conversation |
| `CommitCommand` | `commit` | — | Run git commit workflow |

### Relationship to TypeScript

`cc-commands` corresponds to the TypeScript `src/commands/` directory (150+ files). Each TypeScript command module (e.g., `src/commands/compact/`, `src/commands/model/`) maps to a struct in this crate. The slash command names and behaviors are preserved.

---

## Crate: `cc-mcp`

**Path:** `crates/mcp/src/lib.rs`

Full MCP (Model Context Protocol) client implementation. Uses JSON-RPC 2.0 over stdio subprocess transport.

### JSON-RPC Types

**`JsonRpcRequest`:** `{ jsonrpc: "2.0", method: String, params: Option<Value>, id: Option<u64> }`
- `JsonRpcRequest::new(method, params, id)` — regular request
- `JsonRpcRequest::notification(method, params)` — no id

**`JsonRpcResponse`:** `{ jsonrpc: "2.0", id: Option<u64>, result: Option<Value>, error: Option<JsonRpcError> }`

**`JsonRpcError`:** `{ code: i32, message: String, data: Option<Value> }`

### MCP Protocol Types

**`InitializeParams`:** `{ protocol_version: "2024-11-05", capabilities: ClientCapabilities, client_info: ClientInfo }`

**`ClientCapabilities`:** `{ roots: Option<RootsCapability> }`

**`InitializeResult`:** `{ protocol_version: String, capabilities: ServerCapabilities, server_info: ServerInfo }`

**`ServerCapabilities`:** `{ tools: Option<ToolsCapability>, resources: Option<ResourcesCapability>, prompts: Option<PromptsCapability> }`

**`McpTool`:** `{ name: String, description: Option<String>, input_schema: Value }`
- `From<&McpTool> for ToolDefinition` — converts to cc-core ToolDefinition

**`CallToolParams`:** `{ name: String, arguments: Option<Value> }`

**`CallToolResult`:** `{ content: Vec<McpContent>, is_error: Option<bool> }`

**`McpContent` enum** (serde tagged):
- `Text { type: "text", text: String }`
- `Image { type: "image", data: String, mime_type: String }`
- `Resource { type: "resource", resource: ResourceContents }`

**`McpResource`:** `{ uri: String, name: String, description: Option<String>, mime_type: Option<String> }`

**`McpPrompt`:** `{ name: String, description: Option<String>, arguments: Option<Vec<McpPromptArgument>> }`

### Transport

**`McpTransport` trait** (async_trait):
- `async fn send(&mut self, request: &JsonRpcRequest) -> Result<()>`
- `async fn recv(&mut self) -> Result<Option<JsonRpcResponse>>`
- `async fn close(&mut self)`

**`StdioTransport`:**
- `StdioTransport::spawn(command: &str, args: &[String], env: &HashMap<String, String>) -> Result<Self>`
  - Spawns subprocess with piped stdin/stdout
  - Spawns background reader task forwarding lines to `mpsc::UnboundedReceiver<String>`
- `send()` — serializes to JSON + newline on stdin
- `recv()` — receives from channel, deserializes JSON-RPC response

### `McpClient`

**`McpClient::connect_stdio(config: &McpServerConfig) -> Result<Self>`** (async):
1. Calls `StdioTransport::spawn()`
2. Calls `initialize()` — sends `initialize` request, receives `InitializeResult`
3. Sends `notifications/initialized` notification
4. If `capabilities.tools` present: calls `tools/list`, stores in `self.tools`
5. If `capabilities.resources` present: calls `resources/list`, stores
6. If `capabilities.prompts` present: calls `prompts/list`, stores
7. Returns connected client

**`call<T: DeserializeOwned>(&mut self, method, params) -> Result<T>`:**
- Sequential request/response: sends request with incrementing ID
- Calls `transport.recv()` in loop until response ID matches
- Deserializes `result` field

**`call_tool(&mut self, name: &str, arguments: Option<Value>) -> Result<CallToolResult>`:**
- Calls `tools/call` with `CallToolParams`

**`list_resources(&mut self) -> Result<Vec<McpResource>>`** — `resources/list`

**`read_resource(&mut self, uri: &str) -> Result<ResourceContents>`** — `resources/read`

### `McpManager`

Manages multiple named MCP server connections.

**`McpManager::connect_all(configs: &[McpServerConfig]) -> Result<Self>`** (async):
- Attempts to connect each server; logs warnings on failure (doesn't abort)

**`all_tool_definitions(&self) -> Vec<ToolDefinition>`:**
- Prefixes each tool name with `"{server_name}_"` to namespace tools

**`call_tool(&self, prefixed_name: &str, arguments: Option<Value>) -> Result<CallToolResult>`:**
- Strips server prefix to identify server
- Routes to correct `McpClient`

**`list_all_resources(&self) -> Result<Vec<McpResource>>`:**
- Aggregates resources from all connected servers

**`read_resource(&self, uri: &str) -> Result<ResourceContents>`:**
- Tries each server until one returns a result

**`server_count(&self) -> usize`**, **`server_names(&self) -> Vec<String>`**

**`mcp_result_to_string(result: &CallToolResult) -> String`:**
- Converts `McpContent::Text` → text, `McpContent::Image` → `[image: mime_type]`, `McpContent::Resource` → URI/text

### Relationship to TypeScript

`cc-mcp` corresponds to `src/services/mcpClient.ts` (TypeScript MCP implementation). Implements the same MCP protocol version (`2024-11-05`), stdio transport, and tool namespacing convention.

---

## Crate: `cc-bridge`

**Path:** `crates/bridge/src/lib.rs`

Implements the bridge protocol connecting the local Claude Code CLI to the claude.ai web UI. Enables remote control of the CLI from a browser session.

### Configuration

**`BridgeConfig` struct:**
- `enabled: bool`
- `server_url: String`
- `device_id: String`
- `session_token: Option<String>`
- `polling_interval_ms: u64` (default: 1000)
- `max_reconnect_attempts: u32` (default: 10)

### Protocol Types

**`BridgeMessage` enum** (serde tagged — messages from server to client):
- `UserMessage { content: String, attachments: Vec<String> }`
- `PermissionResponse { tool_use_id: String, decision: PermissionDecision }`
- `Cancel`
- `Ping`

**`BridgeEvent` enum** (serde tagged — events from client to server):
- `TextDelta { text: String }`
- `ToolStart { tool_name: String, tool_id: String }`
- `ToolEnd { tool_name: String, tool_id: String, result: String, is_error: bool }`
- `PermissionRequest { tool_use_id: String, tool_name: String, description: String }`
- `TurnComplete { stop_reason: String }`
- `Error { message: String }`
- `Pong`

**`PermissionDecision` enum:** `Allow`, `AllowPermanently`, `Deny`, `DenyPermanently`

**`BridgeState` enum:** `Connecting`, `Connected`, `Reconnecting { attempt: u32 }`, `Disconnected`

### Session Management

**`BridgeSession::new(config: BridgeConfig) -> (Self, mpsc::Receiver<BridgeMessage>, mpsc::Sender<BridgeEvent>)`:**
- Creates channel pair for bidirectional communication

**`BridgeManager::start(config, msg_tx, event_rx) -> Self`:**
- Spawns `run_poll_loop()` background task
- Returns manager with `JoinHandle`

### Polling Loop

**`run_poll_loop(config, msg_tx, event_rx)`** (async):
1. Long-polls `{server_url}/sessions/{id}/poll` with `reqwest` GET
2. On response: deserializes `BridgeMessage` array, sends each to `msg_tx`
3. Drains `event_rx`: sends accumulated `BridgeEvent` items to `{server_url}/sessions/{id}/events` via POST
4. On network error: exponential backoff up to `max_reconnect_attempts`
5. On 401/403: sets state to `Disconnected`, exits loop

### Module: `jwt`

**`JwtClaims` struct:** `{ sub: String, exp: u64, iat: u64, device_id: String }`

**`decode_payload(token: &str) -> Result<JwtClaims>`:**
- Splits token by `"."`, takes index 1 (payload segment)
- Base64 decodes (URL-safe, no padding) via `base64` crate
- Deserializes JSON to `JwtClaims`

**`is_expired(claims: &JwtClaims) -> bool`:**
- Compares `claims.exp` against `SystemTime::now()` Unix timestamp

### Module: `trusted_device`

**`device_fingerprint() -> String`:**
- Collects: `hostname()` (from `hostname` crate), `USER` env var, home directory path
- SHA-256 hash of concatenated string via `sha2` crate
- Returns lowercase hex string (first 16 chars) via `hex` crate

### Relationship to TypeScript

`cc-bridge` corresponds to `src/bridge/` (31 TypeScript files including `bridgeMain.ts`, `bridgeMessaging.ts`, `replBridge.ts`, `jwtUtils.ts`, `trustedDevice.ts`, etc.). Implements the same polling-based bridge protocol and JWT handling.

---

## Crate: `claude-code` (CLI Binary)

**Path:** `crates/cli/src/main.rs`

Binary entry point. Produces the `claude` executable. Wires all crates together.

### CLI Arguments (`Cli` struct via clap derive)

| Flag | Type | Description |
|---|---|---|
| `prompt` | `Option<String>` (positional) | Non-interactive prompt |
| `-p, --print` | `bool` | Print mode (alias for non-interactive) |
| `-m, --model` | `Option<String>` | Override model |
| `--permission-mode` | `Option<CliPermissionMode>` | Permission mode |
| `--resume` | `Option<String>` | Resume session by ID |
| `--max-turns` | `u32` (default: 10) | Max conversation turns |
| `-s, --system-prompt` | `Option<String>` | Override system prompt |
| `--append-system-prompt` | `Option<String>` | Append to system prompt |
| `--no-claude-md` | `bool` | Skip CLAUDE.md loading |
| `--output-format` | `Option<CliOutputFormat>` | Output format |
| `-v, --verbose` | `bool` | Enable verbose logging |
| `--api-key` | `Option<String>` | API key |
| `--max-tokens` | `Option<u32>` | Override max tokens |
| `--cwd` | `Option<PathBuf>` | Working directory |
| `--dangerously-skip-permissions` | `bool` | BypassPermissions mode |
| `--dump-system-prompt` | `bool` | Print system prompt and exit |
| `--mcp-config` | `Option<PathBuf>` | MCP server config JSON file |
| `--no-auto-compact` | `bool` | Disable auto-compact |

**`CliPermissionMode` enum** (clap ValueEnum): `Default`, `AcceptEdits`, `BypassPermissions`, `Plan`

**`CliOutputFormat` enum** (clap ValueEnum): `Text`, `Json`, `StreamJson`

### `McpToolWrapper`

Implements `Tool` for tools provided by MCP servers:
- `permission_level()` → `Execute`
- `execute()` strips server prefix from tool name, calls `McpManager::call_tool()`, converts result via `mcp_result_to_string()`

### `main()` Function

1. Parses `Cli` args with clap
2. Sets up `tracing_subscriber` (verbose → DEBUG, default → WARN)
3. Loads `Settings` from `~/.claude/settings.json`
4. Builds `Config` by layering: settings → CLI overrides
5. Determines `working_dir` (from `--cwd` or `std::env::current_dir()`)
6. Creates `Arc<CostTracker>`
7. Builds system context strings:
   - Reads `crates/cli/src/system_prompt.txt` (embedded at compile time via `include_str!`)
   - Calls `ContextBuilder::build_system_context()`
   - Calls `ContextBuilder::build_user_context()` (unless `--no-claude-md`)
   - Joins all parts
8. If `--dump-system-prompt`: prints and exits
9. Creates `AnthropicClient::from_config()`
10. Creates `ToolContext` with `AutoPermissionHandler`
11. Calls `McpManager::connect_all()` if MCP config provided
12. Builds tool list: `cc_tools::all_tools()` + `AgentTool` + `McpToolWrapper` for each MCP tool
13. Creates `CancellationToken`, starts cron scheduler with `start_cron_scheduler()`
14. If prompt provided or `--print`: calls `run_headless()`
15. Otherwise: calls `run_interactive()`

### `run_headless(prompt, client, messages, tools, tool_ctx, config, cost_tracker, output_format)`

1. Reads prompt from arg or stdin (if no positional arg)
2. Pushes `Message::user(prompt)` to messages
3. Spawns `run_query_loop()` with `mpsc::unbounded_channel()` for events
4. Drains event channel:
   - `Text` output format: prints `QueryEvent::Stream(TextDelta)` text directly; prints tool names
   - `Json` format: collects full response, outputs as single JSON object
   - `StreamJson` format: outputs each `QueryEvent` as NDJSON line
5. Returns on `QueryOutcome::EndTurn` or error

### `run_interactive()`

Interactive TUI REPL:
1. Sets up terminal via `cc_tui::setup_terminal()`
2. Restores terminal on exit (via `defer`-pattern)
3. Handles session resume if `--resume` provided
4. Main event loop at 16ms poll interval (`EventStream` from crossterm):
   - Processes `crossterm::event::KeyEvent` via `app.handle_key_event()`
   - On Enter: if slash command (`is_slash_command()`), calls `execute_command()` from `cc-commands`
   - Regular message: pushes to `messages`, spawns `run_query_loop()` as `tokio::spawn`
   - Shares `Arc<Mutex<Vec<Message>>>` between main and spawned task for result sync
   - Drains query events via `event_rx.try_recv()`
   - Calls `app.handle_query_event()` to update TUI state
   - Re-renders via `terminal.draw(|f| render_app(f, &app))`
5. Saves session to `cc_core::history::save_session()` after each completed turn

### System Prompt (`system_prompt.txt`)

Embedded in binary at compile time. Content:
> You are Claude Code, an AI coding assistant by Anthropic.

Guidelines:
- Read files before editing them
- Prefer editing existing files over creating new ones
- Write clean, idiomatic code
- Run tests after making changes
- Use git log/diff for codebase context
- Be concise in responses
- Produce production-quality code
- Never introduce security vulnerabilities

### Relationship to TypeScript

`claude-code` CLI corresponds to `src/entrypoints/cli.tsx` (the main TypeScript CLI entry point), `src/main.tsx`, `src/screens/REPL.tsx`, and `src/cli/` directory. The CLI flag names and behaviors are preserved, including `--print`, `--output-format`, `--permission-mode`, and `--resume`.

---

## Cross-Cutting Architecture Notes

### Async Runtime
All async code uses `tokio` with `"full"` features. The `#[tokio::main]` macro is on `main()` in `crates/cli/src/main.rs`. All tools use `async fn execute()` via `async_trait`.

### Cancellation
`tokio_util::sync::CancellationToken` is threaded through `run_query_loop()`, the cron scheduler, and TUI event loop. `Ctrl+C` fires the token.

### Global State
Three `DashMap`/`RwLock` singletons using `once_cell::sync::Lazy`:
- `TASK_STORE` (cc-tools/tasks.rs) — task management
- `INBOX` (cc-tools/send_message.rs) — inter-agent messaging
- `CRON_STORE` (cc-tools/cron.rs) — scheduled tasks
- `WORKTREE_SESSION` (cc-tools/worktree.rs) — active git worktree

### Error Handling
- Libraries use `thiserror` for typed `ClaudeError`
- CLI binary uses `anyhow` for ergonomic error propagation
- Tool errors never panic; always return `ToolResult::error()`

### Prompt Caching
`cc-api` automatically applies `CacheControl::ephemeral()` to:
- System prompt blocks (when using `SystemPrompt::Blocks`)
- The last tool definition in the tools list

### Logging
`tracing` + `tracing-subscriber` with `EnvFilter`. Default level WARN; `--verbose` enables DEBUG. Structured fields on all log calls.

### TypeScript Parity Summary

| TypeScript Area | Rust Crate |
|---|---|
| `src/entrypoints/cli.tsx`, `src/main.tsx` | `crates/cli` |
| `src/services/api/` | `crates/api` |
| `src/query.ts`, `src/query/` | `crates/query` |
| `src/components/`, `src/ink/` | `crates/tui` |
| `src/commands/` | `crates/commands` |
| `src/constants/`, `src/context.ts`, etc. | `crates/core` |
| Tool implementations (Bash, Read, Edit, etc.) | `crates/tools` |
| MCP client (`src/services/mcpClient.ts`) | `crates/mcp` |
| `src/bridge/` | `crates/bridge` |
