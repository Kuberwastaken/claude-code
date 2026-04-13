// provider_types.rs — Unified request/response types shared across all
// provider implementations.
//
// These types form a provider-agnostic layer that every concrete provider
// adapter (Anthropic, OpenAI, Google, …) maps to/from.

use claurst_core::types::{ContentBlock, Message, ToolDefinition, UsageInfo};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Re-export ThinkingConfig and SystemPrompt from the api types module so
// callers only need to import from this module.
pub use crate::types::{SystemPrompt, ThinkingConfig};

// ---------------------------------------------------------------------------
// StopReason
// ---------------------------------------------------------------------------

/// The reason a model stopped generating tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model reached a natural stopping point.
    EndTurn,
    /// The model generated a stop sequence.
    StopSequence,
    /// The model hit the max_tokens limit.
    MaxTokens,
    /// The model made a tool/function call.
    ToolUse,
    /// Content was filtered by the provider's safety system.
    ContentFiltered,
    /// The provider returned an unknown or unrecognised stop reason.
    Other(String),
}

impl Default for StopReason {
    fn default() -> Self {
        StopReason::EndTurn
    }
}

// ---------------------------------------------------------------------------
// ProviderRequest
// ---------------------------------------------------------------------------

/// A normalised request that any provider adapter can consume.
///
/// Provider-specific parameters that cannot be expressed through the common
/// fields can be passed via `provider_options` as an arbitrary JSON object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRequest {
    /// The model identifier (e.g. `"claude-opus-4-5"`, `"gpt-4o"`).
    pub model: String,

    /// The conversation history to send to the model.
    pub messages: Vec<Message>,

    /// An optional system / developer prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<SystemPrompt>,

    /// Tool definitions available to the model for this turn.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,

    /// Maximum number of tokens to generate.
    pub max_tokens: u32,

    /// Sampling temperature (provider-dependent range, typically 0.0–1.0 or 0.0–2.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// Nucleus sampling probability mass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    /// Top-k sampling cutoff.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,

    /// Sequences that cause the model to stop generating.
    #[serde(default)]
    pub stop_sequences: Vec<String>,

    /// Extended thinking / chain-of-thought configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,

    /// Arbitrary provider-specific options merged into the request body.
    /// Defaults to an empty JSON object `{}`.
    #[serde(default)]
    pub provider_options: Value,
}

// ---------------------------------------------------------------------------
// ProviderResponse
// ---------------------------------------------------------------------------

/// A normalised response returned by any provider adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResponse {
    /// Provider-assigned message / request identifier.
    pub id: String,

    /// The generated content blocks.
    pub content: Vec<ContentBlock>,

    /// Why the model stopped generating.
    pub stop_reason: StopReason,

    /// Token usage for billing / budget tracking.
    pub usage: UsageInfo,

    /// The model that produced this response (as reported by the provider).
    pub model: String,
}

// ---------------------------------------------------------------------------
// StreamEvent
// ---------------------------------------------------------------------------

/// Events emitted by the provider-agnostic streaming layer.
///
/// Each provider's SSE/websocket parser maps its wire format onto these events
/// so that the rest of the application can consume a single unified stream.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// The message has started; carries the provider-assigned id and model.
    MessageStart {
        id: String,
        model: String,
        usage: UsageInfo,
    },

    /// A new content block is beginning.
    ContentBlockStart {
        index: usize,
        content_block: ContentBlock,
    },

    /// Incremental text delta for an in-progress block.
    TextDelta { index: usize, text: String },

    /// Incremental thinking / reasoning delta.
    ThinkingDelta { index: usize, thinking: String },

    /// Incremental delta for tool-call JSON arguments.
    InputJsonDelta { index: usize, partial_json: String },

    /// Incremental delta for a cryptographic signature block.
    SignatureDelta { index: usize, signature: String },

    /// An in-progress content block is now complete.
    ContentBlockStop { index: usize },

    /// Final message-level delta carrying the stop reason and updated usage.
    MessageDelta {
        stop_reason: Option<StopReason>,
        usage: Option<UsageInfo>,
    },

    /// The message stream is fully complete.
    MessageStop,

    /// A provider-level error occurred mid-stream.
    Error { error_type: String, message: String },

    /// Incremental reasoning / scratchpad delta (alias used by some providers).
    ReasoningDelta { index: usize, reasoning: String },
}

// ---------------------------------------------------------------------------
// ProviderCapabilities
// ---------------------------------------------------------------------------

/// Describes the features supported by a particular provider/model combination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Provider supports streaming responses via SSE or websocket.
    pub streaming: bool,

    /// Provider supports function / tool calling.
    pub tool_calling: bool,

    /// Provider supports extended thinking / chain-of-thought tokens.
    pub thinking: bool,

    /// Provider accepts image inputs.
    pub image_input: bool,

    /// Provider accepts PDF document inputs.
    pub pdf_input: bool,

    /// Provider accepts audio inputs.
    pub audio_input: bool,

    /// Provider accepts video inputs.
    pub video_input: bool,

    /// Provider supports prompt caching.
    pub caching: bool,

    /// Provider supports JSON-schema-constrained structured output.
    pub structured_output: bool,

    /// How the provider expects the system prompt to be delivered.
    pub system_prompt_style: SystemPromptStyle,
}

/// Describes where/how a provider expects the system prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemPromptStyle {
    /// Delivered as a top-level `system` field in the request body (Anthropic style).
    TopLevel,
    /// Delivered as a `{"role": "system", "content": "…"}` message at index 0 (OpenAI style).
    SystemMessage,
    /// Delivered as a `system_instruction` field (Google Gemini style).
    SystemInstruction,
}

// ---------------------------------------------------------------------------
// ProviderStatus
// ---------------------------------------------------------------------------

/// The current health status of a provider endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ProviderStatus {
    /// The provider is operating normally.
    Healthy,
    /// The provider is reachable but experiencing elevated errors or latency.
    Degraded { reason: String },
    /// The provider is unreachable or has been disabled.
    Unavailable { reason: String },
}

// ---------------------------------------------------------------------------
// AuthMethod
// ---------------------------------------------------------------------------

/// The authentication mechanism used to talk to a provider endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum AuthMethod {
    /// A static API key sent as an HTTP header.
    ApiKey { key: String, header: ApiKeyHeader },

    /// A bearer token sent in the `Authorization` header.
    Bearer { token: String },

    /// AWS Signature V4 credentials for Amazon Bedrock.
    AwsCredentials {
        #[serde(skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
        region: String,
        /// Optional bearer token for cross-account or SSO scenarios.
        #[serde(skip_serializing_if = "Option::is_none")]
        bearer_token: Option<String>,
    },

    /// OAuth 2.0 access + refresh token pair.
    OAuth {
        access_token: String,
        refresh_token: String,
        /// Unix timestamp (seconds) when the access token expires.
        expires_at: u64,
    },

    /// No authentication required (e.g. local Ollama).
    None,
}

/// Which HTTP header carries the API key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyHeader {
    /// `x-api-key: <key>` (Anthropic, Mistral, …)
    XApiKey,
    /// `Authorization: Bearer <key>` (OpenAI, Groq, …)
    Authorization,
    /// A custom header name.
    Custom(String),
}
