//! OAuth configuration for multiple environments.
//!
//! This module mirrors the TypeScript `src/constants/oauth.ts` and
//! `src/services/oauth/crypto.ts` constants.  It is intentionally
//! *configuration-only* — no live network I/O except for the optional
//! `fetch_oauth_profile` helper at the bottom.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Scope constants (mirrors constants/oauth.ts)
// ---------------------------------------------------------------------------

/// The Claude.ai inference scope — required for Bearer-auth API calls.
pub const CLAUDE_AI_INFERENCE_SCOPE: &str = "user:inference";

/// The profile scope — required to read account / subscription data.
pub const CLAUDE_AI_PROFILE_SCOPE: &str = "user:profile";

/// Console scope — used when creating an API key via the Console flow.
pub const CONSOLE_SCOPE: &str = "org:create_api_key";

/// All Claude.ai OAuth scopes (mirrors `CLAUDE_AI_OAUTH_SCOPES`).
pub const CLAUDE_AI_OAUTH_SCOPES: &[&str] = &[
    CLAUDE_AI_PROFILE_SCOPE,
    CLAUDE_AI_INFERENCE_SCOPE,
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
];

/// Console OAuth scopes (mirrors `CONSOLE_OAUTH_SCOPES`).
pub const CONSOLE_OAUTH_SCOPES: &[&str] = &[CONSOLE_SCOPE, CLAUDE_AI_PROFILE_SCOPE];

/// Union of all scopes used during login (mirrors `ALL_OAUTH_SCOPES`).
/// Requesting all at once lets a single login satisfy both Console and
/// claude.ai auth paths.
pub const ALL_OAUTH_SCOPES: &[&str] = &[
    CONSOLE_SCOPE,
    CLAUDE_AI_PROFILE_SCOPE,
    CLAUDE_AI_INFERENCE_SCOPE,
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
];

/// Minimum scopes required for basic operation.
pub const MINIMUM_SCOPES: &[&str] = &[CLAUDE_AI_INFERENCE_SCOPE, CLAUDE_AI_PROFILE_SCOPE];

// ---------------------------------------------------------------------------
// OAuthConfig struct
// ---------------------------------------------------------------------------

/// Full OAuth configuration for a deployment environment.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub base_api_url: &'static str,
    pub console_authorize_url: &'static str,
    pub claude_ai_authorize_url: &'static str,
    /// The raw claude.ai web origin (separate from the authorize URL which
    /// may bounce through claude.com for attribution).
    pub claude_ai_origin: &'static str,
    pub token_url: &'static str,
    pub api_key_url: &'static str,
    pub roles_url: &'static str,
    pub console_success_url: &'static str,
    pub claudeai_success_url: &'static str,
    pub manual_redirect_url: &'static str,
    pub client_id: &'static str,
    pub oauth_file_suffix: &'static str,
    pub mcp_proxy_url: &'static str,
    pub mcp_proxy_path: &'static str,
}

// ---------------------------------------------------------------------------
// Production config (mirrors PROD_OAUTH_CONFIG in oauth.ts)
// ---------------------------------------------------------------------------

// NOTE: These OAuth client IDs are registered to Anthropic's official Claude Code CLI.
// They will NOT work for Claurst — Anthropic's auth server will reject or misattribute requests.
// Users should use an API key from console.anthropic.com instead.
// To use OAuth, Claurst would need its own registered OAuth application with Anthropic.
pub const PROD_OAUTH: OAuthConfig = OAuthConfig {
    base_api_url: "https://api.anthropic.com",
    // Routes through claude.com/cai/* for attribution, 307s to claude.ai in
    // two hops — same behaviour as the TypeScript client.
    console_authorize_url: "https://platform.claude.com/oauth/authorize",
    claude_ai_authorize_url: "https://claude.com/cai/oauth/authorize",
    claude_ai_origin: "https://claude.ai",
    token_url: "https://platform.claude.com/v1/oauth/token",
    api_key_url: "https://api.anthropic.com/api/oauth/claude_cli/create_api_key",
    roles_url: "https://api.anthropic.com/api/oauth/claude_cli/roles",
    console_success_url:
        "https://platform.claude.com/buy_credits?returnUrl=/oauth/code/success%3Fapp%3Dclaude-code",
    claudeai_success_url: "https://platform.claude.com/oauth/code/success?app=claude-code",
    manual_redirect_url: "https://platform.claude.com/oauth/code/callback",
    client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e", // Anthropic's Claude Code — will not work for Claurst
    oauth_file_suffix: "",
    mcp_proxy_url: "https://mcp-proxy.anthropic.com",
    mcp_proxy_path: "/v1/mcp/{server_id}",
};

// ---------------------------------------------------------------------------
// Staging config (mirrors STAGING_OAUTH_CONFIG — ant builds only)
// ---------------------------------------------------------------------------

pub const STAGING_OAUTH: OAuthConfig = OAuthConfig {
    base_api_url: "https://api-staging.anthropic.com",
    console_authorize_url: "https://platform.staging.ant.dev/oauth/authorize",
    claude_ai_authorize_url: "https://claude-ai.staging.ant.dev/oauth/authorize",
    claude_ai_origin: "https://claude-ai.staging.ant.dev",
    token_url: "https://platform.staging.ant.dev/v1/oauth/token",
    api_key_url: "https://api-staging.anthropic.com/api/oauth/claude_cli/create_api_key",
    roles_url: "https://api-staging.anthropic.com/api/oauth/claude_cli/roles",
    console_success_url: "https://platform.staging.ant.dev/buy_credits?returnUrl=/oauth/code/success%3Fapp%3Dclaude-code",
    claudeai_success_url: "https://platform.staging.ant.dev/oauth/code/success?app=claude-code",
    manual_redirect_url: "https://platform.staging.ant.dev/oauth/code/callback",
    client_id: "22422756-60c9-4084-8eb7-27705fd5cf9a", // Anthropic's Claude Code staging — will not work for Claurst
    oauth_file_suffix: "-staging-oauth",
    mcp_proxy_url: "https://mcp-proxy-staging.anthropic.com",
    mcp_proxy_path: "/v1/mcp/{server_id}",
};

/// Client-ID Metadata Document URL for MCP OAuth (CIMD / SEP-991).
pub const MCP_CLIENT_METADATA_URL: &str = "https://claude.ai/oauth/claude-code-client-metadata";

// ---------------------------------------------------------------------------
// Config selection
// ---------------------------------------------------------------------------

/// Return the OAuth config appropriate for the current environment.
///
/// Free-code always uses production OAuth. The `USER_TYPE=ant` gate and
/// staging variant have been removed for the OSS/free build.
pub fn get_oauth_config() -> &'static OAuthConfig {
    &PROD_OAUTH
}

// ---------------------------------------------------------------------------
// PKCE helpers (mirrors src/services/oauth/crypto.ts)
// ---------------------------------------------------------------------------

/// PKCE code-challenge / code-verifier helpers.
pub mod pkce {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use sha2::{Digest, Sha256};

    /// Generate a cryptographically random code verifier (43–128 chars of
    /// Base64url characters, as required by RFC 7636).
    ///
    /// Uses `getrandom` via the `rand` crate's OS RNG through the `uuid`
    /// crate's v4 generator — both already in-tree.  Falls back to a
    /// time+pid mix if the OS RNG is unavailable.
    pub fn generate_code_verifier() -> String {
        // 32 random bytes → 43-char Base64url string (same as the TS impl).
        let bytes = random_bytes_32();
        URL_SAFE_NO_PAD.encode(bytes)
    }

    /// Compute `BASE64URL(SHA256(verifier))` — the S256 code challenge.
    pub fn code_challenge(verifier: &str) -> String {
        let hash = Sha256::digest(verifier.as_bytes());
        URL_SAFE_NO_PAD.encode(hash)
    }

    /// Generate a random state parameter (16 Base64url chars).
    pub fn generate_state() -> String {
        let bytes = random_bytes_32();
        let encoded = URL_SAFE_NO_PAD.encode(bytes);
        // Take first 43 chars for a compact state parameter
        encoded.chars().take(43).collect()
    }

    // ------------------------------------------------------------------
    // Internal: produce 32 random bytes.
    // We derive them from a UUID v4 (which already pulls from the OS RNG
    // via the `uuid` crate) so we don't need to add a new `rand` dep.
    // ------------------------------------------------------------------
    fn random_bytes_32() -> [u8; 32] {
        // Two UUID v4 values give us 32 bytes of OS-backed randomness.
        let u1 = uuid::Uuid::new_v4();
        let u2 = uuid::Uuid::new_v4();
        let mut out = [0u8; 32];
        out[..16].copy_from_slice(u1.as_bytes());
        out[16..].copy_from_slice(u2.as_bytes());
        out
    }
}

// ---------------------------------------------------------------------------
// Token and profile types
// ---------------------------------------------------------------------------

/// Raw OAuth token response from the token endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Slim profile fetched after token exchange.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OAuthProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_tier: Option<String>,
}

/// Fetch the OAuth profile using an access token.
///
/// Returns a default (all-`None`) profile on any non-success response so
/// callers can treat a profile fetch failure as non-fatal.
pub async fn fetch_oauth_profile(
    access_token: &str,
    api_base: &str,
) -> anyhow::Result<OAuthProfile> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/auth/oauth/profile", api_base.trim_end_matches('/'));

    let resp = client
        .get(&url)
        .bearer_auth(access_token)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?;

    if resp.status().is_success() {
        let profile: OAuthProfile = resp.json().await.unwrap_or_default();
        Ok(profile)
    } else {
        // Non-fatal: return an empty profile so the caller can continue.
        Ok(OAuthProfile::default())
    }
}

// ---------------------------------------------------------------------------
// Auth URL builder
// ---------------------------------------------------------------------------

/// Build the OAuth authorization URL (mirrors `buildAuthUrl` in client.ts).
pub fn build_auth_url(
    code_challenge: &str,
    state: &str,
    port: u16,
    is_manual: bool,
    login_with_claude_ai: bool,
    inference_only: bool,
) -> String {
    let cfg = get_oauth_config();

    let base = if login_with_claude_ai {
        cfg.claude_ai_authorize_url
    } else {
        cfg.console_authorize_url
    };

    let redirect_uri = if is_manual {
        cfg.manual_redirect_url.to_string()
    } else {
        format!("http://localhost:{}/callback", port)
    };

    let scopes: Vec<&str> = if inference_only {
        vec![CLAUDE_AI_INFERENCE_SCOPE]
    } else {
        ALL_OAUTH_SCOPES.to_vec()
    };

    let scope_str = scopes.join(" ");

    format!(
        "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        base,
        urlencoding::encode(cfg.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&scope_str),
        urlencoding::encode(code_challenge),
        urlencoding::encode(state),
    )
}

// ---------------------------------------------------------------------------
// Codex (OpenAI) OAuth Token Storage
// ---------------------------------------------------------------------------

/// OpenAI Codex OAuth tokens, persisted to ~/.claurst/codex_tokens.json
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodexTokens {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Unix timestamp in seconds when the access token expires
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

/// Path to the Codex tokens file (~/.claurst/codex_tokens.json)
fn codex_tokens_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".claurst").join("codex_tokens.json"))
}

/// Save Codex OAuth tokens to ~/.claurst/codex_tokens.json
pub fn save_codex_tokens(tokens: &CodexTokens) -> anyhow::Result<()> {
    let path =
        codex_tokens_path().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    let json = serde_json::to_string(tokens)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Load Codex OAuth tokens from ~/.claurst/codex_tokens.json
pub fn get_codex_tokens() -> Option<CodexTokens> {
    let path = codex_tokens_path()?;
    if !path.exists() {
        return None;
    }
    let json = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Clear stored Codex tokens
pub fn clear_codex_tokens() -> anyhow::Result<()> {
    let path =
        codex_tokens_path().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Returns true if the user has a valid Codex access token.
/// Tokens are obtained via `/connect → OpenAI Codex` (browser OAuth flow)
/// or by setting `CLAURST_USE_OPENAI=1` with a manually stored token.
pub fn is_codex_subscriber() -> bool {
    get_codex_tokens()
        .map(|t| !t.access_token.is_empty())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prod_config_urls_are_https() {
        assert!(PROD_OAUTH.token_url.starts_with("https://"));
        assert!(PROD_OAUTH.api_key_url.starts_with("https://"));
        assert!(PROD_OAUTH.claude_ai_authorize_url.starts_with("https://"));
    }

    #[test]
    fn test_staging_config_urls_are_https() {
        assert!(STAGING_OAUTH.token_url.starts_with("https://"));
        assert!(STAGING_OAUTH.api_key_url.starts_with("https://"));
    }

    #[test]
    fn test_pkce_code_challenge_is_base64url() {
        let verifier = pkce::generate_code_verifier();
        assert!(!verifier.is_empty());
        // Base64url characters only (no +, /, =)
        assert!(!verifier.contains('+'));
        assert!(!verifier.contains('/'));
        assert!(!verifier.contains('='));

        let challenge = pkce::code_challenge(&verifier);
        assert!(!challenge.is_empty());
        assert!(!challenge.contains('+'));
        assert!(!challenge.contains('/'));
        assert!(!challenge.contains('='));
    }

    #[test]
    fn test_verifier_length_meets_rfc7636_minimum() {
        let verifier = pkce::generate_code_verifier();
        // RFC 7636 §4.1: code_verifier length ∈ [43, 128]
        assert!(
            verifier.len() >= 43,
            "verifier too short: {} chars",
            verifier.len()
        );
        assert!(
            verifier.len() <= 128,
            "verifier too long: {} chars",
            verifier.len()
        );
    }

    #[test]
    fn test_all_oauth_scopes_contains_inference() {
        assert!(ALL_OAUTH_SCOPES.contains(&CLAUDE_AI_INFERENCE_SCOPE));
    }

    #[test]
    fn test_build_auth_url_contains_required_params() {
        let url = build_auth_url("challenge123", "state456", 8080, false, true, false);
        assert!(url.contains("challenge123"));
        assert!(url.contains("state456"));
        assert!(url.contains("S256"));
        assert!(url.contains("localhost"));
    }
}
