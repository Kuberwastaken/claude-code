# Agents and Multi-Agent Features

Claurst has a named-agent system that lets you select a pre-configured persona with its own tool permissions, model, system prompt, and turn budget. For larger tasks it also supports a coordinator mode where a top-level agent orchestrates a pool of parallel worker agents.

---

## Built-in Named Agents

Three agents ship by default. Their definitions can be overridden per-user in `~/.claurst/settings.json`.

### build

Full tool access. Intended for implementing features and fixing bugs.

| Property | Value |
|---|---|
| Access | `full` — all tools available |
| Max turns | Unlimited (uses the global default) |
| Display color | Cyan |

Default system prompt prefix:

> You are the build agent. You have full access to read, write, and execute. Focus on implementing the requested changes completely and correctly.

### plan

Read-only analysis. Cannot write files or execute commands. Intended for understanding codebases and planning changes before committing to implementation.

| Property | Value |
|---|---|
| Access | `read-only` — file reads, no writes or shell execution |
| Max turns | 20 |
| Display color | Yellow |

Default system prompt prefix:

> You are the plan agent. You can read files and analyze code but cannot write files or execute commands. Focus on understanding the codebase and describing what changes should be made.

### explore

Fast search-only exploration. Intended for quickly locating relevant code and answering questions about structure.

| Property | Value |
|---|---|
| Access | `search-only` — search tools only |
| Max turns | 15 |
| Display color | Green |

Default system prompt prefix:

> You are the explore agent. You can search and read files. Focus on quickly finding relevant code and answering questions about the codebase.

---

## Selecting an Agent with --agent

Pass `--agent <name>` to activate a named agent for a session:

```
claurst --agent build "implement the OAuth2 login flow"
claurst --agent plan "analyze the database schema and suggest improvements"
claurst --agent explore "find all usages of the deprecated config API"
```

The `--agent` flag can be combined with `--provider` and `--model`:

```
claurst --agent plan --provider openai --model o3 "review this architecture"
```

---

## The /agents Command

Within an interactive session, `/agents` lists all available named agents (built-in and custom):

```
/agents
```

Output shows the agent name, description, access level, and max turn limit. Agents with `visible: false` in their definition are hidden from this list.

---

## Custom Agent Definitions

Define custom agents in `~/.claurst/settings.json` under the `agents` key. Custom definitions override built-in agents of the same name.

```json
{
  "agents": {
    "review": {
      "description": "Senior code reviewer focused on correctness and security",
      "model": "anthropic/claude-opus-4-6",
      "temperature": 0.3,
      "prompt": "You are a senior software engineer performing code review. Focus on correctness, security vulnerabilities, performance issues, and maintainability. Be specific about problems and suggest concrete fixes.",
      "access": "read-only",
      "visible": true,
      "max_turns": 30,
      "color": "magenta"
    },
    "test-writer": {
      "description": "Writes comprehensive unit and integration tests",
      "model": "anthropic/claude-sonnet-4-6",
      "prompt": "You are a test engineer. Write thorough tests covering happy paths, edge cases, and error conditions. Use the project's existing test framework and conventions.",
      "access": "full",
      "visible": true,
      "max_turns": null,
      "color": "blue"
    },
    "docs": {
      "description": "Technical documentation writer",
      "model": "anthropic/claude-sonnet-4-6",
      "temperature": 0.5,
      "prompt": "You are a technical writer. Write clear, accurate documentation for the code you are given. Use the project's existing documentation style.",
      "access": "read-only",
      "visible": true,
      "max_turns": 25,
      "color": "cyan"
    }
  }
}
```

### AgentDefinition Fields

| Field | Type | Description |
|---|---|---|
| `description` | string | Short description shown in `/agents` |
| `model` | string | Model override in `provider/model` or bare `model` form. Omit to use the session default. |
| `temperature` | number | Sampling temperature override (0.0–1.0). Omit to use the model default. |
| `prompt` | string | System prompt prefix prepended before the main system prompt. |
| `access` | string | Permission restriction: `"full"` (all tools), `"read-only"` (no writes/shell), `"search-only"` (search tools only). Default: `"full"`. |
| `visible` | bool | Whether to show in `/agents` output. Default: `true`. |
| `max_turns` | number or null | Maximum agentic turns. Null means unlimited. Overrides the global turn budget. |
| `color` | string | ANSI terminal color for display: `"cyan"`, `"magenta"`, `"green"`, `"yellow"`, `"blue"`, etc. |

Use the agent with the `--agent` flag:

```
claurst --agent review "check the authentication module for security issues"
claurst --agent test-writer "write tests for the payment processor"
```

---

## Coordinator Mode

Coordinator mode enables a single top-level agent to orchestrate multiple parallel worker agents. The coordinator delegates research, implementation, and verification tasks to workers, then synthesises their results.

### Enabling Coordinator Mode

Set the `CLAURST_COORDINATOR_MODE` environment variable to `1` before launching:

```bash
CLAURST_COORDINATOR_MODE=1 claurst "refactor the entire authentication subsystem"
```

Or within a shell session:

```bash
export CLAURST_COORDINATOR_MODE=1
claurst
```

The value `"0"` and `"false"` disable coordinator mode even if the variable is set. Any other non-empty value enables it.

### How the Coordinator Works

When coordinator mode is active, Claurst injects a coordinator system prompt that instructs the model to orchestrate rather than act directly. The recommended workflow is:

1. **Research Phase** — Spawn workers in parallel to gather information about the codebase or requirements.
2. **Synthesis Phase** — Collect worker findings and build a complete understanding before proceeding.
3. **Implementation Phase** — Delegate implementation tasks to workers, each responsible for a well-defined scope.
4. **Verification Phase** — Spawn verification workers to validate results, run tests, and confirm correctness.

Worker prompts must be fully self-contained: workers cannot see the coordinator's conversation history.

### Coordinator-Only Tools

The following tools are available to the coordinator but are not passed to worker agents:

| Tool | Purpose |
|---|---|
| `Agent` | Spawn a new worker agent with a given prompt |
| `SendMessage` | Continue communication with a running worker |
| `TaskStop` | Cancel a worker that is no longer needed |
| `TeamCreate` | Create a named team of workers |
| `TeamDelete` | Dismantle a team |
| `SyntheticOutput` | Inject synthetic output into the conversation |

### Worker Tool Set

Workers receive all standard tools (file operations, Bash, web search, MCP tools, skills) but do not receive the coordinator-only tools listed above. This prevents workers from spawning their own sub-coordinators or interfering with task management.

In simple mode (`CLAURST_SIMPLE=1`), workers are further restricted to `["Bash", "Read", "Edit"]`.

### Banned Tools in Coordinator Mode

The coordinator itself does not use `Bash` directly — shell execution is delegated to workers. This enforces the principle that the coordinator orchestrates rather than executes.

---

## Task Management Tools

These tools are available in coordinator mode for tracking parallel work:

| Tool | Description |
|---|---|
| `TaskCreate` | Create a new tracked task |
| `TaskGet` | Retrieve task details by ID |
| `TaskUpdate` | Update task status or metadata |
| `TaskList` | List all active tasks |
| `TaskStop` | Cancel a running task (coordinator only) |
| `TaskOutput` | Read output produced by a task |

### /tasks Command

Within an interactive session, `/tasks` shows the current task list with status, worker assignments, and results:

```
/tasks
```

---

## Parallel Agent Execution

The coordinator spawns workers via the `Agent` tool. Workers run asynchronously; the coordinator can spawn multiple workers simultaneously and wait for all of them before proceeding to synthesis.

**Example coordinator workflow (expressed as a prompt):**

```
I need to refactor the authentication module. Let me plan this in parallel:

1. Use Agent to spawn a worker that reads and summarises all files in src/auth/
2. Use Agent to spawn a worker that identifies all callers of the auth API across the codebase
3. Wait for both workers to finish
4. Synthesise findings
5. Use Agent to spawn implementation workers for each logical unit of work
6. Run verification workers on the result
```

**Example coordinator session prompt:**

```bash
CLAURST_COORDINATOR_MODE=1 claurst \
  "Audit the entire src/payments directory for security issues. \
   Use parallel workers to examine each file, then produce a \
   consolidated security report with severity rankings."
```

---

## Agent Definitions in settings.json: Complete Example

```json
{
  "provider": "anthropic",
  "agents": {
    "build": {
      "description": "Full-access implementation agent",
      "model": "anthropic/claude-sonnet-4-6",
      "prompt": "You are the build agent. Implement requested changes completely. Prefer targeted, minimal edits over rewrites.",
      "access": "full",
      "visible": true,
      "max_turns": null,
      "color": "cyan"
    },
    "plan": {
      "description": "Read-only analysis and planning agent",
      "model": "anthropic/claude-opus-4-6",
      "temperature": 0.2,
      "prompt": "You are the plan agent. Analyse the codebase carefully before producing a detailed, step-by-step implementation plan. Do not write or execute anything.",
      "access": "read-only",
      "visible": true,
      "max_turns": 20,
      "color": "yellow"
    },
    "explore": {
      "description": "Fast search-only exploration agent",
      "model": "anthropic/claude-haiku-4-5-20251001",
      "prompt": "You are the explore agent. Search and read files to answer questions quickly.",
      "access": "search-only",
      "visible": true,
      "max_turns": 15,
      "color": "green"
    },
    "security": {
      "description": "Security-focused read-only audit agent",
      "model": "anthropic/claude-opus-4-6",
      "temperature": 0.1,
      "prompt": "You are a security auditor. Look for authentication flaws, injection vulnerabilities, insecure dependencies, and data-exposure risks. Report findings with severity levels (critical, high, medium, low) and concrete remediation steps.",
      "access": "read-only",
      "visible": true,
      "max_turns": 40,
      "color": "magenta"
    },
    "architect": {
      "description": "System design and architecture advisor",
      "model": "anthropic/claude-opus-4-6",
      "temperature": 0.4,
      "prompt": "You are a software architect. Reason carefully about system design trade-offs, scalability, maintainability, and technical debt. Produce clear architectural recommendations with rationale.",
      "access": "read-only",
      "visible": true,
      "max_turns": 30,
      "color": "blue"
    }
  }
}
```

---

## Session Continuity and Mode Matching

When resuming a saved session, Claurst detects whether the original session used coordinator mode and automatically sets `CLAURST_COORDINATOR_MODE` to match. A warning is printed when the environment is changed to prevent mode confusion in long-running workflows.

---

## Managed Agents (Preview)

Managed agents provide a formal **manager-executor** architecture that is distinct from coordinator mode. In coordinator mode, the orchestrator and workers all share the same configuration. With managed agents, you explicitly configure separate models, turn budgets, concurrency limits, and budget splits for the manager and for executors.

### When to use managed agents vs coordinator mode

| | Coordinator mode | Managed agents |
|---|---|---|
| **Setup** | `CLAURST_COORDINATOR_MODE=1` | `/managed-agents enable` |
| **Model selection** | All agents use the same model | Manager and executors can use different models |
| **Budget control** | Global session limits | Per-role USD caps or percentage splits |
| **Presets** | None | Several built-in presets available |
| **Use case** | Homogeneous parallel workers | Heterogeneous manager/worker separation |

### Enabling and configuring

```
/managed-agents presets                               — list presets
/managed-agents preset <name>                         — apply a preset
/managed-agents configure manager-model  anthropic/claude-opus-4-6
/managed-agents configure executor-model anthropic/claude-sonnet-4-6
/managed-agents configure concurrent     3
/managed-agents budget 5.00
/managed-agents enable
```

See [Managed Agents](./advanced.md#managed-agents) in the advanced guide for the full configuration reference.
