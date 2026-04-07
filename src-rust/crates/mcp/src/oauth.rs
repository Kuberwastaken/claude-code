//! MCP OAuth / XAA IdP login flow.
//! Mirrors src/services/mcp/xaaIdpLogin.ts and src/services/mcp/auth.ts.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Token storage
// ---------------------------------------------------------------------------

/// An OAuth access token with expiry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix timestamp (seconds) when the token expires.
    pub expires_at: Option<u64>,
    pub scope: Option<String>,
    pub server_name: String,
}

impl McpToken {
    /// Returns `true` if the token is expired or will expire within `grace_secs`.
    pub fn is_expired(&self, grace_secs: u64) -> bool {
        let Some(exp) = self.expires_at else {
            return false;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now + grace_secs >= exp
    }
}

/// Path to the token store for a given MCP server.
fn token_path(server_name: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".claurst/mcp-tokens")
        .join(format!("{}.json", server_name))
}

/// Persist an MCP OAuth token to disk.
pub fn store_mcp_token(token: &McpToken) -> std::io::Result<()> {
    let path = token_path(&token.server_name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(token)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&path, json)
}

/// Read a stored MCP OAuth token (None if not found or invalid).
pub fn get_mcp_token(server_name: &str) -> Option<McpToken> {
    let path = token_path(server_name);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Delete the stored token for a server (effectively logs out).
pub fn remove_mcp_token(server_name: &str) -> std::io::Result<()> {
    let path = token_path(server_name);
    if path.exists() {
        std::fs::remove_file(&path)
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PKCE helpers
// ---------------------------------------------------------------------------

/// Generate a PKCE code verifier (43 URL-safe random chars per RFC 7636).
pub fn pkce_verifier() -> String {
    use base64::Engine as _;
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("getrandom failed");
    // base64url-encode → 43 chars (256 bits of entropy, no padding)
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Derive a PKCE code challenge from a verifier (S256 method).
pub fn pkce_challenge(verifier: &str) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
}

// ---------------------------------------------------------------------------
// OAuth port allocation
// ---------------------------------------------------------------------------

/// Bind to an ephemeral localhost port for the OAuth redirect.
/// Returns the allocated port number.
pub fn oauth_port_alloc() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

// ---------------------------------------------------------------------------
// Browser-based login flow
// ---------------------------------------------------------------------------

/// OAuth login state.
#[derive(Debug, Clone)]
pub struct XaaLoginState {
    pub server_name: String,
    pub idp_url: String,
    pub verifier: String,
    pub redirect_port: u16,
}

/// Initiate an XAA (cross-agent authorization) login flow.
///
/// Opens the browser to the IdP authorization URL with PKCE parameters.
/// Returns the login state needed to complete the exchange.
pub fn initiate_xaa_login(server_name: &str, idp_url: &str) -> std::io::Result<XaaLoginState> {
    let port = oauth_port_alloc()?;
    let verifier = pkce_verifier();
    let challenge = pkce_challenge(&verifier);
    let redirect_uri = format!("http://127.0.0.1:{}/callback", port);

    let auth_url = format!(
        "{}?response_type=code&code_challenge={}&code_challenge_method=S256&redirect_uri={}",
        idp_url, challenge, redirect_uri
    );

    // Open the browser (best-effort; ignore errors on headless systems).
    let _ = open_browser(&auth_url);

    Ok(XaaLoginState {
        server_name: server_name.to_string(),
        idp_url: idp_url.to_string(),
        verifier,
        redirect_port: port,
    })
}

/// Open a URL in the system browser.
fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .status()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).status()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).status()?;
    }
    Ok(())
}

/// Exchange an authorization code for an access token.
pub async fn exchange_code(
    token_endpoint: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> anyhow::Result<McpToken> {
    let client = reqwest::Client::new();
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", redirect_uri),
    ];

    let resp = client
        .post(token_endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("exchange_code: request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("exchange_code: HTTP {} — {}", status, body);
    }

    #[derive(serde::Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<u64>,
        scope: Option<String>,
    }

    let tr: TokenResponse = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("exchange_code: bad JSON: {}", e))?;

    let expires_at = tr.expires_in.map(|secs| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + secs
    });

    Ok(McpToken {
        access_token: tr.access_token,
        refresh_token: tr.refresh_token,
        expires_at,
        scope: tr.scope,
        server_name: String::new(), // caller should set this
    })
}

/// Refresh an existing MCP token using the stored refresh token.
pub async fn refresh_mcp_token(
    server_name: &str,
    token_endpoint: &str,
) -> anyhow::Result<McpToken> {
    let existing = get_mcp_token(server_name)
        .ok_or_else(|| anyhow::anyhow!("No stored token for {}", server_name))?;
    let refresh = existing
        .refresh_token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Token for {} has no refresh token", server_name))?
        .to_string();

    let client = reqwest::Client::new();
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh.as_str()),
    ];

    let resp = client
        .post(token_endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("refresh: request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("refresh: HTTP {} — {}", status, body);
    }

    #[derive(serde::Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<u64>,
    }

    let tr: TokenResponse = resp.json().await?;
    let expires_at = tr.expires_in.map(|s| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + s
    });

    let new_token = McpToken {
        access_token: tr.access_token,
        refresh_token: tr.refresh_token.or(existing.refresh_token),
        expires_at,
        scope: existing.scope,
        server_name: server_name.to_string(),
    };

    store_mcp_token(&new_token)?;
    Ok(new_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_length() {
        let v = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";
        let c = pkce_challenge(v);
        assert!(!c.is_empty());
        // Base64url of SHA256 is 43 chars (without padding).
        assert_eq!(c.len(), 43);
    }

    #[test]
    fn token_expiry() {
        let t = McpToken {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at: Some(1), // expired long ago
            scope: None,
            server_name: "test".to_string(),
        };
        assert!(t.is_expired(0));
    }
}
