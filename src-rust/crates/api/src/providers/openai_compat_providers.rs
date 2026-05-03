// providers/openai_compat_providers.rs — Factory functions for all
// OpenAI-compatible provider instances.
//
// Each function constructs a pre-configured [`OpenAiCompatProvider`] for a
// specific service.  API keys are read from environment variables; if the
// variable is absent or empty the provider is still constructed but
// `health_check()` will return `ProviderStatus::Unavailable`.

use claurst_core::config::Settings;
use claurst_core::provider_id::ProviderId;

use super::openai_compat::{OpenAiCompatProvider, ProviderQuirks};

pub fn provider_for_id(provider_id: &str) -> Option<OpenAiCompatProvider> {
    match provider_id {
        "ollama" => Some(ollama()),
        "lmstudio" | "lm-studio" => Some(lm_studio()),
        "llamacpp" | "llama-cpp" | "llama-server" => Some(llama_cpp()),
        "deepseek" => Some(deepseek()),
        "groq" => Some(groq()),
        "xai" => Some(xai()),
        "deepinfra" => Some(deepinfra()),
        "cerebras" => Some(cerebras()),
        "togetherai" | "together-ai" => Some(together_ai()),
        "perplexity" => Some(perplexity()),
        "venice" => Some(venice()),
        "qwen" => Some(qwen()),
        "mistral" => Some(mistral()),
        "openrouter" => Some(openrouter()),
        "sambanova" => Some(sambanova()),
        "huggingface" => Some(huggingface()),
        "nvidia" => Some(nvidia()),
        "siliconflow" => Some(siliconflow()),
        "moonshot" | "moonshotai" => Some(moonshot()),
        "zhipu" | "zhipuai" => Some(zhipu()),
        "zai" => Some(zai()),
        "nebius" => Some(nebius()),
        "novita" => Some(novita()),
        "ovhcloud" => Some(ovhcloud()),
        "scaleway" => Some(scaleway()),
        "vultr" | "vultr-ai" => Some(vultr_ai()),
        "baseten" => Some(baseten()),
        "friendli" => Some(friendli()),
        "upstage" => Some(upstage()),
        "stepfun" => Some(stepfun()),
        "fireworks" => Some(fireworks()),
        "opencode-go" | "opencode_go" => Some(opencode_go()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Local / self-hosted providers (no API key required)
// ---------------------------------------------------------------------------

/// Ollama — local inference server.
/// Reads `OLLAMA_HOST` for the base URL; defaults to `http://localhost:11434`.
pub fn ollama() -> OpenAiCompatProvider {
    let host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let base_url = format!("{}/v1", host.trim_end_matches('/'));
    OpenAiCompatProvider::new(ProviderId::OLLAMA, "Ollama", base_url).with_quirks(ProviderQuirks {
        overflow_patterns: vec![
            "prompt too long".to_string(),
            "exceeded.*context length".to_string(),
        ],
        no_api_key_required: true,
        ollama_native_host: Some(host),
        ..Default::default()
    })
}

/// LM Studio — local OpenAI-compatible server.
/// Reads `LM_STUDIO_HOST` for the base URL; defaults to `http://localhost:1234`.
pub fn lm_studio() -> OpenAiCompatProvider {
    let host =
        std::env::var("LM_STUDIO_HOST").unwrap_or_else(|_| "http://localhost:1234".to_string());
    let base_url = format!("{}/v1", host.trim_end_matches('/'));
    OpenAiCompatProvider::new(ProviderId::LM_STUDIO, "LM Studio", base_url).with_quirks(
        ProviderQuirks {
            overflow_patterns: vec!["greater than the context length".to_string()],
            no_api_key_required: true,
            ..Default::default()
        },
    )
}

/// llama.cpp — lightweight C++ inference server.
/// Reads `LLAMA_CPP_HOST` for the base URL; defaults to `http://localhost:8080`.
pub fn llama_cpp() -> OpenAiCompatProvider {
    let host =
        std::env::var("LLAMA_CPP_HOST").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let base_url = format!("{}/v1", host.trim_end_matches('/'));
    OpenAiCompatProvider::new(ProviderId::LLAMA_CPP, "llama.cpp", base_url).with_quirks(
        ProviderQuirks {
            overflow_patterns: vec!["exceeds the available context size".to_string()],
            no_api_key_required: true,
            ..Default::default()
        },
    )
}

// ---------------------------------------------------------------------------
// Remote / cloud providers (API key required)
// ---------------------------------------------------------------------------
/// Custom OpenAI-compatible provider supplied by the user.
pub fn custom_openai_with_url(base_url: impl Into<String>) -> OpenAiCompatProvider {
    let key = std::env::var("CUSTOM_OPENAI_API_KEY").unwrap_or_default();

    OpenAiCompatProvider::new(
        "custom-openai",
        "Custom OpenAI-Compatible",
        base_url.into(),
    )
    .with_api_key(key)
}

/// Custom OpenAI-compatible provider supplied by the user.
pub fn custom_openai() -> OpenAiCompatProvider {
    let settings = Settings::load_sync().unwrap_or_default();
    let base_url = settings
        .providers
        .get("custom-openai")
        .and_then(|config| config.api_base.as_deref())
        .filter(|url| !url.trim().is_empty())
        .unwrap_or("http://localhost:11434/v1");

    custom_openai_with_url(base_url)
}

/// DeepSeek V4 — supports reasoning output via `reasoning_content` field.
/// Reads `DEEPSEEK_API_KEY`.
pub fn deepseek() -> OpenAiCompatProvider {
    let key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::DEEPSEEK,
        "DeepSeek",
        "https://api.deepseek.com/v1",
    )
    .with_api_key(key)
    .with_quirks(ProviderQuirks {
        reasoning_field: Some("reasoning_content".to_string()),
        overflow_patterns: vec!["maximum context length is".to_string()],
        include_usage_in_stream: true,
        max_tokens_cap: None,
        ..Default::default()
    })
}

/// Groq — fast inference cloud.  Reads `GROQ_API_KEY`.
pub fn groq() -> OpenAiCompatProvider {
    let key = std::env::var("GROQ_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(ProviderId::GROQ, "Groq", "https://api.groq.com/openai/v1")
        .with_api_key(key)
        .with_quirks(ProviderQuirks {
            overflow_patterns: vec!["reduce the length of the messages".to_string()],
            include_usage_in_stream: true,
            ..Default::default()
        })
}

/// xAI (Grok).  Reads `XAI_API_KEY`.
pub fn xai() -> OpenAiCompatProvider {
    let key = std::env::var("XAI_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(ProviderId::XAI, "xAI (Grok)", "https://api.x.ai/v1")
        .with_api_key(key)
        .with_quirks(ProviderQuirks {
            overflow_patterns: vec!["maximum prompt length is".to_string()],
            ..Default::default()
        })
}

/// DeepInfra — hosted open-weight models.  Reads `DEEPINFRA_API_KEY`.
pub fn deepinfra() -> OpenAiCompatProvider {
    let key = std::env::var("DEEPINFRA_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::DEEPINFRA,
        "DeepInfra",
        "https://api.deepinfra.com/v1/openai",
    )
    .with_api_key(key)
}

/// Cerebras — wafer-scale inference.  Reads `CEREBRAS_API_KEY`.
pub fn cerebras() -> OpenAiCompatProvider {
    let key = std::env::var("CEREBRAS_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::CEREBRAS,
        "Cerebras",
        "https://api.cerebras.ai/v1",
    )
    .with_api_key(key)
    .with_quirks(ProviderQuirks {
        include_usage_in_stream: true,
        ..Default::default()
    })
}

/// Together AI — hosted open-source models.  Reads `TOGETHER_API_KEY`.
pub fn together_ai() -> OpenAiCompatProvider {
    let key = std::env::var("TOGETHER_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::TOGETHER_AI,
        "Together AI",
        "https://api.together.xyz/v1",
    )
    .with_api_key(key)
    .with_quirks(ProviderQuirks {
        include_usage_in_stream: true,
        ..Default::default()
    })
}

/// Perplexity — search-augmented LLM API.  Reads `PERPLEXITY_API_KEY`.
pub fn perplexity() -> OpenAiCompatProvider {
    let key = std::env::var("PERPLEXITY_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::PERPLEXITY,
        "Perplexity",
        "https://api.perplexity.ai",
    )
    .with_api_key(key)
    .with_quirks(ProviderQuirks {
        include_usage_in_stream: true,
        ..Default::default()
    })
}

/// Venice AI — privacy-focused inference.  Reads `VENICE_API_KEY`.
pub fn venice() -> OpenAiCompatProvider {
    let key = std::env::var("VENICE_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::VENICE,
        "Venice AI",
        "https://api.venice.ai/api/v1",
    )
    .with_api_key(key)
}

/// Qwen / Alibaba DashScope.  Reads `DASHSCOPE_API_KEY`.
/// Uses a default temperature of 0.55 as recommended by Alibaba's docs.
pub fn qwen() -> OpenAiCompatProvider {
    let key = std::env::var("DASHSCOPE_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        "qwen",
        "Qwen (Alibaba)",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
    )
    .with_api_key(key)
    .with_quirks(ProviderQuirks {
        default_temperature: Some(0.55),
        ..Default::default()
    })
}

/// Mistral AI — Reads `MISTRAL_API_KEY`.
/// Uses OpenAI-compatible format with Mistral-specific quirks:
///   - Tool call IDs must be alphanumeric only, truncated to 9 chars and
///     right-padded with zeroes to exactly 9 chars.
///   - An assistant "Done." turn is inserted between tool→user message transitions.
pub fn mistral() -> OpenAiCompatProvider {
    let key = std::env::var("MISTRAL_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::MISTRAL,
        "Mistral AI",
        "https://api.mistral.ai/v1",
    )
    .with_api_key(key)
    .with_quirks(ProviderQuirks {
        tool_id_max_len: Some(9),
        tool_id_alphanumeric_only: true,
        fix_tool_user_sequence: true,
        include_usage_in_stream: true,
        overflow_patterns: vec!["too large for model with".to_string()],
        ..Default::default()
    })
}

/// OpenRouter — unified API gateway to many models.  Reads `OPENROUTER_API_KEY`.
pub fn openrouter() -> OpenAiCompatProvider {
    let key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::OPENROUTER,
        "OpenRouter",
        "https://openrouter.ai/api/v1",
    )
    .with_api_key(key)
    .with_header("HTTP-Referer", "https://claurst.ai/")
    .with_header("X-Title", "Claurst")
    .with_quirks(ProviderQuirks {
        include_usage_in_stream: true,
        ..Default::default()
    })
}

/// SambaNova — fast inference cloud.  Reads `SAMBANOVA_API_KEY`.
pub fn sambanova() -> OpenAiCompatProvider {
    let key = std::env::var("SAMBANOVA_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::SAMBANOVA,
        "SambaNova",
        "https://api.sambanova.ai/v1",
    )
    .with_api_key(key)
}

/// Hugging Face Inference API.  Reads `HF_TOKEN`.
pub fn huggingface() -> OpenAiCompatProvider {
    let key = std::env::var("HF_TOKEN").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::HUGGINGFACE,
        "Hugging Face",
        "https://router.huggingface.co/v1",
    )
    .with_api_key(key)
}

/// Nvidia NIM — enterprise AI inference.  Reads `NVIDIA_API_KEY`.
pub fn nvidia() -> OpenAiCompatProvider {
    let key = std::env::var("NVIDIA_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::NVIDIA,
        "Nvidia",
        "https://integrate.api.nvidia.com/v1",
    )
    .with_api_key(key)
}

/// SiliconFlow — DeepSeek / Qwen hosting.  Reads `SILICONFLOW_API_KEY`.
pub fn siliconflow() -> OpenAiCompatProvider {
    let key = std::env::var("SILICONFLOW_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::SILICONFLOW,
        "SiliconFlow",
        "https://api.siliconflow.com/v1",
    )
    .with_api_key(key)
}

/// Moonshot AI / Kimi.  Reads `MOONSHOT_API_KEY`.
pub fn moonshot() -> OpenAiCompatProvider {
    let key = std::env::var("MOONSHOT_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::MOONSHOT,
        "Moonshot AI",
        "https://api.moonshot.ai/v1",
    )
    .with_api_key(key)
}

/// Zhipu AI / GLM.  Reads `ZHIPU_API_KEY`.
pub fn zhipu() -> OpenAiCompatProvider {
    let key = std::env::var("ZHIPU_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::ZHIPU,
        "Zhipu AI",
        "https://open.bigmodel.cn/api/paas/v4",
    )
    .with_api_key(key)
}

/// Z.AI (Zhipu) — current-generation GLM models (GLM-5.1, GLM-5, GLM-5-Turbo, GLM-4.7, etc.).
/// Uses the Z.AI international endpoint per docs.z.ai.
/// Reads `ZAI_API_KEY`.
pub fn zai() -> OpenAiCompatProvider {
    let key = std::env::var("ZAI_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::ZAI,
        "Z.AI",
        "https://api.z.ai/api/coding/paas/v4",
    )
    .with_api_key(key)
}

/// Nebius — Llama / Qwen hosting.  Reads `NEBIUS_API_KEY`.
pub fn nebius() -> OpenAiCompatProvider {
    let key = std::env::var("NEBIUS_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::NEBIUS,
        "Nebius",
        "https://api.tokenfactory.nebius.com/v1",
    )
    .with_api_key(key)
}

/// Novita — Llama / Stable Diffusion hosting.  Reads `NOVITA_API_KEY`.
pub fn novita() -> OpenAiCompatProvider {
    let key = std::env::var("NOVITA_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::NOVITA,
        "Novita",
        "https://api.novita.ai/v3/openai",
    )
    .with_api_key(key)
}

/// OVHcloud — EU-hosted AI.  Reads `OVHCLOUD_API_KEY`.
pub fn ovhcloud() -> OpenAiCompatProvider {
    let key = std::env::var("OVHCLOUD_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::OVHCLOUD,
        "OVHcloud",
        "https://oai.endpoints.kepler.ai.cloud.ovh.net/v1",
    )
    .with_api_key(key)
}

/// Scaleway — EU cloud AI.  Reads `SCALEWAY_API_KEY`.
pub fn scaleway() -> OpenAiCompatProvider {
    let key = std::env::var("SCALEWAY_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::SCALEWAY,
        "Scaleway",
        "https://api.scaleway.ai/v1",
    )
    .with_api_key(key)
}

/// Vultr — cloud inference.  Reads `VULTR_API_KEY`.
pub fn vultr_ai() -> OpenAiCompatProvider {
    let key = std::env::var("VULTR_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::VULTR,
        "Vultr",
        "https://api.vultrinference.com/v1",
    )
    .with_api_key(key)
}

/// Baseten — model serving.  Reads `BASETEN_API_KEY`.
pub fn baseten() -> OpenAiCompatProvider {
    let key = std::env::var("BASETEN_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::BASETEN,
        "Baseten",
        "https://inference.baseten.co/v1",
    )
    .with_api_key(key)
}

/// Friendli — serverless inference.  Reads `FRIENDLI_TOKEN`.
pub fn friendli() -> OpenAiCompatProvider {
    let key = std::env::var("FRIENDLI_TOKEN").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::FRIENDLI,
        "Friendli",
        "https://api.friendli.ai/serverless/v1",
    )
    .with_api_key(key)
}

/// Upstage — Solar models.  Reads `UPSTAGE_API_KEY`.
pub fn upstage() -> OpenAiCompatProvider {
    let key = std::env::var("UPSTAGE_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::UPSTAGE,
        "Upstage",
        "https://api.upstage.ai/v1/solar",
    )
    .with_api_key(key)
}

/// StepFun — Step models.  Reads `STEPFUN_API_KEY`.
pub fn stepfun() -> OpenAiCompatProvider {
    let key = std::env::var("STEPFUN_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(ProviderId::STEPFUN, "StepFun", "https://api.stepfun.com/v1")
        .with_api_key(key)
}

/// Fireworks AI — fast inference.  Reads `FIREWORKS_API_KEY`.
pub fn fireworks() -> OpenAiCompatProvider {
    let key = std::env::var("FIREWORKS_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::FIREWORKS,
        "Fireworks AI",
        "https://api.fireworks.ai/inference/v1",
    )
    .with_api_key(key)
}

/// OpenCode Go — flat-rate subscription endpoint hosted by opencode.ai.
/// OpenAI-compatible chat completions surface; same key works for the Zen
/// metered tier, hence the shared `OPENCODE_API_KEY` env var.
pub fn opencode_go() -> OpenAiCompatProvider {
    let key = std::env::var("OPENCODE_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        ProviderId::OPENCODE_GO,
        "OpenCode Go",
        "https://opencode.ai/zen/go/v1",
    )
    .with_api_key(key)
    .with_quirks(ProviderQuirks {
        include_usage_in_stream: true,
        ..Default::default()
    })
}
