//! MCP OAuth / XAA IdP login flow.
//! Mirrors src/services/mcp/xaaIdpLogin.ts and src/services/mcp/auth.ts.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

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
        let Some(exp) = self.expires_at else { return false };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now + grace_secs >= exp
    }

    pub fn expiry_datetime(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        token_expiry_datetime(self.expires_at)
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

pub fn token_expiry_datetime(
    expires_at: Option<u64>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    expires_at.map(|timestamp| {
        chrono::DateTime::<chrono::Utc>::from(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(timestamp),
        )
    })
}

// ---------------------------------------------------------------------------
// OAuth metadata / auth flow helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
}

#[derive(Debug, Clone)]
pub struct McpAuthSession {
    pub server_name: String,
    pub auth_url: String,
    pub redirect_uri: String,
    pub verifier: String,
    pub metadata: McpOAuthMetadata,
}

#[derive(Debug, Clone)]
pub struct McpAuthResult {
    pub server_name: String,
    pub auth_url: String,
    pub redirect_uri: String,
    pub token_path: PathBuf,
}

fn normalized_server_url(server_url: &str) -> &str {
    server_url.trim_end_matches('/')
}

fn fallback_oauth_metadata(server_url: &str) -> McpOAuthMetadata {
    let base_url = normalized_server_url(server_url);
    McpOAuthMetadata {
        authorization_endpoint: format!("{}/oauth/authorize", base_url),
        token_endpoint: format!("{}/oauth/token", base_url),
    }
}

fn build_mcp_auth_url(
    authorization_endpoint: &str,
    redirect_uri: &str,
    verifier: &str,
) -> String {
    let challenge = pkce_challenge(verifier);
    format!(
        "{}?client_id=claurst&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256",
        authorization_endpoint,
        urlencoding::encode(redirect_uri),
        challenge,
    )
}

pub async fn fetch_oauth_metadata(server_url: &str) -> anyhow::Result<McpOAuthMetadata> {
    let base_url = normalized_server_url(server_url);
    let fallback = fallback_oauth_metadata(base_url);
    let metadata_url = format!("{}/.well-known/oauth-authorization-server", base_url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {}", e))?;

    match client.get(&metadata_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let meta: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| anyhow::anyhow!("OAuth metadata parse error: {}", e))?;
            Ok(McpOAuthMetadata {
                authorization_endpoint: meta
                    .get("authorization_endpoint")
                    .and_then(|value| value.as_str())
                    .unwrap_or(fallback.authorization_endpoint.as_str())
                    .to_string(),
                token_endpoint: meta
                    .get("token_endpoint")
                    .and_then(|value| value.as_str())
                    .unwrap_or(fallback.token_endpoint.as_str())
                    .to_string(),
            })
        }
        Ok(_) | Err(_) => Ok(fallback),
    }
}

pub async fn begin_mcp_auth(
    server_name: &str,
    server_url: &str,
) -> anyhow::Result<McpAuthSession> {
    let metadata = fetch_oauth_metadata(server_url).await?;
    let redirect_port = oauth_port_alloc()
        .map_err(|e| anyhow::anyhow!("Failed to allocate OAuth redirect port: {}", e))?;
    let redirect_uri = format!("http://127.0.0.1:{}/callback", redirect_port);
    let verifier = pkce_verifier();
    let auth_url = build_mcp_auth_url(
        &metadata.authorization_endpoint,
        &redirect_uri,
        &verifier,
    );

    Ok(McpAuthSession {
        server_name: server_name.to_string(),
        auth_url,
        redirect_uri,
        verifier,
        metadata,
    })
}

async fn bind_callback_listener(
    redirect_uri: &str,
) -> anyhow::Result<(TcpListener, String, String)> {
    let redirect_url = url::Url::parse(redirect_uri)
        .map_err(|e| anyhow::anyhow!("Failed to parse redirect URI '{}': {}", redirect_uri, e))?;
    let host = redirect_url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("Redirect URI '{}' is missing host", redirect_uri))?
        .to_string();
    let port = redirect_url
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("Redirect URI '{}' is missing port", redirect_uri))?;
    let callback_path = if redirect_url.path().is_empty() {
        "/callback".to_string()
    } else {
        redirect_url.path().to_string()
    };
    let listener = TcpListener::bind((host.as_str(), port))
        .await
        .map_err(|e| anyhow::anyhow!("Failed to bind OAuth callback listener on {}:{}: {}", host, port, e))?;

    Ok((listener, host, callback_path))
}

async fn wait_for_authorization_code(
    listener: TcpListener,
    host: &str,
    callback_path: &str,
    expected_state: Option<&str>,
) -> anyhow::Result<String> {
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(180), listener.accept())
        .await
        .map_err(|_| anyhow::anyhow!("Timeout waiting for OAuth callback"))?
        .map_err(|e| anyhow::anyhow!("Failed to accept OAuth callback connection: {}", e))?;

    let (reader, mut writer) = socket.split();
    let mut reader = BufReader::new(reader);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read OAuth callback request: {}", e))?;
    loop {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read OAuth callback headers: {}", e))?;
        if header.trim().is_empty() {
            break;
        }
    }

    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    let parsed_url = url::Url::parse(&format!("http://{}{}", host, path))
        .map_err(|e| anyhow::anyhow!("Failed to parse OAuth callback URL '{}': {}", path, e))?;

    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\nMCP OAuth authentication finished. You can close this tab.\r\n";
    writer
        .write_all(response.as_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write OAuth callback response: {}", e))?;

    if parsed_url.path() != callback_path {
        anyhow::bail!(
            "OAuth callback path mismatch: expected '{}', got '{}'",
            callback_path,
            parsed_url.path()
        );
    }

    if let Some(expected_state) = expected_state {
        let received_state = parsed_url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.to_string());
        if received_state.as_deref() != Some(expected_state) {
            anyhow::bail!("OAuth state mismatch — possible CSRF attack");
        }
    }

    parsed_url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.to_string())
        .ok_or_else(|| anyhow::anyhow!("OAuth callback did not contain an authorization code"))
}

pub async fn run_mcp_auth_session(session: McpAuthSession) -> anyhow::Result<McpAuthResult> {
    let (listener, host, callback_path) = bind_callback_listener(&session.redirect_uri).await?;
    open::that(&session.auth_url)
        .map_err(|e| anyhow::anyhow!("Failed to open browser for OAuth: {}", e))?;

    let code = wait_for_authorization_code(listener, &host, &callback_path, None).await?;
    let mut token = exchange_code(
        &session.metadata.token_endpoint,
        &code,
        &session.verifier,
        &session.redirect_uri,
    )
    .await?;
    token.server_name = session.server_name.clone();
    store_mcp_token(&token).map_err(|e| {
        anyhow::anyhow!(
            "Failed to store MCP token for '{}': {}",
            session.server_name,
            e
        )
    })?;

    Ok(McpAuthResult {
        server_name: session.server_name,
        auth_url: session.auth_url,
        redirect_uri: session.redirect_uri,
        token_path: token_path(&token.server_name),
    })
}

pub async fn run_mcp_auth_flow(
    server_name: &str,
    server_url: &str,
) -> anyhow::Result<McpAuthResult> {
    let session = begin_mcp_auth(server_name, server_url).await?;
    run_mcp_auth_session(session).await
}

pub async fn get_valid_mcp_token(
    server_name: &str,
    server_url: &str,
) -> anyhow::Result<Option<McpToken>> {
    let Some(token) = get_mcp_token(server_name) else {
        return Ok(None);
    };

    if !token.is_expired(60) {
        return Ok(Some(token));
    }

    if token.refresh_token.is_none() {
        return Ok(None);
    }

    let metadata = fetch_oauth_metadata(server_url).await?;
    refresh_mcp_token(server_name, &metadata.token_endpoint)
        .await
        .map(Some)
}

pub async fn get_valid_mcp_access_token(
    server_name: &str,
    server_url: &str,
) -> anyhow::Result<Option<String>> {
    Ok(get_valid_mcp_token(server_name, server_url)
        .await?
        .map(|token| token.access_token))
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
pub fn initiate_xaa_login(
    server_name: &str,
    idp_url: &str,
) -> std::io::Result<XaaLoginState> {
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

    let tr: TokenResponse = resp.json().await.map_err(|e| anyhow::anyhow!("exchange_code: bad JSON: {}", e))?;

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
pub async fn refresh_mcp_token(server_name: &str, token_endpoint: &str) -> anyhow::Result<McpToken> {
    let existing = get_mcp_token(server_name)
        .ok_or_else(|| anyhow::anyhow!("No stored token for {}", server_name))?;
    let refresh = existing
        .refresh_token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Token for {} has no refresh token", server_name))?
        .to_string();

    let client = reqwest::Client::new();
    let params = [("grant_type", "refresh_token"), ("refresh_token", refresh.as_str())];

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

    #[test]
    fn fallback_metadata_uses_default_oauth_paths() {
        let metadata = fallback_oauth_metadata("https://example.com/mcp/");
        assert_eq!(
            metadata.authorization_endpoint,
            "https://example.com/mcp/oauth/authorize"
        );
        assert_eq!(metadata.token_endpoint, "https://example.com/mcp/oauth/token");
    }

    #[test]
    fn build_auth_url_contains_required_params() {
        let verifier = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";
        let redirect_uri = "http://127.0.0.1:9999/callback";
        let url = build_mcp_auth_url(
            "https://example.com/oauth/authorize",
            redirect_uri,
            verifier,
        );
        assert!(url.contains("client_id=claurst"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A9999%2Fcallback"));
    }

    #[tokio::test]
    async fn bind_callback_listener_reuses_redirect_port() {
        let (listener, host, callback_path) =
            bind_callback_listener("http://127.0.0.1:14555/callback")
                .await
                .expect("listener should bind");
        let port = listener.local_addr().expect("listener addr").port();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(callback_path, "/callback");
        assert_eq!(port, 14555);
        drop(listener);
    }
}
