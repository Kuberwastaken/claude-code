// settings_sync.rs — Settings Sync
//
// Port of src/services/settingsSync/index.ts
//
// Syncs user settings and AGENTS.md memory files between a local Claurst
// installation and claude.ai via:
//   - Upload (interactive CLI, fire-and-forget at startup)
//   - Download (CCR / CLAURST_REMOTE=1, blocking before plugin load)
//
// Authentication requires OAuth (Bearer token).  API-key-only users are
// skipped silently — the TypeScript side gates on `isUsingOAuth()`.
//
// The sync API stores a flat key→value map where keys are canonical file paths
// and values are the UTF-8 file contents (JSON or Markdown).

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SYNC_TIMEOUT_SECS: u64 = 10;
#[allow(dead_code)]
const DEFAULT_MAX_RETRIES: u32 = 3;
/// 500 KB per-file size limit (matches backend enforcement).
const MAX_FILE_SIZE_BYTES: u64 = 500 * 1024;

// ---------------------------------------------------------------------------
// Sync key helpers (mirrors SYNC_KEYS in types.ts)
// ---------------------------------------------------------------------------

/// Canonical sync key for the global user settings file.
pub const SYNC_KEY_USER_SETTINGS: &str = "~/.claurst/settings.json";
/// Canonical sync key for the global user memory file.
pub const SYNC_KEY_USER_MEMORY: &str = "~/.claurst/AGENTS.md";

/// Canonical sync key for per-project settings (keyed by git-remote hash).
pub fn sync_key_project_settings(project_id: &str) -> String {
    format!("projects/{project_id}/.claurst/settings.local.json")
}

/// Canonical sync key for per-project memory (keyed by git-remote hash).
pub fn sync_key_project_memory(project_id: &str) -> String {
    format!("projects/{project_id}/AGENTS.local.md")
}

// ---------------------------------------------------------------------------
// API wire types
// ---------------------------------------------------------------------------

/// Content field in the GET response — flat string key/value map.
#[derive(Debug, Deserialize)]
struct UserSyncContent {
    entries: HashMap<String, String>,
}

/// Full GET /api/claude_code/user_settings response.
#[derive(Debug, Deserialize)]
struct UserSyncData {
    #[allow(dead_code)]
    #[serde(rename = "userId")]
    user_id: Option<String>,
    #[allow(dead_code)]
    version: Option<u64>,
    #[allow(dead_code)]
    #[serde(rename = "lastModified")]
    last_modified: Option<String>,
    #[allow(dead_code)]
    checksum: Option<String>,
    content: UserSyncContent,
}

/// PUT response (partial — only fields we care about).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct UploadResponse {
    checksum: Option<String>,
    #[serde(rename = "lastModified")]
    last_modified: Option<String>,
}

// ---------------------------------------------------------------------------
// Public output types
// ---------------------------------------------------------------------------

/// Data returned by a successful download.
#[derive(Debug, Clone, Default)]
pub struct SyncedData {
    /// Parsed user settings JSON (if the `user_settings` key was present).
    pub settings: Option<Value>,
    /// Raw file contents keyed by their sync keys.
    pub memory_files: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// SettingsSyncManager
// ---------------------------------------------------------------------------

/// Manages uploading and downloading settings/memory files to/from claude.ai.
pub struct SettingsSyncManager {
    /// OAuth bearer token for authentication.
    pub oauth_token: String,
    /// Base API URL (default: https://api.anthropic.com).
    pub base_url: String,
    http: reqwest::Client,
}

impl SettingsSyncManager {
    /// Create a new manager.
    pub fn new(oauth_token: String, base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(SYNC_TIMEOUT_SECS))
            .build()
            .unwrap_or_default();
        Self {
            oauth_token,
            base_url,
            http,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/api/claude_code/user_settings", self.base_url)
    }

    #[allow(dead_code)]
    fn auth_headers(&self) -> [(&'static str, String); 2] {
        [
            ("Authorization", format!("Bearer {}", self.oauth_token)),
            ("anthropic-beta", "oauth-2025-04-20".to_string()),
        ]
    }

    // -----------------------------------------------------------------------
    // Download
    // -----------------------------------------------------------------------

    /// Download remote settings and memory files.
    ///
    /// Returns `Ok(None)` when the server has no data for this user (404).
    /// Fails open — callers should treat errors as "no remote data".
    pub async fn download(&self) -> Result<Option<SyncedData>> {
        let resp = self
            .http
            .get(self.endpoint())
            .header("Authorization", format!("Bearer {}", self.oauth_token))
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await?;

        let status = resp.status().as_u16();
        if status == 404 {
            debug!("Settings sync: no remote data (404)");
            return Ok(None);
        }
        if status != 200 {
            anyhow::bail!("Settings sync download: unexpected status {}", status);
        }

        let data: UserSyncData = resp.json().await?;
        Ok(Some(entries_to_synced_data(data.content.entries)))
    }

    /// Download with exponential-backoff retry.
    #[allow(dead_code)]
    async fn download_with_retry(&self) -> Result<Option<SyncedData>> {
        let mut last_err = anyhow::anyhow!("No attempts made");
        for attempt in 1..=(DEFAULT_MAX_RETRIES + 1) {
            match self.download().await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let msg = e.to_string();
                    // Auth failures are terminal
                    if msg.contains("401") || msg.contains("403") {
                        return Err(e);
                    }
                    warn!(
                        attempt,
                        max = DEFAULT_MAX_RETRIES,
                        error = %e,
                        "Settings sync download failed, will retry"
                    );
                    last_err = e;
                    if attempt <= DEFAULT_MAX_RETRIES {
                        tokio::time::sleep(retry_delay(attempt)).await;
                    }
                }
            }
        }
        Err(last_err)
    }

    /// Apply downloaded entries to local files.
    ///
    /// Writes settings and memory files to the appropriate local paths,
    /// enforcing the 500 KB per-file size limit.
    pub async fn apply_to_local(&self, data: &SyncedData, project_id: Option<&str>) -> ApplyResult {
        let mut result = ApplyResult::default();

        // Global user settings
        if let Some(ref settings_json) = data.settings {
            let path = claude_config_dir().join("settings.json");
            let content = serde_json::to_string_pretty(settings_json).unwrap_or_default();
            match write_file_for_sync(&path, &content).await {
                Ok(()) => {
                    result.settings_written = true;
                    result.applied_count += 1;
                }
                Err(e) => warn!("Settings sync: failed to write user settings: {}", e),
            }
        }

        // Global user memory
        if let Some(memory) = data.memory_files.get(SYNC_KEY_USER_MEMORY) {
            let path = claude_config_dir().join("AGENTS.md");
            match write_file_for_sync(&path, memory).await {
                Ok(()) => {
                    result.memory_written = true;
                    result.applied_count += 1;
                }
                Err(e) => warn!("Settings sync: failed to write user memory: {}", e),
            }
        }

        // Project-specific files
        if let Some(pid) = project_id {
            let proj_settings_key = sync_key_project_settings(pid);
            if let Some(content) = data.memory_files.get(&proj_settings_key) {
                let path = std::env::current_dir()
                    .unwrap_or_default()
                    .join(".claurst")
                    .join("settings.local.json");
                match write_file_for_sync(&path, content).await {
                    Ok(()) => {
                        result.settings_written = true;
                        result.applied_count += 1;
                    }
                    Err(e) => {
                        warn!("Settings sync: failed to write project settings: {}", e)
                    }
                }
            }

            let proj_memory_key = sync_key_project_memory(pid);
            if let Some(content) = data.memory_files.get(&proj_memory_key) {
                let path = std::env::current_dir()
                    .unwrap_or_default()
                    .join("AGENTS.local.md");
                match write_file_for_sync(&path, content).await {
                    Ok(()) => {
                        result.memory_written = true;
                        result.applied_count += 1;
                    }
                    Err(e) => {
                        warn!("Settings sync: failed to write project memory: {}", e)
                    }
                }
            }
        }

        result
    }

    // -----------------------------------------------------------------------
    // Upload
    // -----------------------------------------------------------------------

    /// Upload local settings and memory files to remote.
    ///
    /// Compares with existing remote entries and only uploads changed keys.
    pub async fn upload(&self, local_entries: HashMap<String, String>) -> Result<()> {
        // Fetch current remote state for diff
        let remote_entries = match self.download().await? {
            Some(data) => data.memory_files,
            None => HashMap::new(),
        };

        // Only send keys that have changed
        let changed: HashMap<String, String> = local_entries
            .into_iter()
            .filter(|(k, v)| remote_entries.get(k).map(|rv| rv != v).unwrap_or(true))
            .collect();

        if changed.is_empty() {
            debug!("Settings sync: no changes to upload");
            return Ok(());
        }

        debug!(
            count = changed.len(),
            "Settings sync: uploading changed entries"
        );
        self.put_entries(changed).await
    }

    async fn put_entries(&self, entries: HashMap<String, String>) -> Result<()> {
        let body = serde_json::json!({ "entries": entries });
        let resp = self
            .http
            .put(self.endpoint())
            .header("Authorization", format!("Bearer {}", self.oauth_token))
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            anyhow::bail!("Settings sync upload: unexpected status {}", status);
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Fire-and-forget background upload (called from startup)
    // -----------------------------------------------------------------------

    /// Spawn a fire-and-forget upload task. Errors are logged but not propagated.
    ///
    /// Call this right after auth is established.  The task will:
    ///   1. Read local settings and AGENTS.md files
    ///   2. Fetch current remote state for diffing
    ///   3. Upload only changed entries
    pub fn upload_in_background(token: String, base_url: String) {
        tokio::spawn(async move {
            let mgr = SettingsSyncManager::new(token, base_url);
            let entries = collect_local_entries(None).await;
            if let Err(e) = mgr.upload(entries).await {
                warn!("Settings sync: background upload failed: {}", e);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Apply result
// ---------------------------------------------------------------------------

/// Summary of what `apply_to_local` wrote.
#[derive(Debug, Default)]
pub struct ApplyResult {
    pub applied_count: usize,
    pub settings_written: bool,
    pub memory_written: bool,
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Convert raw sync entries into the `SyncedData` structure.
///
/// The user settings entry is parsed as JSON; memory files are kept as-is.
fn entries_to_synced_data(entries: HashMap<String, String>) -> SyncedData {
    let mut data = SyncedData::default();

    for (key, value) in entries {
        if key == SYNC_KEY_USER_SETTINGS {
            data.settings = serde_json::from_str(&value).ok();
        } else {
            data.memory_files.insert(key, value);
        }
    }

    data
}

/// Collect local files that should be uploaded.
///
/// Reads global user settings and AGENTS.md, plus (if `project_id` is given)
/// project-local settings and AGENTS.local.md.  Files larger than 500 KB or
/// that cannot be read are silently omitted.
pub async fn collect_local_entries(project_id: Option<&str>) -> HashMap<String, String> {
    let mut entries = HashMap::new();

    // Global user settings
    let settings_path = claude_config_dir().join("settings.json");
    if let Some(content) = try_read_for_sync(&settings_path).await {
        entries.insert(SYNC_KEY_USER_SETTINGS.to_string(), content);
    }

    // Global user memory
    let memory_path = claude_config_dir().join("AGENTS.md");
    if let Some(content) = try_read_for_sync(&memory_path).await {
        entries.insert(SYNC_KEY_USER_MEMORY.to_string(), content);
    }

    // Project-specific files
    if let Some(pid) = project_id {
        let cwd = std::env::current_dir().unwrap_or_default();

        let local_settings = cwd.join(".claurst").join("settings.local.json");
        if let Some(content) = try_read_for_sync(&local_settings).await {
            entries.insert(sync_key_project_settings(pid), content);
        }

        let local_memory = cwd.join("AGENTS.local.md");
        if let Some(content) = try_read_for_sync(&local_memory).await {
            entries.insert(sync_key_project_memory(pid), content);
        }
    }

    entries
}

/// Try to read a file, applying the 500 KB size limit.
/// Returns `None` if the file doesn't exist, is empty, or exceeds the limit.
async fn try_read_for_sync(path: &PathBuf) -> Option<String> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    if meta.len() > MAX_FILE_SIZE_BYTES {
        debug!(path = %path.display(), "Settings sync: file exceeds 500 KB limit, skipping");
        return None;
    }
    let content = tokio::fs::read_to_string(path).await.ok()?;
    if content.trim().is_empty() {
        return None;
    }
    Some(content)
}

/// Write `content` to `path`, creating parent directories as needed.
async fn write_file_for_sync(path: &PathBuf, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, content).await?;
    Ok(())
}

/// Return the ~/.claurst directory.
fn claude_config_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".claurst"))
        .unwrap_or_else(|| PathBuf::from(".claurst"))
}

/// Exponential backoff delay for retry attempt `n` (1-indexed), capped at 30 s.
#[allow(dead_code)]
fn retry_delay(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(30);
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
    fn test_sync_keys() {
        assert_eq!(SYNC_KEY_USER_SETTINGS, "~/.claurst/settings.json");
        assert_eq!(SYNC_KEY_USER_MEMORY, "~/.claurst/AGENTS.md");
        assert_eq!(
            sync_key_project_settings("abc123"),
            "projects/abc123/.claurst/settings.local.json"
        );
        assert_eq!(
            sync_key_project_memory("abc123"),
            "projects/abc123/AGENTS.local.md"
        );
    }

    #[test]
    fn test_entries_to_synced_data_settings_parsed() {
        let mut entries = HashMap::new();
        entries.insert(
            SYNC_KEY_USER_SETTINGS.to_string(),
            r#"{"model":"claude-3"}"#.to_string(),
        );
        entries.insert(SYNC_KEY_USER_MEMORY.to_string(), "# My notes".to_string());

        let data = entries_to_synced_data(entries);
        assert!(data.settings.is_some());
        assert_eq!(data.settings.unwrap()["model"], json!("claude-3"));
        assert_eq!(
            data.memory_files.get(SYNC_KEY_USER_MEMORY).unwrap(),
            "# My notes"
        );
    }

    #[test]
    fn test_entries_to_synced_data_invalid_json_settings() {
        let mut entries = HashMap::new();
        entries.insert(SYNC_KEY_USER_SETTINGS.to_string(), "not-json".to_string());
        let data = entries_to_synced_data(entries);
        // Malformed settings JSON → field is None (graceful degradation)
        assert!(data.settings.is_none());
    }

    #[test]
    fn test_entries_to_synced_data_empty() {
        let data = entries_to_synced_data(HashMap::new());
        assert!(data.settings.is_none());
        assert!(data.memory_files.is_empty());
    }

    #[test]
    fn test_retry_delay_progression() {
        assert_eq!(retry_delay(1), Duration::from_secs(1));
        assert_eq!(retry_delay(2), Duration::from_secs(2));
        assert_eq!(retry_delay(3), Duration::from_secs(4));
        assert_eq!(retry_delay(4), Duration::from_secs(8));
        assert_eq!(retry_delay(5), Duration::from_secs(16));
        // Capped at 30 s
        assert_eq!(retry_delay(6), Duration::from_secs(30));
        assert_eq!(retry_delay(10), Duration::from_secs(30));
    }
}
