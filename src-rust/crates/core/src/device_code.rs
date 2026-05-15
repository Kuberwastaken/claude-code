// device_code.rs — GitHub Device Code Flow (RFC 8628).
//
// Provides helpers for initiating a device authorization request and polling
// for the resulting access token.  Used primarily for GitHub Copilot auth.

use serde::Deserialize;

const DEVICE_FLOW_USER_AGENT: &str = concat!("claurst/", env!("CARGO_PKG_VERSION"));

/// Response from the device authorization endpoint.
#[derive(Debug, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
}

/// Initiate a device code flow.
///
/// Returns the device code response containing the user code and verification
/// URI that should be shown to the user.
pub async fn request_device_code(
    client_id: &str,
    scope: &str,
    device_code_url: &str,
) -> Result<DeviceCodeResponse, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(device_code_url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", DEVICE_FLOW_USER_AGENT)
        .json(&serde_json::json!({
            "client_id": client_id,
            "scope": scope,
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    resp.json::<DeviceCodeResponse>()
        .await
        .map_err(|e| e.to_string())
}

/// Poll the token endpoint until the user authorizes or we time out.
///
/// Returns the access token on success.
pub async fn poll_for_token(
    client_id: &str,
    device_code: &str,
    token_url: &str,
    interval: u64,
    timeout_secs: u64,
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let start = std::time::Instant::now();

    loop {
        if start.elapsed().as_secs() > timeout_secs {
            return Err("Timed out waiting for authorization".into());
        }

        tokio::time::sleep(std::time::Duration::from_secs(interval + 1)).await;

        let resp = client
            .post(token_url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("User-Agent", DEVICE_FLOW_USER_AGENT)
            .json(&serde_json::json!({
                "client_id": client_id,
                "device_code": device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            }))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

        if let Some(token) = json.get("access_token").and_then(|v| v.as_str()) {
            return Ok(token.to_string());
        }

        if let Some(error) = json.get("error").and_then(|v| v.as_str()) {
            match error {
                "authorization_pending" => continue,
                "slow_down" => {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
                _ => return Err(format!("Auth error: {}", error)),
            }
        }
    }
}
