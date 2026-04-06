//! OpenAI Codex OAuth configuration and constants.
//!

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::Value;

/// OpenAI Codex OAuth client ID (shared with the OpenCode ecosystem).
pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// OpenAI OAuth issuer base URL.
pub const CODEX_ISSUER: &str = "https://auth.openai.com";

/// OpenAI OAuth authorization endpoint
pub const CODEX_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";

/// OpenAI OAuth token endpoint
pub const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

/// Codex Responses API endpoint (used for inference after login)
pub const CODEX_API_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";

/// Local redirect URI for OAuth callback
pub const CODEX_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";

/// OAuth callback port
pub const CODEX_OAUTH_PORT: u16 = 1455;

/// OAuth scopes requested from OpenAI
pub const CODEX_SCOPES: &str = "openid profile email offline_access";

/// Available Codex models
pub const CODEX_MODELS: &[(&str, &str)] = &[
    ("gpt-5.2-codex", "GPT-5.2 Codex (default)"),
    ("gpt-5.1-codex", "GPT-5.1 Codex"),
    ("gpt-5.1-codex-mini", "GPT-5.1 Codex Mini"),
    ("gpt-5.1-codex-max", "GPT-5.1 Codex Max"),
    ("gpt-5.4", "GPT-5.4"),
    ("gpt-5.2", "GPT-5.2"),
];

/// Default Codex model to use
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.2-codex";

pub fn extract_account_id_from_jwt(token: &str) -> Option<String> {
    let payload_b64 = token.split('.').nth(1)?;
    let payload = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let json: Value = serde_json::from_slice(&payload).ok()?;

    json["chatgpt_account_id"]
        .as_str()
        .or_else(|| json["https://api.openai.com/auth"]["chatgpt_account_id"].as_str())
        .or_else(|| json["https://api.openai.com/auth"]["account_id"].as_str())
        .or_else(|| {
            json["organizations"]
                .as_array()
                .and_then(|orgs| orgs.first())
                .and_then(|org| org["id"].as_str())
        })
        .map(|s| s.to_string())
}

pub fn expires_at_from_now(expires_in_secs: Option<u64>) -> Option<u64> {
    expires_in_secs.map(|secs| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(secs)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codex_constants_not_empty() {
        assert!(!CODEX_CLIENT_ID.is_empty());
        assert!(!CODEX_AUTHORIZE_URL.is_empty());
        assert!(!CODEX_TOKEN_URL.is_empty());
        assert!(!CODEX_REDIRECT_URI.is_empty());
        assert!(!CODEX_SCOPES.is_empty());
        assert!(!CODEX_MODELS.is_empty());
        assert!(!DEFAULT_CODEX_MODEL.is_empty());
    }

    #[test]
    fn test_codex_models_contains_default() {
        let default_found = CODEX_MODELS
            .iter()
            .any(|(model, _)| model == &DEFAULT_CODEX_MODEL);
        assert!(
            default_found,
            "DEFAULT_CODEX_MODEL must be in CODEX_MODELS list"
        );
    }

    #[test]
    fn test_redirect_uri_is_localhost() {
        assert!(CODEX_REDIRECT_URI.contains("localhost:1455"));
    }

    #[test]
    fn test_extract_account_id_from_auth_claims() {
        let payload = r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_123"}}"#;
        let token = format!("a.{}.c", URL_SAFE_NO_PAD.encode(payload.as_bytes()));
        assert_eq!(
            extract_account_id_from_jwt(&token).as_deref(),
            Some("acct_123")
        );
    }

    #[test]
    fn test_extract_account_id_from_organizations() {
        let payload = r#"{"organizations":[{"id":"org_123"}]}"#;
        let token = format!("a.{}.c", URL_SAFE_NO_PAD.encode(payload.as_bytes()));
        assert_eq!(
            extract_account_id_from_jwt(&token).as_deref(),
            Some("org_123")
        );
    }

    #[test]
    fn test_expires_at_from_now_sets_future_time() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expires_at = expires_at_from_now(Some(3600)).unwrap();
        assert!(expires_at >= now + 3599);
    }
}
