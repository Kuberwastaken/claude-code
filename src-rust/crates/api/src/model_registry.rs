// model_registry.rs — Lossless model registry sourced from models.dev.
//
// **Architecture** (mirrors opencode):
//   1. A bundled snapshot of `https://models.dev/api.json` is embedded at
//      compile time from `crates/api/assets/models-snapshot.json`.  This is
//      the authoritative catalog for ~118 providers / ~4500 models.
//   2. On startup the registry is hydrated from the embedded snapshot.
//   3. Optionally, `load_cache()` overlays a fresher copy from disk
//      (refreshed by `refresh_from_models_dev()` once it has run).
//   4. `refresh_from_models_dev()` fetches the latest catalog from
//      `https://models.dev/api.json` (overridable via `MODELS_DEV_URL` /
//      `CLAURST_MODELS_URL`) and writes it back to the on-disk cache.
//
// **No more hardcoded per-provider model lists.**  All metadata —
// modalities, pricing, release date, capability flags, npm SDK package —
// lives in the bundled JSON and is updated by re-running
// `script/sync-models.{ps1,sh}`.
//
// All network and parse failures are non-fatal: the bundled snapshot is
// always available as a fallback.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use claurst_core::provider_id::{ModelId, ProviderId};

use crate::provider::ModelInfo;

// ---------------------------------------------------------------------------
// Embedded snapshot
// ---------------------------------------------------------------------------

/// The bundled models.dev snapshot, baked into the binary at compile time.
///
/// Refresh it locally with `bun run script/sync-models.ts` (TS) or
/// `pwsh script/sync-models.ps1` (PS).  CI also refreshes weekly.
const BUNDLED_SNAPSHOT: &[u8] = include_bytes!("../assets/models-snapshot.json");

// ---------------------------------------------------------------------------
// Capability enums
// ---------------------------------------------------------------------------

/// Input or output media type for a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    Text,
    Audio,
    Image,
    Video,
    Pdf,
}

/// Model lifecycle status as reported by models.dev.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelStatus {
    Active,
    Beta,
    Alpha,
    Deprecated,
}

impl Default for ModelStatus {
    fn default() -> Self { ModelStatus::Active }
}

impl ModelStatus {
    /// Whether to surface this model in default UI listings.
    ///
    /// Alpha/deprecated models are hidden unless
    /// `CLAURST_ENABLE_EXPERIMENTAL_MODELS=1`.
    pub fn is_listed_by_default(self) -> bool {
        matches!(self, ModelStatus::Active | ModelStatus::Beta)
    }
}

/// How a model emits reasoning content during streaming.
///
/// `Plain` means reasoning is delivered alongside normal content.  The
/// `Field` variant indicates reasoning arrives in a specific JSON field
/// (`reasoning_content` or `reasoning_details`) and must be hoisted into a
/// thinking block by the streaming adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InterleavedReasoning {
    Plain(bool),
    Field { field: String },
}

/// Per-model override of the provider-level NPM SDK package or API URL.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
}

/// One alternative dispatch mode for a model (e.g. "fast", "priority").
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExperimentalMode {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostBreakdown>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub provider_body: HashMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub provider_headers: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// CostBreakdown
// ---------------------------------------------------------------------------

/// Full pricing breakdown for one model.  All values are USD per 1M tokens.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostBreakdown {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_audio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_audio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<f64>,
    /// Pricing tier when the prompt exceeds 200K tokens (currently used by
    /// Claude on certain providers).  Not recursive — this is the only depth
    /// models.dev supports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_over_200k: Option<Box<CostBreakdown>>,
}

// ---------------------------------------------------------------------------
// ProviderEntry
// ---------------------------------------------------------------------------

/// Provider-level metadata captured from models.dev.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub id: ProviderId,
    pub name: String,
    /// Environment variable names that may supply this provider's credentials.
    #[serde(default)]
    pub env: Vec<String>,
    /// Default base URL for the provider's API (may be `None` for providers
    /// that require user-supplied URLs, e.g. self-hosted deployments).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// AI-SDK npm package for this provider (informational).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm: Option<String>,
    /// Documentation URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

// ---------------------------------------------------------------------------
// ModelEntry
// ---------------------------------------------------------------------------

/// Lossless representation of one model.  Mirrors models.dev schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// Identifier, provider link, name, context window, output limit.
    pub info: ModelInfo,

    // ---- Identity & lifecycle ---------------------------------------------
    /// Model family (`"claude"`, `"gpt"`, `"gemini"`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// Lifecycle status; influences default visibility in pickers.
    #[serde(default)]
    pub status: ModelStatus,
    /// First public availability (ISO 8601 date string).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    /// Last meaningful update on models.dev (ISO 8601 date string).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
    /// Knowledge cutoff (free-form, e.g. `"2024-09"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<String>,
    /// Whether the model weights are publicly available.
    #[serde(default)]
    pub open_weights: bool,

    // ---- Capability flags -------------------------------------------------
    /// Supports tool / function calling.
    #[serde(default)]
    pub tool_calling: bool,
    /// Supports extended thinking / reasoning.
    #[serde(default)]
    pub reasoning: bool,
    /// Supports structured (JSON-schema) output.
    #[serde(default)]
    pub structured_output: bool,
    /// Honours the `temperature` parameter.
    #[serde(default = "default_true")]
    pub temperature: bool,
    /// Accepts file attachments (PDF, images, etc.).
    #[serde(default)]
    pub attachment: bool,
    /// How reasoning content is delivered when streaming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interleaved: Option<InterleavedReasoning>,

    // ---- Modalities -------------------------------------------------------
    /// Input modalities (text, image, audio, video, pdf).
    #[serde(default)]
    pub modalities_input: Vec<Modality>,
    /// Output modalities.
    #[serde(default)]
    pub modalities_output: Vec<Modality>,

    // ---- Pricing (top-level mirrors are kept for backward compat) ---------
    /// USD per 1M input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_input: Option<f64>,
    /// USD per 1M output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_output: Option<f64>,
    /// USD per 1M cache-read tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cache_read: Option<f64>,
    /// USD per 1M cache-write tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cache_write: Option<f64>,
    /// Full cost breakdown including audio and reasoning premiums.  Use this
    /// instead of the top-level `cost_*` fields when you need every tier.
    #[serde(default)]
    pub cost: CostBreakdown,

    // ---- SDK overrides ----------------------------------------------------
    /// Override for the provider-level npm SDK / API URL on a per-model basis
    /// (e.g. when a "minimax-m2.7" hosted on opencode-go uses the Anthropic
    /// SDK rather than the openai-compatible one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_override: Option<ProviderOverride>,
    /// Per-mode dispatch alternatives (rarely populated).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub experimental_modes: HashMap<String, ExperimentalMode>,
}

fn default_true() -> bool { true }

impl ModelEntry {
    /// Whether this model accepts image input.  Derived from `modalities_input`.
    ///
    /// Kept as a method (rather than a field) so the registry only stores one
    /// source of truth.
    pub fn vision(&self) -> bool {
        self.modalities_input.contains(&Modality::Image)
    }

    /// Whether this model accepts audio input.
    pub fn audio_input(&self) -> bool {
        self.modalities_input.contains(&Modality::Audio)
    }

    /// Whether this model accepts PDF input.
    pub fn pdf_input(&self) -> bool {
        self.modalities_input.contains(&Modality::Pdf)
    }

    /// Whether this model accepts video input.
    pub fn video_input(&self) -> bool {
        self.modalities_input.contains(&Modality::Video)
    }
}

// ---------------------------------------------------------------------------
// models.dev raw schema (for parsing)
// ---------------------------------------------------------------------------

/// Raw shape of `models.dev/api.json`.  We deserialize into this and then
/// transform into the internal `ProviderEntry` / `ModelEntry` records.
#[allow(dead_code)]
mod md {
    use super::*;

    pub type ApiJson = HashMap<String, Provider>;

    #[derive(Debug, Deserialize)]
    pub struct Provider {
        pub id: String,
        pub name: String,
        #[serde(default)]
        pub env: Vec<String>,
        #[serde(default)]
        pub api: Option<String>,
        #[serde(default)]
        pub npm: Option<String>,
        #[serde(default)]
        pub doc: Option<String>,
        #[serde(default)]
        pub models: HashMap<String, Model>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Model {
        pub id: String,
        pub name: String,
        #[serde(default)]
        pub family: Option<String>,
        #[serde(default)]
        pub status: Option<ModelStatus>,
        #[serde(default)]
        pub release_date: Option<String>,
        #[serde(default)]
        pub last_updated: Option<String>,
        #[serde(default)]
        pub knowledge: Option<String>,
        #[serde(default)]
        pub open_weights: bool,
        #[serde(default)]
        pub attachment: bool,
        #[serde(default = "default_true")]
        pub temperature: bool,
        #[serde(default)]
        pub tool_call: bool,
        #[serde(default)]
        pub reasoning: bool,
        #[serde(default)]
        pub structured_output: bool,
        #[serde(default)]
        pub interleaved: Option<InterleavedReasoning>,
        #[serde(default)]
        pub modalities: Option<Modalities>,
        #[serde(default)]
        pub cost: Option<MdCost>,
        #[serde(default)]
        pub limit: Option<MdLimit>,
        #[serde(default)]
        pub provider: Option<ProviderOverride>,
        #[serde(default)]
        pub experimental: Option<MdExperimental>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Modalities {
        #[serde(default)]
        pub input: Vec<Modality>,
        #[serde(default)]
        pub output: Vec<Modality>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct MdCost {
        #[serde(default)]
        pub input: Option<f64>,
        #[serde(default)]
        pub output: Option<f64>,
        #[serde(default)]
        pub cache_read: Option<f64>,
        #[serde(default)]
        pub cache_write: Option<f64>,
        #[serde(default)]
        pub input_audio: Option<f64>,
        #[serde(default)]
        pub output_audio: Option<f64>,
        #[serde(default)]
        pub reasoning: Option<f64>,
        #[serde(default)]
        pub context_over_200k: Option<Box<MdCost>>,
    }

    impl From<MdCost> for CostBreakdown {
        fn from(c: MdCost) -> Self {
            CostBreakdown {
                input: c.input,
                output: c.output,
                cache_read: c.cache_read,
                cache_write: c.cache_write,
                input_audio: c.input_audio,
                output_audio: c.output_audio,
                reasoning: c.reasoning,
                context_over_200k: c.context_over_200k.map(|c| Box::new((*c).into())),
            }
        }
    }

    #[derive(Debug, Deserialize)]
    pub struct MdLimit {
        #[serde(default)]
        pub context: Option<u64>,
        #[serde(default)]
        pub input: Option<u64>,
        #[serde(default)]
        pub output: Option<u64>,
    }

    #[derive(Debug, Deserialize)]
    pub struct MdExperimental {
        #[serde(default)]
        pub modes: HashMap<String, MdExperimentalMode>,
    }

    #[derive(Debug, Deserialize)]
    pub struct MdExperimentalMode {
        #[serde(default)]
        pub cost: Option<MdCost>,
        #[serde(default)]
        pub provider: Option<MdExperimentalProvider>,
    }

    #[derive(Debug, Deserialize)]
    pub struct MdExperimentalProvider {
        #[serde(default)]
        pub body: HashMap<String, serde_json::Value>,
        #[serde(default)]
        pub headers: HashMap<String, String>,
    }

    fn default_true() -> bool { true }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Result of parsing a models.dev `api.json` payload.
#[derive(Debug, Default)]
struct ParsedSnapshot {
    providers: HashMap<String, ProviderEntry>,
    models: HashMap<String, ModelEntry>,
}

fn parse_snapshot_bytes(bytes: &[u8]) -> Option<ParsedSnapshot> {
    let api: md::ApiJson = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "Failed to parse models.dev snapshot");
            return None;
        }
    };
    Some(transform_api(api))
}

fn parse_snapshot_str(s: &str) -> Option<ParsedSnapshot> {
    parse_snapshot_bytes(s.as_bytes())
}

fn transform_api(api: md::ApiJson) -> ParsedSnapshot {
    let mut out = ParsedSnapshot::default();

    for (provider_id, p) in api.into_iter() {
        let pid = ProviderId::new(provider_id.clone());

        out.providers.insert(provider_id.clone(), ProviderEntry {
            id: pid.clone(),
            name: p.name,
            env: p.env,
            api: p.api,
            npm: p.npm,
            doc: p.doc,
        });

        for (model_id, m) in p.models.into_iter() {
            let mid = ModelId::new(model_id.clone());
            let key = format!("{}/{}", provider_id, model_id);

            // Resolve context window / output limit (default to 4K if absent —
            // matches old behaviour).
            let (ctx_window, max_output) = match m.limit {
                Some(l) => (
                    l.context.unwrap_or(4_096) as u32,
                    l.output.unwrap_or(4_096) as u32,
                ),
                None => (4_096, 4_096),
            };

            // Cost: keep top-level mirrors so existing callers
            // (`entry.cost_input.unwrap_or(0.0)`) keep working.
            let cost: CostBreakdown = m.cost.clone().map(Into::into).unwrap_or_default();

            // Modalities: default to text-only when omitted.
            let (mod_in, mod_out) = match m.modalities {
                Some(m) => (m.input, m.output),
                None => (vec![Modality::Text], vec![Modality::Text]),
            };

            // Experimental modes -> internal map.
            let experimental_modes = m
                .experimental
                .map(|e| {
                    e.modes
                        .into_iter()
                        .map(|(k, v)| {
                            let mode = ExperimentalMode {
                                cost: v.cost.map(Into::into),
                                provider_body: v
                                    .provider
                                    .as_ref()
                                    .map(|p| p.body.clone())
                                    .unwrap_or_default(),
                                provider_headers: v
                                    .provider
                                    .as_ref()
                                    .map(|p| p.headers.clone())
                                    .unwrap_or_default(),
                            };
                            (k, mode)
                        })
                        .collect()
                })
                .unwrap_or_default();

            let entry = ModelEntry {
                info: ModelInfo {
                    id: mid,
                    provider_id: pid.clone(),
                    name: m.name,
                    context_window: ctx_window,
                    max_output_tokens: max_output,
                },
                family: m.family,
                status: m.status.unwrap_or_default(),
                release_date: m.release_date,
                last_updated: m.last_updated,
                knowledge: m.knowledge,
                open_weights: m.open_weights,
                tool_calling: m.tool_call,
                reasoning: m.reasoning,
                structured_output: m.structured_output,
                temperature: m.temperature,
                attachment: m.attachment,
                interleaved: m.interleaved,
                modalities_input: mod_in,
                modalities_output: mod_out,
                cost_input: cost.input,
                cost_output: cost.output,
                cost_cache_read: cost.cache_read,
                cost_cache_write: cost.cache_write,
                cost,
                provider_override: m.provider,
                experimental_modes,
            };

            out.models.insert(key, entry);
        }
    }

    out
}

// ---------------------------------------------------------------------------
// ModelRegistry
// ---------------------------------------------------------------------------

/// In-memory registry of every known provider and model.
///
/// Hydrated on construction from the embedded models.dev snapshot, then
/// optionally overlaid from disk cache and refreshed from the network.
pub struct ModelRegistry {
    /// Keyed by `"provider_id/model_id"`.
    entries: HashMap<String, ModelEntry>,
    /// Keyed by provider id.
    providers: HashMap<String, ProviderEntry>,
    /// Optional path for on-disk persistence.
    cache_path: Option<PathBuf>,
    /// Minimum age before a network refresh is attempted again (mtime-based).
    refresh_interval: Duration,
}

impl ModelRegistry {
    /// Create a new registry pre-populated with the bundled snapshot.
    pub fn new() -> Self {
        let mut registry = Self {
            entries: HashMap::new(),
            providers: HashMap::new(),
            cache_path: None,
            refresh_interval: Duration::from_secs(5 * 60),
        };
        registry.load_bundled_snapshot();
        registry
    }

    /// Configure a cache file path for persistence between sessions.
    pub fn with_cache_path(mut self, path: PathBuf) -> Self {
        self.cache_path = Some(path);
        self
    }

    fn load_bundled_snapshot(&mut self) {
        if let Some(parsed) = parse_snapshot_bytes(BUNDLED_SNAPSHOT) {
            self.entries = parsed.models;
            self.providers = parsed.providers;
            tracing::debug!(
                providers = self.providers.len(),
                models = self.entries.len(),
                "Loaded bundled models.dev snapshot"
            );
        } else {
            tracing::warn!("Embedded models snapshot failed to parse; registry empty");
        }
    }

    // -----------------------------------------------------------------------
    // Queries — models
    // -----------------------------------------------------------------------

    /// Get an entry by `"provider_id/model_id"` key.
    pub fn get(&self, provider_id: &str, model_id: &str) -> Option<&ModelEntry> {
        let key = format!("{}/{}", provider_id, model_id);
        self.entries.get(&key)
    }

    /// Resolve a model string into `(ProviderId, ModelId)`.
    ///
    /// Accepts either `"provider/model"` or a bare model name (which defaults
    /// to the Anthropic provider for backward-compat).
    pub fn resolve(s: &str) -> (ProviderId, ModelId) {
        if let Some((provider, model)) = s.split_once('/') {
            (ProviderId::new(provider), ModelId::new(model))
        } else {
            (ProviderId::new(ProviderId::ANTHROPIC), ModelId::new(s))
        }
    }

    /// Look up a bare model name across all registry entries and return the
    /// provider that owns it.  Returns `None` if the model is not found or
    /// if the model string already contains a `"provider/"` prefix.
    pub fn find_provider_for_model(&self, model_name: &str) -> Option<ProviderId> {
        if model_name.contains('/') {
            return None;
        }

        // Family-based heuristic FIRST: well-known model name prefixes always
        // map to their canonical provider.  Prevents gateway/proxy entries in
        // the registry from hijacking well-known models like claude-* or gpt-*.
        let canonical: Option<&'static str> = if model_name.starts_with("claude") {
            Some(ProviderId::ANTHROPIC)
        } else if model_name.starts_with("gpt-")
            || model_name.starts_with("o1")
            || model_name.starts_with("o3")
            || model_name.starts_with("o4")
        {
            Some(ProviderId::OPENAI)
        } else if model_name.starts_with("gemini") || model_name.starts_with("gemma") {
            Some(ProviderId::GOOGLE)
        } else if model_name.starts_with("deepseek") {
            Some(ProviderId::DEEPSEEK)
        } else if model_name.starts_with("mistral")
            || model_name.starts_with("codestral")
            || model_name.starts_with("pixtral")
        {
            Some(ProviderId::MISTRAL)
        } else if model_name.starts_with("grok") {
            Some(ProviderId::XAI)
        } else if model_name.starts_with("command-r") || model_name.starts_with("command-a") {
            Some(ProviderId::COHERE)
        } else if model_name.starts_with("sonar") {
            Some(ProviderId::PERPLEXITY)
        } else if model_name.starts_with("glm-") {
            Some(ProviderId::ZAI)
        } else {
            None
        };
        if let Some(pid) = canonical {
            return Some(ProviderId::new(pid));
        }

        // Exact match
        for entry in self.entries.values() {
            if &*entry.info.id == model_name {
                return Some(entry.info.provider_id.clone());
            }
        }

        // Prefix match (handles version suffixes)
        for entry in self.entries.values() {
            if (*entry.info.id).starts_with(model_name)
                || model_name.starts_with(&*entry.info.id)
            {
                return Some(entry.info.provider_id.clone());
            }
        }

        None
    }

    /// List all models for a given provider.
    pub fn list_by_provider(&self, provider_id: &str) -> Vec<&ModelEntry> {
        self.entries
            .values()
            .filter(|e| &*e.info.provider_id == provider_id)
            .collect()
    }

    /// List models for a provider, filtered to those that should appear in
    /// default UI listings (active/beta only — no alpha or deprecated).
    ///
    /// Set the `CLAURST_ENABLE_EXPERIMENTAL_MODELS=1` env var to also include
    /// alpha/deprecated entries.
    pub fn list_visible_by_provider(&self, provider_id: &str) -> Vec<&ModelEntry> {
        let show_all = std::env::var("CLAURST_ENABLE_EXPERIMENTAL_MODELS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        self.list_by_provider(provider_id)
            .into_iter()
            .filter(|e| show_all || e.status.is_listed_by_default())
            .collect()
    }

    /// Pick the best default model for a provider.
    ///
    /// **Selection rule** (mirrors opencode's "latest tag"):
    ///   1. Prefer non-alpha, non-deprecated models.
    ///   2. Prefer models matching well-known flagship-name patterns
    ///      (configured per-provider).
    ///   3. Prefer most recent `release_date`.
    ///   4. Tie-break by descending model id.
    pub fn best_model_for_provider(&self, provider_id: &str) -> Option<String> {
        let mut models = self.list_visible_by_provider(provider_id);
        if models.is_empty() {
            // Fallback: include alpha/deprecated rather than return nothing
            models = self.list_by_provider(provider_id);
        }
        if models.is_empty() {
            return None;
        }

        let priority_patterns = flagship_patterns_for(provider_id);

        models.sort_by(|a, b| {
            let id_a: &str = &a.info.id;
            let id_b: &str = &b.info.id;

            // 1. Flagship pattern index
            let prio_a = priority_patterns
                .iter()
                .position(|pat| id_a.contains(pat))
                .unwrap_or(usize::MAX);
            let prio_b = priority_patterns
                .iter()
                .position(|pat| id_b.contains(pat))
                .unwrap_or(usize::MAX);

            prio_a
                .cmp(&prio_b)
                .then_with(|| {
                    // 2. "latest" suffix bonus
                    let latest_a = if id_a.contains("latest") { 0u8 } else { 1 };
                    let latest_b = if id_b.contains("latest") { 0u8 } else { 1 };
                    latest_a.cmp(&latest_b)
                })
                .then_with(|| {
                    // 3. Newer release_date wins
                    let rd_a = a.release_date.as_deref().unwrap_or("");
                    let rd_b = b.release_date.as_deref().unwrap_or("");
                    rd_b.cmp(rd_a)
                })
                .then_with(|| id_b.cmp(id_a)) // 4. descending by id
        });

        models.first().map(|e| e.info.id.to_string())
    }

    /// Pick the best "small" (fast/cheap) model for a provider.
    pub fn best_small_model_for_provider(&self, provider_id: &str) -> Option<String> {
        let mut models = self.list_visible_by_provider(provider_id);
        if models.is_empty() {
            models = self.list_by_provider(provider_id);
        }
        if models.is_empty() {
            return None;
        }

        let small_priority = small_patterns_for(provider_id);

        models.sort_by(|a, b| {
            let id_a: &str = &a.info.id;
            let id_b: &str = &b.info.id;

            let prio_a = small_priority
                .iter()
                .position(|pat| id_a.contains(pat))
                .unwrap_or(usize::MAX);
            let prio_b = small_priority
                .iter()
                .position(|pat| id_b.contains(pat))
                .unwrap_or(usize::MAX);

            prio_a
                .cmp(&prio_b)
                .then_with(|| {
                    let rd_a = a.release_date.as_deref().unwrap_or("");
                    let rd_b = b.release_date.as_deref().unwrap_or("");
                    rd_b.cmp(rd_a)
                })
                .then_with(|| id_b.cmp(id_a))
        });

        models.first().map(|e| e.info.id.to_string())
    }

    /// List every entry in the registry.
    pub fn list_all(&self) -> Vec<&ModelEntry> {
        self.entries.values().collect()
    }

    /// Number of models in the registry.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` if the registry has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    // -----------------------------------------------------------------------
    // Queries — providers
    // -----------------------------------------------------------------------

    /// Get provider metadata by id.
    pub fn provider(&self, provider_id: &str) -> Option<&ProviderEntry> {
        self.providers.get(provider_id)
    }

    /// List all known providers (sorted by id for stable output).
    pub fn list_providers(&self) -> Vec<&ProviderEntry> {
        let mut v: Vec<&ProviderEntry> = self.providers.values().collect();
        v.sort_by(|a, b| (&*a.id).cmp(&*b.id));
        v
    }

    /// Number of providers in the registry.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    // -----------------------------------------------------------------------
    // Network refresh
    // -----------------------------------------------------------------------

    /// Resolve the models.dev source URL, honoring env-var overrides.
    fn source_url() -> String {
        std::env::var("CLAURST_MODELS_URL")
            .or_else(|_| std::env::var("MODELS_DEV_URL"))
            .unwrap_or_else(|_| "https://models.dev/api.json".to_string())
    }

    /// Whether the configured cache file is newer than the refresh interval.
    fn cache_is_fresh(&self) -> bool {
        let path = match &self.cache_path {
            Some(p) => p,
            None => return false,
        };
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return false,
        };
        let mtime = match meta.modified() {
            Ok(t) => t,
            Err(_) => return false,
        };
        match mtime.elapsed() {
            Ok(age) => age < self.refresh_interval,
            Err(_) => true, // future mtime → treat as fresh
        }
    }

    /// Attempt to refresh the registry from the models.dev public API.
    ///
    /// Returns `Ok(true)` if new data was fetched, `Ok(false)` if the cache
    /// was still fresh.  Honors `CLAURST_DISABLE_MODELS_FETCH`.  All network
    /// or parse failures are silenced — the bundled snapshot is always
    /// sufficient.
    pub async fn refresh_from_models_dev(&mut self) -> anyhow::Result<bool> {
        if std::env::var("CLAURST_DISABLE_MODELS_FETCH").is_ok() {
            return Ok(false);
        }
        if self.cache_is_fresh() {
            return Ok(false);
        }

        let url = Self::source_url();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;

        let resp = client.get(&url).send().await;
        match resp {
            Ok(r) if r.status().is_success() => {
                let text = r.text().await?;
                if let Some(parsed) = parse_snapshot_str(&text) {
                    self.entries.extend(parsed.models);
                    self.providers.extend(parsed.providers);
                    if let Some(ref path) = self.cache_path.clone() {
                        // Write the raw response so future loads can re-parse
                        // it identically.  Best-effort; ignore I/O errors.
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(path, &text);
                    }
                    return Ok(true);
                }
                Ok(false)
            }
            // Fail silently — bundled snapshot is sufficient.
            _ => Ok(false),
        }
    }

    // -----------------------------------------------------------------------
    // Cache persistence
    // -----------------------------------------------------------------------

    /// Load a previously saved cache file, merging entries into the registry.
    ///
    /// The cache file may be either:
    ///   1. The raw models.dev `api.json` response (providers at the top level), or
    ///   2. Our own serialized `HashMap<String, ModelEntry>` format (legacy).
    ///
    /// Both formats are tried in order so the background fetch can simply
    /// save the raw models.dev response to disk and this method will ingest
    /// it.
    pub fn load_cache(&mut self, path: &PathBuf) {
        let data = match std::fs::read_to_string(path) {
            Ok(d) => d,
            Err(_) => return,
        };

        // Try models.dev raw format first.
        if let Some(parsed) = parse_snapshot_str(&data) {
            // Only overwrite if the cache yielded at least one model — guards
            // against an accidentally-empty response wiping the bundled
            // snapshot.
            if !parsed.models.is_empty() {
                self.entries.extend(parsed.models);
                self.providers.extend(parsed.providers);
                return;
            }
        }

        // Legacy: our own serialized HashMap<String, ModelEntry> format.
        if let Ok(entries) = serde_json::from_str::<HashMap<String, ModelEntry>>(&data) {
            self.entries.extend(entries);
        }
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Provider-specific flagship / small model patterns
// ---------------------------------------------------------------------------

/// Substring patterns that mark a model as the flagship for its provider.
/// Earlier entries score higher.
fn flagship_patterns_for(provider_id: &str) -> &'static [&'static str] {
    match provider_id {
        "anthropic" | "amazon-bedrock" | "github-copilot" | "azure" | "google-vertex" => &[
            "claude-opus-4",
            "claude-sonnet-4",
            "claude-3-5-sonnet",
            "claude-sonnet-3",
        ],
        "openai" => &[
            "gpt-5.2-pro",
            "gpt-5.2",
            "gpt-5.1",
            "gpt-5",
            "gpt-4.1",
            "gpt-4o",
            "o4",
            "o3",
        ],
        "google" => &[
            "gemini-3.1-pro",
            "gemini-3-pro",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
        ],
        "deepseek" => &["deepseek-reasoner", "deepseek-v4-pro", "deepseek-chat"],
        "mistral" => &["mistral-large", "codestral", "mistral-medium", "devstral"],
        "xai" => &["grok-4", "grok-3", "grok-2"],
        "cohere" => &["command-a", "command-r-plus", "command-r"],
        "groq" => &[
            "llama-3.3-70b",
            "llama-3.1-70b",
            "qwen",
            "deepseek-r1",
        ],
        "cerebras" => &["llama-3.3-70b", "qwen-3-235b", "zai-glm"],
        "perplexity" => &["sonar-pro", "sonar-reasoning", "sonar"],
        "openrouter" => &[
            "anthropic/claude-sonnet-4",
            "anthropic/claude-opus-4",
            "openai/gpt-5",
            "openai/gpt-4o",
            "google/gemini",
        ],
        "zai" => &["glm-5.1", "glm-5", "glm-4.7"],
        "minimax" => &["minimax-m2"],
        "codex" | "openai-codex" => &["gpt-5.2-codex", "gpt-5.1-codex"],
        "ollama" | "lmstudio" | "lm-studio" | "llamacpp" | "llama-cpp" => &[
            "qwen3-coder",
            "qwen2.5-coder",
            "deepseek",
            "llama3.3",
            "llama3.1",
            "qwen2.5",
        ],
        _ => &[
            "sonnet",
            "opus",
            "gpt-5",
            "gpt-4o",
            "gemini-2.5-pro",
            "llama-3.3-70b",
            "command-r-plus",
            "latest",
        ],
    }
}

/// Substring patterns marking a model as the lightweight/cheap default.
fn small_patterns_for(provider_id: &str) -> &'static [&'static str] {
    match provider_id {
        "anthropic" | "amazon-bedrock" | "github-copilot" | "azure" => &[
            "claude-haiku-4",
            "claude-haiku-3-5",
            "claude-haiku",
        ],
        "openai" => &["gpt-5-mini", "gpt-4o-mini", "o4-mini", "o3-mini"],
        "google" => &["gemini-2.5-flash-lite", "gemini-2.5-flash", "gemini-2.0-flash"],
        "deepseek" => &["deepseek-v4-flash", "deepseek-chat"],
        "mistral" => &["mistral-small", "mistral-nemo"],
        "xai" => &["grok-3-mini", "grok-2-mini"],
        "cohere" => &["command-r7b", "command-r"],
        "groq" => &["llama-3.1-8b", "gemma2-9b"],
        "openrouter" => &[
            "anthropic/claude-haiku",
            "openai/gpt-4o-mini",
            "google/gemini-2.5-flash",
        ],
        "zai" => &["glm-5-turbo", "glm-4.7"],
        _ => &["mini", "haiku", "flash", "lite", "small", "nano"],
    }
}

// ---------------------------------------------------------------------------
// Dynamic model resolution helper
// ---------------------------------------------------------------------------

/// Resolve the effective model for a [`Config`], using the model registry to
/// dynamically pick the best available model for the active provider.
///
/// **Resolution order**:
///  1. If the user explicitly set `config.model`, use it verbatim.
///  2. Consult the model registry for the configured provider's best model.
///  3. Fall back to the hardcoded table in [`Config::effective_model()`].
pub fn effective_model_for_config(
    config: &claurst_core::config::Config,
    registry: &ModelRegistry,
) -> String {
    if config.model.is_some() {
        return config.effective_model().to_string();
    }

    if let Some(provider_id) = config.provider.as_deref() {
        if let Some(best) = registry.best_model_for_provider(provider_id) {
            return best;
        }
    }

    config.effective_model().to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_snapshot_loads() {
        let reg = ModelRegistry::new();
        // Empty would mean the embed or parser broke.
        assert!(!reg.is_empty(), "bundled snapshot must populate registry");
        assert!(reg.provider_count() > 0, "providers must be populated");
    }

    #[test]
    fn well_known_providers_present() {
        let reg = ModelRegistry::new();
        for pid in ["anthropic", "openai", "google", "openrouter", "groq"] {
            assert!(
                reg.provider(pid).is_some(),
                "expected provider {pid} in bundled snapshot"
            );
            assert!(
                !reg.list_by_provider(pid).is_empty(),
                "expected at least one model for provider {pid}"
            );
        }
    }

    #[test]
    fn anthropic_has_claude_models() {
        let reg = ModelRegistry::new();
        let models = reg.list_by_provider("anthropic");
        let has_claude = models.iter().any(|m| (*m.info.id).starts_with("claude"));
        assert!(has_claude, "anthropic should have at least one claude model");
    }

    #[test]
    fn best_model_for_anthropic_is_claude() {
        let reg = ModelRegistry::new();
        let best = reg.best_model_for_provider("anthropic");
        assert!(best.is_some(), "anthropic must have a default model");
        assert!(
            best.unwrap().contains("claude"),
            "anthropic default must be a claude variant"
        );
    }

    #[test]
    fn modalities_drive_vision() {
        let reg = ModelRegistry::new();
        if let Some(opus) = reg.list_by_provider("anthropic")
            .iter()
            .find(|m| (*m.info.id).contains("opus"))
        {
            // Opus models are multimodal — image input expected.
            assert!(
                opus.modalities_input.contains(&Modality::Image),
                "opus should accept image input"
            );
            assert!(opus.vision(), "opus.vision() must mirror image modality");
        }
    }

    #[test]
    fn find_provider_for_model_canonical() {
        let reg = ModelRegistry::new();
        assert_eq!(
            reg.find_provider_for_model("claude-sonnet-4-6"),
            Some(ProviderId::new("anthropic"))
        );
        assert_eq!(
            reg.find_provider_for_model("gpt-4o"),
            Some(ProviderId::new("openai"))
        );
        assert_eq!(
            reg.find_provider_for_model("gemini-2.5-pro"),
            Some(ProviderId::new("google"))
        );
    }

    #[test]
    fn provider_metadata_populated() {
        let reg = ModelRegistry::new();
        let anthropic = reg.provider("anthropic").expect("anthropic provider");
        assert_eq!(anthropic.name, "Anthropic");
        assert!(anthropic.env.iter().any(|e| e == "ANTHROPIC_API_KEY"));
    }
}
