// remote_settings.rs — Remote Managed Settings
//
// Port of src/services/remoteManagedSettings/index.ts
//
// Fetches enterprise-managed settings from Anthropic's API, caches them to
// ~/.claurst/remote-settings.json, and polls every hour in the background.
// Fails open — if the fetch fails, the app continues without remote settings.
//
// Eligibility:
//   - API key users: always eligible
//   - OAuth users: only Enterprise/Team (server decides; we just try and accept
//     empty/204 as "no settings configured")

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SETTINGS_FILENAME: &str = "remote-settings.json";
const SETTINGS_TIMEOUT_SECS: u64 = 10;
const DEFAULT_MAX_RETRIES: u32 = 5;
/// 1-hour polling interval (matches TypeScript POLLING_INTERVAL_MS)
pub const DEFAULT_POLLING_INTERVAL: Duration = Duration::from_secs(60 * 60);

// ---------------------------------------------------------------------------
// Free-code stub: no remote settings fetching
// ---------------------------------------------------------------------------

/// Stub: Returns empty managed settings.
/// The free/OSS build does not fetch server-pushed security overlays or
/// enterprise-managed settings from Anthropic's API.
pub async fn fetch_remote_managed_settings() -> Value {
    serde_json::json!({})
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration for the remote settings manager.
#[derive(Debug, Clone)]
pub struct RemoteSettingsConfig {
    /// Anthropic API key (x-api-key header). Takes precedence over OAuth.
    pub api_key: Option<String>,
    /// OAuth bearer token (Authorization: Bearer …).
    pub oauth_token: Option<String>,
    /// Base URL for the Anthropic API (default: https://api.anthropic.com).
    pub base_url: String,
    /// How often to poll for new settings in the background.
    pub polling_interval: Duration,
}

impl Default for RemoteSettingsConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            oauth_token: None,
            base_url: "https://api.anthropic.com".to_string(),
            polling_interval: DEFAULT_POLLING_INTERVAL,
        }
    }
}

/// On-disk cache structure stored in ~/.claurst/remote-settings.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteSettingsCache {
    /// The cached settings object (may be empty `{}`).
    pub settings: Option<Value>,
    /// SHA-256 checksum of the settings used for HTTP ETag caching.
    pub checksum: Option<String>,
    /// When the cache was last successfully fetched from the API.
    pub fetched_at: Option<DateTime<Utc>>,
}

/// Wire format returned by the remote settings API.
#[derive(Debug, Deserialize)]
struct RemoteSettingsResponse {
    /// Settings UUID (informational only).
    #[allow(dead_code)]
    uuid: Option<String>,
    /// Server-computed checksum.
    checksum: Option<String>,
    /// The settings payload.
    settings: Value,
}

// ---------------------------------------------------------------------------
// RemoteSettingsManager
// ---------------------------------------------------------------------------

/// Manages fetching, caching, and background polling of remote-managed settings.
pub struct RemoteSettingsManager {
    config: RemoteSettingsConfig,
    cache_path: PathBuf,
    http: reqwest::Client,
}

impl RemoteSettingsManager {
    /// Create a new manager with the given config.
    /// The cache file lives at `<claude_config_dir>/remote-settings.json`.
    pub fn new(config: RemoteSettingsConfig) -> Self {
        let cache_path = claude_config_dir().join(SETTINGS_FILENAME);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(SETTINGS_TIMEOUT_SECS))
            .build()
            .unwrap_or_default();
        Self {
            config,
            cache_path,
            http,
        }
    }

    /// Check whether the caller is eligible for remote managed settings.
    ///
    /// Any user with an API key is eligible. OAuth-only users may or may not
    /// have enterprise access — the server will return 204/empty for those
    /// without managed settings, so we treat all authenticated users as
    /// "eligible" and let the server determine actual access.
    pub fn is_eligible(api_key: Option<&str>) -> bool {
        api_key.map(|k| !k.is_empty()).unwrap_or(false)
    }

    /// Endpoint URL for remote settings.
    fn endpoint(&self) -> String {
        format!("{}/api/claude_code/settings", self.config.base_url)
    }

    /// Build auth headers. API key takes precedence over OAuth.
    fn auth_headers(&self) -> Option<std::collections::HashMap<String, String>> {
        let mut headers = std::collections::HashMap::new();
        if let Some(ref key) = self.config.api_key {
            if !key.is_empty() {
                headers.insert("x-api-key".to_string(), key.clone());
                return Some(headers);
            }
        }
        if let Some(ref token) = self.config.oauth_token {
            if !token.is_empty() {
                headers.insert("Authorization".to_string(), format!("Bearer {}", token));
                headers.insert("anthropic-beta".to_string(), "oauth-2025-04-20".to_string());
                return Some(headers);
            }
        }
        None
    }

    /// Load settings cached to disk (without hitting the network).
    /// Returns `None` if the cache file doesn't exist or is malformed.
    pub async fn load_cached(&self) -> Option<Value> {
        match tokio::fs::read_to_string(&self.cache_path).await {
            Ok(text) => serde_json::from_str::<Value>(&text).ok(),
            Err(_) => None,
        }
    }

    /// Persist the cache entry to disk (mode 0o600 equivalent via truncate).
    async fn save_cache(&self, cache: &RemoteSettingsCache) -> Result<()> {
        let text = serde_json::to_string_pretty(cache)?;
        // Create parent dir if needed
        if let Some(parent) = self.cache_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&self.cache_path, text).await?;
        Ok(())
    }

    /// Delete the on-disk cache file.
    pub async fn clear_cache(&self) {
        let _ = tokio::fs::remove_file(&self.cache_path).await;
    }

    /// Perform a single fetch attempt (no retries).
    ///
    /// Returns:
    /// - `Ok(Some(settings))` — new settings fetched
    /// - `Ok(None)` — 304 Not Modified; caller should keep cached value
    /// - `Err(...)` — transient or permanent failure
    async fn fetch_once(&self, cached_checksum: Option<&str>) -> Result<Option<Value>> {
        let auth = self
            .auth_headers()
            .ok_or_else(|| anyhow::anyhow!("No authentication available for remote settings"))?;

        let mut req = self.http.get(self.endpoint());
        for (k, v) in &auth {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(cs) = cached_checksum {
            req = req.header("If-None-Match", format!("\"{}\"", cs));
        }

        let resp = req.send().await?;
        let status = resp.status().as_u16();

        match status {
            304 => {
                debug!("Remote settings: 304 Not Modified — cache still valid");
                return Ok(None);
            }
            204 | 404 => {
                debug!("Remote settings: {} — no settings configured", status);
                return Ok(Some(Value::Object(Default::default())));
            }
            200 => {}
            401 | 403 => {
                // Auth errors are terminal — no point retrying
                anyhow::bail!("Remote settings: not authorized ({})", status);
            }
            other => {
                anyhow::bail!("Remote settings: unexpected status {}", other);
            }
        }

        let body: Value = resp.json().await?;

        // Try to parse as the expected response shape, but be permissive —
        // accept raw settings object if the wrapper is missing.
        let (settings, _checksum) =
            if let Ok(parsed) = serde_json::from_value::<RemoteSettingsResponse>(body.clone()) {
                (parsed.settings, parsed.checksum)
            } else if body.is_object() {
                (body, None)
            } else {
                anyhow::bail!("Remote settings: unexpected response shape");
            };

        Ok(Some(settings))
    }

    /// Fetch settings with exponential-backoff retry.
    ///
    /// On success returns the new settings value (or `None` for 304).
    /// On failure after all retries, returns the last error.
    pub async fn fetch_with_retry(&self, cached_checksum: Option<&str>) -> Result<Option<Value>> {
        let mut last_err = anyhow::anyhow!("No attempts made");
        for attempt in 1..=(DEFAULT_MAX_RETRIES + 1) {
            match self.fetch_once(cached_checksum).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    // Auth errors: don't retry
                    let msg = e.to_string();
                    if msg.contains("not authorized") || msg.contains("No authentication") {
                        return Err(e);
                    }
                    warn!(
                        attempt,
                        max = DEFAULT_MAX_RETRIES,
                        error = %e,
                        "Remote settings fetch failed, will retry"
                    );
                    last_err = e;
                    if attempt <= DEFAULT_MAX_RETRIES {
                        let delay = retry_delay(attempt);
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        Err(last_err)
    }

    /// Full fetch-and-cache cycle: read disk cache → HTTP fetch → save to disk.
    ///
    /// Fails open: returns the stale cached value (or `None`) on any error.
    pub async fn fetch_once_and_cache(&self) -> Option<Value> {
        // Load any previously cached settings to compute an ETag checksum.
        let cached_raw = match tokio::fs::read_to_string(&self.cache_path).await {
            Ok(text) => Some(text),
            Err(_) => None,
        };
        let cached_settings: Option<Value> = cached_raw
            .as_deref()
            .and_then(|t| serde_json::from_str(t).ok());

        let cached_checksum = cached_settings
            .as_ref()
            .map(|s| compute_checksum_from_settings(s));

        match self.fetch_with_retry(cached_checksum.as_deref()).await {
            Ok(Some(new_settings)) => {
                // Got fresh settings — persist and return.
                let checksum = compute_checksum_from_settings(&new_settings);
                let cache = RemoteSettingsCache {
                    settings: Some(new_settings.clone()),
                    checksum: Some(checksum),
                    fetched_at: Some(Utc::now()),
                };
                if let Err(e) = self.save_cache(&cache).await {
                    warn!("Remote settings: failed to save cache: {}", e);
                }
                Some(new_settings)
            }
            Ok(None) => {
                // 304 — cached settings are still valid
                debug!("Remote settings: cache still valid (304)");
                cached_settings
            }
            Err(e) => {
                warn!("Remote settings: fetch failed ({}), using stale cache", e);
                cached_settings
            }
        }
    }

    /// Spawn a long-running background polling task.
    ///
    /// Polls every `config.polling_interval`, gracefully degrading on failures.
    /// Stops when `cancel` is triggered.
    pub async fn start_polling(self: Arc<Self>, cancel: CancellationToken) {
        let mut interval = tokio::time::interval(self.config.polling_interval);
        // The first tick fires immediately; skip it so we don't double-fetch at startup.
        interval.tick().await;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    debug!("Remote settings: background polling stopped");
                    break;
                }
                _ = interval.tick() => {
                    debug!("Remote settings: background poll tick");
                    self.fetch_once_and_cache().await;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Checksum helpers
// ---------------------------------------------------------------------------

/// Compute a SHA-256 checksum over a settings JSON value.
///
/// Keys are sorted recursively to produce a canonical representation,
/// matching the Python server-side implementation:
/// `json.dumps(settings, sort_keys=True, separators=(",", ":"))`.
pub fn compute_checksum_from_settings(settings: &Value) -> String {
    let sorted = sort_keys_deep(settings);
    let canonical = serde_json::to_string(&sorted).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    format!("sha256:{}", hex::encode(digest))
}

/// Recursively sort all object keys (mirrors `sortKeysDeep` in TypeScript).
fn sort_keys_deep(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: serde_json::Map<String, Value> = serde_json::Map::new();
            let mut keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            for key in keys {
                sorted.insert(key.to_string(), sort_keys_deep(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_keys_deep).collect()),
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// Settings merge
// ---------------------------------------------------------------------------

/// Merge remote settings on top of local settings.
///
/// Remote settings take precedence for any key they define (enterprise policy
/// override). Local settings fill in everything else.
pub fn merge_remote_into_local(local: &Value, remote: &Value) -> Value {
    match (local, remote) {
        (Value::Object(local_map), Value::Object(remote_map)) => {
            let mut merged = local_map.clone();
            for (k, v) in remote_map {
                merged.insert(k.clone(), v.clone());
            }
            Value::Object(merged)
        }
        // If either side is not an object, remote wins.
        (_, remote) => remote.clone(),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the ~/.claurst directory, falling back to the current directory.
fn claude_config_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".claurst"))
        .unwrap_or_else(|| PathBuf::from(".claurst"))
}

/// Exponential backoff delay for retry attempt `n` (1-indexed).
/// Matches the TypeScript `getRetryDelay` pattern: 1s, 2s, 4s, 8s, 16s …
fn retry_delay(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(30); // prevent overflow
    let secs: u64 = 1u64.checked_shl(shift).unwrap_or(u64::MAX).min(30);
    Duration::from_secs(secs)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_sort_keys_deep_flat() {
        let input = json!({"z": 1, "a": 2, "m": 3});
        let sorted = sort_keys_deep(&input);
        let s = serde_json::to_string(&sorted).unwrap();
        assert_eq!(s, r#"{"a":2,"m":3,"z":1}"#);
    }

    #[test]
    fn test_sort_keys_deep_nested() {
        let input = json!({"b": {"z": 1, "a": 2}, "a": [{"z": 0, "a": 1}]});
        let sorted = sort_keys_deep(&input);
        let s = serde_json::to_string(&sorted).unwrap();
        assert_eq!(s, r#"{"a":[{"a":1,"z":0}],"b":{"a":2,"z":1}}"#);
    }

    #[test]
    fn test_checksum_is_stable() {
        let s1 = json!({"apiKeyHelper": "test", "autoUpdaterStatus": "enabled"});
        let s2 = json!({"autoUpdaterStatus": "enabled", "apiKeyHelper": "test"});
        // Order-independent — both should produce the same checksum.
        assert_eq!(
            compute_checksum_from_settings(&s1),
            compute_checksum_from_settings(&s2)
        );
    }

    #[test]
    fn test_checksum_format() {
        let settings = json!({});
        let checksum = compute_checksum_from_settings(&settings);
        assert!(checksum.starts_with("sha256:"));
        assert_eq!(checksum.len(), 7 + 64); // "sha256:" + 64 hex chars
    }

    #[test]
    fn test_merge_remote_wins() {
        let local = json!({"model": "claude-3", "theme": "dark"});
        let remote = json!({"model": "claude-opus-4", "disallowedTools": ["bash"]});
        let merged = merge_remote_into_local(&local, &remote);
        assert_eq!(merged["model"], "claude-opus-4"); // remote wins
        assert_eq!(merged["theme"], "dark"); // local preserved
        assert!(merged["disallowedTools"].is_array()); // remote-only key added
    }

    #[test]
    fn test_merge_empty_remote() {
        let local = json!({"model": "claude-3"});
        let remote = json!({});
        let merged = merge_remote_into_local(&local, &remote);
        assert_eq!(merged["model"], "claude-3");
    }

    #[test]
    fn test_is_eligible_with_key() {
        assert!(RemoteSettingsManager::is_eligible(Some("sk-ant-test")));
        assert!(!RemoteSettingsManager::is_eligible(Some("")));
        assert!(!RemoteSettingsManager::is_eligible(None));
    }

    #[test]
    fn test_retry_delay() {
        assert_eq!(retry_delay(1), Duration::from_secs(1));
        assert_eq!(retry_delay(2), Duration::from_secs(2));
        assert_eq!(retry_delay(3), Duration::from_secs(4));
        assert_eq!(retry_delay(6), Duration::from_secs(30)); // capped at 30s
    }
}
