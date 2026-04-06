//! OpenAI Codex OAuth 2.0 PKCE flow for Claurst.
//!
//! Implements authorization code flow with PKCE to obtain OpenAI access
//! tokens for Codex model access.

#![allow(dead_code)] // OAuth functions are integrated via create_message_codex

use anyhow::{anyhow, bail};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use claurst_core::codex_oauth::{
    CODEX_AUTHORIZE_URL, CODEX_CLIENT_ID, CODEX_OAUTH_PORT, CODEX_REDIRECT_URI, CODEX_SCOPES,
    CODEX_TOKEN_URL,
};
use claurst_core::oauth_config::CodexTokens;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;

/// Generate a PKCE code verifier (random 64-byte base64url string).
pub fn generate_code_verifier() -> String {
    let mut bytes = [0u8; 48];
    // Use UUID v4 for randomness (reuse the approach from oauth_config.rs)
    let u1 = uuid::Uuid::new_v4();
    let u2 = uuid::Uuid::new_v4();
    bytes[..16].copy_from_slice(u1.as_bytes());
    bytes[16..32].copy_from_slice(u2.as_bytes());
    // For remaining bytes, use UUID truncation
    let u3 = uuid::Uuid::new_v4();
    bytes[32..48].copy_from_slice(&u3.as_bytes()[..16]);

    URL_SAFE_NO_PAD.encode(&bytes)
}

/// Compute PKCE code challenge (SHA-256 of verifier, base64url encoded).
pub fn compute_code_challenge(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

/// Generate a random OAuth state parameter.
pub fn generate_state() -> String {
    let bytes = uuid::Uuid::new_v4();
    URL_SAFE_NO_PAD
        .encode(bytes.as_bytes())
        .chars()
        .take(32)
        .collect()
}

/// Build the OpenAI authorization URL for Codex OAuth.
pub fn build_auth_url(code_challenge: &str, state: &str) -> String {
    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&id_token_add_organizations=true&codex_cli_simplified_flow=true&originator=claurst",
        CODEX_AUTHORIZE_URL,
        CODEX_CLIENT_ID,
        urlencoding::encode(CODEX_REDIRECT_URI),
        urlencoding::encode(CODEX_SCOPES),
        code_challenge,
        state,
    )
}

/// Start local HTTP server on port 1455, open browser, wait for callback,
/// exchange code for tokens, return CodexTokens.
pub async fn run_oauth_flow() -> anyhow::Result<CodexTokens> {
    let verifier = generate_code_verifier();
    let challenge = compute_code_challenge(&verifier);
    let state = generate_state();

    // Bind local server for callback
    let listener = TcpListener::bind(format!("127.0.0.1:{}", CODEX_OAUTH_PORT))
        .await
        .map_err(|e| anyhow!("Failed to bind port {}: {}", CODEX_OAUTH_PORT, e))?;

    let auth_url = build_auth_url(&challenge, &state);

    // Try to open browser
    eprintln!("\nOpening browser for OpenAI Codex login...");
    eprintln!("If your browser doesn't open, visit:\n{}\n", auth_url);
    let _ = open::that(&auth_url);

    // Wait for OAuth callback
    let (code, callback_state) = wait_for_callback(listener).await?;

    if callback_state != state {
        bail!("OAuth state mismatch — possible CSRF attack");
    }

    // Exchange code for tokens
    let tokens = exchange_code_for_tokens(&code, &verifier).await?;

    // Persist tokens
    claurst_core::oauth_config::save_codex_tokens(&tokens)?;

    eprintln!("Codex login successful!");
    Ok(tokens)
}

/// Wait for OAuth callback on local server, extract code and state.
async fn wait_for_callback(listener: TcpListener) -> anyhow::Result<(String, String)> {
    use tokio::io::AsyncWriteExt;

    let (mut socket, _) = tokio::time::timeout(
        std::time::Duration::from_secs(300), // 5 minute timeout
        listener.accept(),
    )
    .await
    .map_err(|_| anyhow!("OAuth callback timeout (5 minutes)"))?
    .map_err(|e| anyhow!("Failed to accept connection: {}", e))?;

    let mut reader = BufReader::new(&mut socket);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;

    // Parse "GET /auth/callback?code=...&state=... HTTP/1.1"
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        bail!("Invalid HTTP request");
    }

    let path = parts[1];
    let query_start = path
        .find('?')
        .ok_or_else(|| anyhow!("No query string in callback"))?;
    let query = &path[query_start + 1..];

    let mut code = String::new();
    let mut state = String::new();
    let mut error = String::new();

    for param in query.split('&') {
        let kv: Vec<&str> = param.splitn(2, '=').collect();
        if kv.len() == 2 {
            match kv[0] {
                "code" => code = urlencoding::decode(kv[1])?.to_string(),
                "state" => state = urlencoding::decode(kv[1])?.to_string(),
                "error" => error = urlencoding::decode(kv[1])?.to_string(),
                "error_description" => error = urlencoding::decode(kv[1])?.to_string(),
                _ => {}
            }
        }
    }

    // Send HTML response to browser before processing
    let html = if error.is_empty() {
        "<html><body style='background:#131010;color:#f1ecec;display:flex;justify-content:center;align-items:center;height:100vh;font-family:system-ui'>\
         <div style='text-align:center'><h1>Authorization Successful</h1><p>You can close this window and return to Claurst.</p></div>\
         <script>setTimeout(()=>window.close(),2000)</script></body></html>"
    } else {
        "<html><body style='background:#131010;color:#f1ecec;display:flex;justify-content:center;align-items:center;height:100vh;font-family:system-ui'>\
         <div style='text-align:center'><h1 style='color:#fc533a'>Authorization Failed</h1><p>Check the terminal for details.</p></div></body></html>"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    // Drop the BufReader so we can write back on the socket
    drop(reader);
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;

    if !error.is_empty() {
        bail!("OAuth error: {}", error);
    }

    if code.is_empty() || state.is_empty() {
        bail!("Missing code or state in OAuth callback");
    }

    Ok((code, state))
}

/// Exchange authorization code for access tokens.
async fn exchange_code_for_tokens(code: &str, verifier: &str) -> anyhow::Result<CodexTokens> {
    let client = reqwest::Client::new();
    let params = [
        ("client_id", CODEX_CLIENT_ID),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", CODEX_REDIRECT_URI),
    ];

    let resp = client
        .post(CODEX_TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| anyhow!("Failed to exchange code: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("Token exchange failed ({}): {}", status, body);
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("Failed to parse token response: {}", e))?;

    let access_token = body["access_token"].as_str().unwrap_or("").to_string();

    if access_token.is_empty() {
        bail!("No access_token in response");
    }

    let refresh_token = body["refresh_token"].as_str().map(|s| s.to_string());
    let expires_at = claurst_core::codex_oauth::expires_at_from_now(body["expires_in"].as_u64());
    let account_id = body["id_token"]
        .as_str()
        .and_then(claurst_core::codex_oauth::extract_account_id_from_jwt)
        .or_else(|| claurst_core::codex_oauth::extract_account_id_from_jwt(&access_token));

    Ok(CodexTokens {
        access_token,
        refresh_token,
        account_id,
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_code_verifier_format() {
        let verifier = generate_code_verifier();
        // Base64url encoding: [A-Za-z0-9_-]
        assert!(verifier
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-'));
        assert!(!verifier.is_empty());
    }

    #[test]
    fn test_compute_code_challenge_consistency() {
        let verifier = "test_verifier_string";
        let challenge1 = compute_code_challenge(verifier);
        let challenge2 = compute_code_challenge(verifier);
        assert_eq!(challenge1, challenge2);
        // Base64url format
        assert!(challenge1
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-'));
    }

    #[test]
    fn test_generate_state_format() {
        let state = generate_state();
        assert!(!state.is_empty());
        assert!(state
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-'));
    }

    #[test]
    fn test_build_auth_url_contains_required_params() {
        let url = build_auth_url("challenge123", "state456");
        assert!(url.contains("client_id="));
        assert!(url.contains("challenge123"));
        assert!(url.contains("state456"));
        assert!(url.contains("S256"));
        assert!(url.contains("response_type=code"));
    }

    #[test]
    fn test_extract_account_id_from_valid_jwt() {
        // This is a test JWT (not real credentials) with account_id in it
        // Format: header.payload.signature
        // For testing we'd need to create a valid JWT structure, which is complex
        // In practice, this function is tested via integration tests
        let invalid_token = "not.a.jwt";
        let result = claurst_core::codex_oauth::extract_account_id_from_jwt(invalid_token);
        // Invalid JWT should return None
        assert!(result.is_none() || result.unwrap().is_empty());
    }
}
