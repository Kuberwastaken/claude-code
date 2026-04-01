# Claude Code Rust — Multi-Provider Model Support Plan

## Goal

Enable this Rust codebase to use multiple model providers (starting with Anthropic + OpenAI-compatible) without regressing current behavior, tool-calling loop, or TUI/CLI UX.

## Scope and Non-Goals

### In Scope

- Keep Anthropic as a first-class provider.
- Introduce a provider abstraction in `cc-api`.
- Allow provider selection via config and CLI.
- Support streaming + non-streaming responses through a unified event model.
- Preserve existing query loop and tool execution semantics.

### Out of Scope (Phase 1)

- Full provider-specific parity for every advanced feature.
- Rewriting permission/tools systems.
- Migrating bridge protocol behavior.

## Current Binding Baseline (What Is Coupled Today)

- Client type is concrete `AnthropicClient` across query and CLI layers.
- API contract is fixed to Anthropic `/v1/messages` and Anthropic headers.
- Auth/env naming is Anthropic-specific (`ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`).
- OAuth/login flow is Anthropic-only.
- Model defaults are Anthropic model IDs.

## Target Architecture

### 1) API Trait Boundary

Add an interface in `cc-api`:

- `trait LlmClient`:
  - `create_message(request) -> response`
  - `create_message_stream(request, handler) -> stream events`
- Provider implementations:
  - `AnthropicClient` (existing, adapted)
  - `OpenAiCompatClient` (new, configurable base URL)

### 2) Canonical Internal Types

Define provider-agnostic canonical types in `cc-api`:

- `UnifiedMessageRequest`
- `UnifiedMessageResponse`
- `UnifiedStreamEvent`
- `UnifiedToolCall` / `UnifiedToolResult`
- `UnifiedUsage`

Each provider adapter maps canonical types to/from provider wire format.

### 3) Provider Selection

Config additions in `cc-core`:

- `provider: "anthropic" | "openai_compat"`
- Provider-scoped fields:
  - `api_key`, `api_base`, optional `api_version`/extra headers
- Backward compatibility:
  - If `provider` missing, default to `anthropic`
  - Keep current Anthropic env vars functional

CLI additions in `crates/cli`:

- `--provider <anthropic|openai-compat>`
- Optional provider-specific auth args later (not in first cut)

### 4) Query Layer Decoupling

Update `cc-query` function signatures to depend on trait object:

- From `&AnthropicClient` to `&(dyn LlmClient + Send + Sync)`
- Keep run-loop and tool orchestration unchanged.

### 5) Error and Retry Normalization

Normalize provider-specific failures into existing `ClaudeError` families or a renamed generic error enum:

- Auth errors
- Rate-limit/retryable errors
- API status errors
- Stream parse errors

## Implementation Phases

## Phase 0 - Prep and Safety

- Add snapshot tests around current Anthropic request/stream behavior.
- Add golden tests for event sequencing in query loop.
- Freeze current public CLI flags behavior.

Exit criteria:
- Existing tests green.
- New coverage around stream + tool events in place.

## Phase 1 - Introduce Abstraction (No Behavior Change)

- Create `LlmClient` trait and unified request/response/event types.
- Adapt `AnthropicClient` to implement trait.
- Refactor `cc-query` and `crates/cli` to use trait object.

Exit criteria:
- Anthropic-only behavior unchanged.
- No user-visible behavior regressions in TUI and headless mode.

## Phase 2 - OpenAI-Compatible Provider (MVP)

- Implement `OpenAiCompatClient` for Chat/Responses compatible endpoint.
- Map tool call events into canonical stream events.
- Add provider config parsing and CLI provider selection.

Exit criteria:
- End-to-end prompt->tool call->tool result->final answer works with OpenAI-compatible endpoint.
- JSON/stream-json output modes still function.

## Phase 3 - Hardening and UX

- Improve provider-specific validation and actionable errors.
- Expand model selection help (`/model`, `/config` guidance).
- Add docs for env vars and migration examples.

Exit criteria:
- Clear setup docs for both providers.
- Stable retry behavior under transient failures.

## Detailed Work Breakdown (By Crate)

### `crates/api`

- Introduce `LlmClient` trait.
- Add `provider` module layout:
  - `providers/anthropic.rs`
  - `providers/openai_compat.rs`
- Create canonical DTOs independent of Anthropic schema names.
- Build adapter mappers both directions.

### `crates/core`

- Extend config schema with provider and provider-specific settings.
- Add backward-compatible env resolution:
  - Keep `ANTHROPIC_API_KEY` and `ANTHROPIC_BASE_URL`.
  - Add generic aliases later (`CLAUDE_CODE_API_KEY`, `CLAUDE_CODE_API_BASE`).
- Keep default provider as `anthropic`.

### `crates/cli`

- Parse provider option.
- Construct selected client implementation.
- Keep auth flow Anthropic-gated in first iteration; non-Anthropic uses API key only.

### `crates/query`

- Accept trait object client in query loop, compact flow, cron scheduler, and sub-agent tool.
- Ensure cancellation and streaming behavior remains identical.

### `crates/commands` / `crates/tui`

- Update `/config` to read/write provider.
- Update diagnostics/help text currently hardcoded to Anthropic env vars.

## Compatibility and Migration Strategy

- Existing users with Anthropic config continue working without changes.
- If `provider=openai_compat` and required fields are missing, fail fast with clear guidance.
- Keep default models Anthropic unless provider-specific defaults are explicitly set.

## Risks and Mitigations

- Streaming protocol mismatch across providers.
  - Mitigation: strict adapter boundary and stream contract tests.
- Tool call schema differences.
  - Mitigation: canonical `UnifiedToolCall` shape + provider mappers.
- Silent behavior drift in retries.
  - Mitigation: deterministic retry tests and explicit backoff policy.

## Testing Strategy

- Unit tests:
  - Provider request/response mapping.
  - Error/status mapping.
- Integration tests:
  - Query loop with mocked provider streams.
  - Tool call roundtrip flow.
- Manual smoke:
  - Interactive TUI session.
  - `--print` + `--output-format json|stream-json`.

## Deliverables

- Provider abstraction merged with Anthropic parity.
- OpenAI-compatible provider MVP.
- Updated config/CLI docs and migration notes.
- Test matrix for Anthropic + OpenAI-compatible.

## Estimated Effort

- Phase 0-1: 1-2 days
- Phase 2: 1-2 days
- Phase 3: 0.5-1 day
- Total: roughly 3-5 engineering days (single contributor), assuming no large protocol surprises.

