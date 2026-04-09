# LLM Providers

Claurst supports a wide range of LLM providers through a unified provider abstraction. Every provider implements the same `LlmProvider` trait, so switching between them requires only a configuration change.

---

## Selecting a Provider

Use the `--provider` flag on any invocation to override the active provider:

```
claurst --provider openai "refactor this module"
claurst --provider ollama "explain this function"
claurst --provider groq --model llama-3.3-70b-versatile "write tests"
```

The provider can also be set persistently in `~/.claurst/settings.json`:

```json
{
  "provider": "openai"
}
```

When no provider is specified, Claurst defaults to **Anthropic**.

---

## Provider Reference

### Anthropic (default)

The default provider. Uses the `/v1/messages` streaming endpoint.

**Authentication:** `ANTHROPIC_API_KEY` environment variable, or set `api_key` in `settings.json`.

**Default model:** `claude-sonnet-4-6`

**Available models (bundled snapshot):**

| Model ID | Context Window | Max Output | Input ($/1M) | Output ($/1M) |
|---|---|---|---|---|
| `claude-opus-4-6` | 200,000 | 32,000 | $15.00 | $75.00 |
| `claude-sonnet-4-6` | 200,000 | 16,000 | $3.00 | $15.00 |
| `claude-haiku-4-5-20251001` | 200,000 | 8,096 | $0.80 | $4.00 |

All Anthropic models support tool calling, vision, and extended reasoning.

**Configuration:**

```json
{
  "provider": "anthropic",
  "providers": {
    "anthropic": {
      "api_key": "sk-ant-...",
      "models_whitelist": ["claude-sonnet-4-6", "claude-haiku-4-5-20251001"]
    }
  }
}
```

**Base URL override:** Set `ANTHROPIC_BASE_URL` to point at a proxy or local mirror.

---

### OpenAI

Uses the OpenAI Chat Completions API (`/v1/chat/completions`).

**Authentication:** `OPENAI_API_KEY` environment variable.

**Default model:** `gpt-4o`

**Available models (bundled snapshot):**

| Model ID | Context Window | Max Output | Reasoning |
|---|---|---|---|
| `gpt-4o` | 128,000 | 16,384 | No |
| `gpt-4o-mini` | 128,000 | 16,384 | No |
| `o3` | 200,000 | 100,000 | Yes |
| `o4-mini` | 200,000 | 100,000 | Yes |

**Configuration:**

```json
{
  "provider": "openai",
  "providers": {
    "openai": {
      "api_key": "sk-...",
      "api_base": "https://api.openai.com/v1"
    }
  }
}
```

---

### Google (Gemini)

Uses the Google Generative Language / Vertex AI API.

**Authentication:** `GOOGLE_API_KEY` environment variable (for AI Studio) or `GOOGLE_APPLICATION_CREDENTIALS` (for Vertex AI).

**Default model:** `gemini-2.5-flash`

**Available models (bundled snapshot):**

| Model ID | Context Window | Max Output |
|---|---|---|
| `gemini-2.5-pro` | 1,048,576 | 65,536 |
| `gemini-2.5-flash` | 1,048,576 | 65,536 |
| `gemini-2.0-flash` | 1,048,576 | 8,192 |

**Configuration:**

```json
{
  "provider": "google",
  "providers": {
    "google": {
      "api_key": "AIza..."
    }
  }
}
```

---

### Azure OpenAI

Uses the Azure OpenAI Chat Completions endpoint. The deployment name acts as the model identifier.

**Authentication:** Three environment variables are required:
- `AZURE_API_KEY` — your Azure OpenAI API key
- `AZURE_RESOURCE_NAME` — your Azure resource name (the subdomain of `.openai.azure.com`)
- `AZURE_API_VERSION` — API version (defaults to `2024-08-01-preview`)

**Default model:** `gpt-4o`

**Request URL format:**

```
https://{AZURE_RESOURCE_NAME}.openai.azure.com/openai/deployments/{deployment}/chat/completions?api-version={version}
```

**Configuration:**

```json
{
  "provider": "azure",
  "providers": {
    "azure": {
      "api_key": "...",
      "options": {
        "resource_name": "my-azure-resource",
        "api_version": "2024-08-01-preview"
      }
    }
  }
}
```

---

### AWS Bedrock

Uses the Bedrock Converse Streaming API. Supports all Claude models deployed on Bedrock.

**Authentication (two modes):**

1. **Bearer token:** Set `AWS_BEARER_TOKEN_BEDROCK` (takes priority over SigV4).
2. **SigV4 credentials:** Set `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and optionally `AWS_SESSION_TOKEN`.

**Region:** Reads `AWS_REGION` or `AWS_DEFAULT_REGION` (defaults to `us-east-1`).

**Default model:** `anthropic.claude-sonnet-4-6-v1`

The adapter automatically prepends regional cross-inference prefixes (e.g. `us.anthropic.claude-...`) for US-region deployments.

**Configuration:**

```json
{
  "provider": "amazon-bedrock",
  "providers": {
    "amazon-bedrock": {
      "options": {
        "region": "us-east-1"
      }
    }
  }
}
```

---

### GitHub Copilot

Uses the GitHub Copilot Chat Completions API (`https://api.githubcopilot.com/chat/completions`).

**Authentication:** `GITHUB_TOKEN` environment variable.

**Default model:** `gpt-4o`

**Configuration:**

```json
{
  "provider": "github-copilot",
  "providers": {
    "github-copilot": {
      "api_key": "ghu_..."
    }
  }
}
```

---

### Cohere

Native Cohere API adapter.

**Authentication:** `COHERE_API_KEY` environment variable.

**Default model:** `command-r-plus`

**Configuration:**

```json
{
  "provider": "cohere",
  "providers": {
    "cohere": {
      "api_key": "..."
    }
  }
}
```

---

### Ollama (local)

Connects to a locally running Ollama instance. No API key required.

**Base URL:** Reads `OLLAMA_HOST` (defaults to `http://localhost:11434`). Claurst appends `/v1` to construct the OpenAI-compatible endpoint.

**Default model:** `llama3.2`

**Configuration:**

```json
{
  "provider": "ollama",
  "providers": {
    "ollama": {
      "api_base": "http://localhost:11434"
    }
  }
}
```

Run a model locally first with `ollama pull llama3.2`, then:

```
claurst --provider ollama --model llama3.2 "explain this code"
```

---

### LM Studio (local)

Connects to a locally running LM Studio server. No API key required.

**Base URL:** Reads `LM_STUDIO_HOST` (defaults to `http://localhost:1234`). Claurst appends `/v1`.

**Default model:** `default` (whichever model is loaded in LM Studio)

**Configuration:**

```json
{
  "provider": "lmstudio",
  "providers": {
    "lmstudio": {
      "api_base": "http://localhost:1234/v1"
    }
  }
}
```

---

### LLaMA.cpp (local)

Connects to a locally running llama.cpp HTTP server. No API key required.

**Base URL:** Reads `LLAMA_CPP_HOST` (defaults to `http://localhost:8080`). Claurst appends `/v1`.

**Default model:** `default`

**Configuration:**

```json
{
  "provider": "llamacpp",
  "providers": {
    "llamacpp": {
      "api_base": "http://localhost:8080/v1"
    }
  }
}
```

Start llama.cpp with the `--server` flag before use.

---

### MLX LM (local)

Connects to a locally running `mlx_lm` HTTP server. Requires Apple Silicon. No API key required.

**Base URL:** Reads `MLX_LM_HOST` (defaults to `http://localhost:8081`). Claurst appends `/v1`.

**Default model:** `default` (the model is loaded when starting the server)

**Configuration:**

```json
{
  "provider": "mlxlm",
  "providers": {
    "mlxlm": {
      "api_base": "http://localhost:8081/v1"
    }
  }
}
```

Start the server before connecting, specifying the model to load:

```bash
python -m mlx_lm.server --model BeastCode/Qwen3.5-27B-Claude-4.6-Opus-Distilled-MLX-4bit
```

Use a custom port:

```bash
MLX_LM_HOST=http://localhost:9000 claurst --provider mlxlm "hello"
# or
python -m mlx_lm.server --model <model> --port 9000
```

---

### Groq

Fast inference cloud with OpenAI-compatible API.

**Authentication:** `GROQ_API_KEY` environment variable.

**Base URL:** `https://api.groq.com/openai/v1`

**Default model:** `llama-3.3-70b-versatile`

**Configuration:**

```json
{
  "provider": "groq",
  "providers": {
    "groq": {
      "api_key": "gsk_..."
    }
  }
}
```

---

### DeepSeek

OpenAI-compatible API with extended reasoning output via a `reasoning_content` field.

**Authentication:** `DEEPSEEK_API_KEY` environment variable.

**Base URL:** `https://api.deepseek.com/v1`

**Default model:** `deepseek-chat`

**Configuration:**

```json
{
  "provider": "deepseek",
  "providers": {
    "deepseek": {
      "api_key": "sk-..."
    }
  }
}
```

---

### Mistral AI

OpenAI-compatible API with Mistral-specific protocol quirks (tool call ID formatting, tool-user sequence injection).

**Authentication:** `MISTRAL_API_KEY` environment variable.

**Base URL:** `https://api.mistral.ai/v1`

**Default model:** `mistral-large-latest`

**Configuration:**

```json
{
  "provider": "mistral",
  "providers": {
    "mistral": {
      "api_key": "..."
    }
  }
}
```

---

### xAI (Grok)

**Authentication:** `XAI_API_KEY` environment variable.

**Base URL:** `https://api.x.ai/v1`

**Default model:** `grok-2`

**Configuration:**

```json
{
  "provider": "xai",
  "providers": {
    "xai": {
      "api_key": "xai-..."
    }
  }
}
```

---

### OpenRouter

Unified API gateway to many models. Sends `HTTP-Referer: https://claurst.ai/` and `X-Title: Claurst` headers automatically.

**Authentication:** `OPENROUTER_API_KEY` environment variable.

**Base URL:** `https://openrouter.ai/api/v1`

**Default model:** `anthropic/claude-sonnet-4`

Model identifiers use OpenRouter's routing format: `provider/model-name`.

**Configuration:**

```json
{
  "provider": "openrouter",
  "providers": {
    "openrouter": {
      "api_key": "sk-or-..."
    }
  }
}
```

---

### Together AI

Hosted open-source models.

**Authentication:** `TOGETHER_API_KEY` environment variable.

**Base URL:** `https://api.together.xyz/v1`

**Default model:** `meta-llama/Llama-3.3-70B-Instruct-Turbo`

**Configuration:**

```json
{
  "provider": "togetherai",
  "providers": {
    "togetherai": {
      "api_key": "..."
    }
  }
}
```

---

### Perplexity

Search-augmented LLM API.

**Authentication:** `PERPLEXITY_API_KEY` environment variable.

**Base URL:** `https://api.perplexity.ai`

**Default model:** `sonar-pro`

**Configuration:**

```json
{
  "provider": "perplexity",
  "providers": {
    "perplexity": {
      "api_key": "pplx-..."
    }
  }
}
```

---

### DeepInfra

Hosted open-weight models on OpenAI-compatible API.

**Authentication:** `DEEPINFRA_API_KEY` environment variable.

**Base URL:** `https://api.deepinfra.com/v1/openai`

**Default model:** `meta-llama/Llama-3.3-70B-Instruct`

**Configuration:**

```json
{
  "provider": "deepinfra",
  "providers": {
    "deepinfra": {
      "api_key": "..."
    }
  }
}
```

---

### Venice AI

Privacy-focused inference.

**Authentication:** `VENICE_API_KEY` environment variable.

**Base URL:** `https://api.venice.ai/api/v1`

**Default model:** `llama-3.3-70b` (resolved from the model registry at runtime)

**Configuration:**

```json
{
  "provider": "venice",
  "providers": {
    "venice": {
      "api_key": "..."
    }
  }
}
```

---

### Cerebras

Wafer-scale inference hardware.

**Authentication:** `CEREBRAS_API_KEY` environment variable.

**Base URL:** `https://api.cerebras.ai/v1`

**Default model:** `llama-3.3-70b`

**Configuration:**

```json
{
  "provider": "cerebras",
  "providers": {
    "cerebras": {
      "api_key": "..."
    }
  }
}
```

---

## Per-Provider Configuration in settings.json

The `providers` map in `~/.claurst/settings.json` accepts per-provider `ProviderConfig` objects:

```json
{
  "provider": "anthropic",
  "providers": {
    "anthropic": {
      "api_key": "sk-ant-...",
      "api_base": "https://api.anthropic.com",
      "enabled": true,
      "models_whitelist": [],
      "models_blacklist": [],
      "options": {}
    },
    "openai": {
      "api_key": "sk-...",
      "enabled": true
    },
    "ollama": {
      "enabled": true,
      "api_base": "http://192.168.1.50:11434/v1"
    }
  }
}
```

**Fields:**

| Field | Type | Description |
|---|---|---|
| `api_key` | string | Override the environment variable API key |
| `api_base` | string | Override the default base URL |
| `enabled` | bool | Enable or disable the provider (default: `true`) |
| `models_whitelist` | array of strings | If non-empty, only listed model IDs are allowed |
| `models_blacklist` | array of strings | Listed model IDs are refused |
| `options` | object | Provider-specific pass-through options |

## Model Whitelist and Blacklist

When `models_whitelist` is non-empty for a provider, only the listed model IDs can be selected for that provider. Any model ID in `models_blacklist` is rejected regardless of the whitelist:

```json
{
  "providers": {
    "openai": {
      "models_whitelist": ["gpt-4o", "gpt-4o-mini"],
      "models_blacklist": ["gpt-4o-mini"]
    }
  }
}
```

The above example allows only `gpt-4o` (whitelist minus blacklist).

## Model Registry

Claurst ships a bundled snapshot of models for Anthropic, OpenAI, and Google. At runtime it optionally refreshes from the public `https://models.dev/api.json` API (cached to `~/.claurst/models_cache.json`, refreshed at most every 5 minutes). Network failures are swallowed silently; the bundled snapshot is always sufficient for normal operation.

When no model is explicitly set, Claurst scores available models by priority patterns to pick the best default. Well-known model prefixes (`claude-*`, `gpt-*`, `gemini-*`, etc.) are always routed to their canonical provider regardless of gateway entries in the remote cache.
