//! Team memory synchronization with claude.ai API.
//!
//! Implements delta push (only changed files) with ETag-based optimistic
//! concurrency and greedy bin-packing of changed entries into batches that
//! fit within the server's PUT body limit.
//!
//! Pull is server-wins: remote content overwrites local files unconditionally.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum bytes per local file accepted for sync (250 KB)
const MAX_FILE_SIZE_BYTES: usize = 250 * 1024;

/// Maximum serialized bytes per PUT request body (200 KB)
const MAX_PUT_BODY_BYTES: usize = 200 * 1024;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Persisted per-repo sync state (stored alongside local team-memory files).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncState {
    /// ETag returned by the last successful GET or PUT.
    pub last_known_etag: Option<String>,
    /// Per-key server-side checksums (`"sha256:<hex>"`).
    /// Used to diff local vs remote without re-uploading unchanged entries.
    pub server_checksums: HashMap<String, String>,
    /// Server-enforced max_entries from a prior 413 response.
    pub server_max_entries: Option<usize>,
}

/// A single team-memory entry (one markdown file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMemoryEntry {
    /// Relative file path (forward-slash separated, e.g. `"MEMORY.md"`).
    pub key: String,
    /// UTF-8 file content (typically Markdown).
    pub content: String,
    /// `"sha256:<hex>"` of the content.
    pub checksum: String,
}

/// Server response shape for GET `/api/claude_code/team_memory`.
#[derive(Debug, Serialize, Deserialize)]
pub struct TeamMemoryData {
    pub entries: Vec<TeamMemoryEntry>,
    pub etag: Option<String>,
}

// ---------------------------------------------------------------------------
// Checksum helper
// ---------------------------------------------------------------------------

/// Compute `"sha256:<lowercase hex>"` of a string.
pub fn content_checksum(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

// ---------------------------------------------------------------------------
// Path security validation
// ---------------------------------------------------------------------------

/// Reject paths that could escape the team-memory directory.
///
/// Checks performed (mirroring the TypeScript `securePath` validation):
/// - No null bytes
/// - No URL-encoded traversal sequences (`%2e`, `%2f`, case-insensitive)
/// - No backslashes
/// - Not an absolute path (Unix `/` or Windows `C:` style)
/// - No `..` components
pub fn validate_memory_path(path: &str) -> Result<()> {
    if path.contains('\0') {
        anyhow::bail!("Path contains null bytes: {:?}", path);
    }
    let lower = path.to_ascii_lowercase();
    if lower.contains("%2e") || lower.contains("%2f") {
        anyhow::bail!("Path contains URL-encoded traversal sequences: {:?}", path);
    }
    if path.contains('\\') {
        anyhow::bail!("Path contains backslashes: {:?}", path);
    }
    if path.starts_with('/') {
        anyhow::bail!("Absolute Unix paths not allowed: {:?}", path);
    }
    // Windows-style absolute path: e.g. "C:" or "c:"
    if path.len() >= 2 {
        let mut chars = path.chars();
        let first = chars.next().unwrap();
        if first.is_ascii_alphabetic() && chars.next() == Some(':') {
            anyhow::bail!("Absolute Windows paths not allowed: {:?}", path);
        }
    }
    if path.split('/').any(|component| component == "..") {
        anyhow::bail!("Path traversal not allowed: {:?}", path);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// TeamMemorySync
// ---------------------------------------------------------------------------

/// Drives pull and push against the claude.ai team-memory API.
pub struct TeamMemorySync {
    /// Base URL of the API, e.g. `"https://claude.ai"`.
    api_base: String,
    /// Repo identifier sent as a query parameter.
    repo: String,
    /// Bearer token for authentication.
    token: String,
    /// Local directory that mirrors the server's key namespace.
    team_dir: PathBuf,
}

impl TeamMemorySync {
    pub fn new(api_base: String, repo: String, token: String, team_dir: PathBuf) -> Self {
        Self {
            api_base,
            repo,
            token,
            team_dir,
        }
    }

    // -----------------------------------------------------------------------
    // Pull
    // -----------------------------------------------------------------------

    /// Pull all entries from the server.  Server wins: overwrites local files.
    ///
    /// Updates `state.last_known_etag` and `state.server_checksums` on success.
    /// Returns `Ok(())` on HTTP 404 (no remote data yet).
    pub async fn pull(&self, state: &mut SyncState) -> Result<()> {
        let client = reqwest::Client::new();
        let url = format!(
            "{}/api/claude_code/team_memory?repo={}",
            self.api_base,
            urlencoding::encode(&self.repo),
        );

        let response = client
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("team memory pull: HTTP request failed")?;

        let http_status = response.status();

        if http_status.as_u16() == 404 {
            return Ok(()); // No remote data yet
        }

        if !http_status.is_success() {
            anyhow::bail!("team memory pull failed with status {}", http_status);
        }

        // Capture ETag before consuming the response body
        if let Some(etag) = response.headers().get("etag").and_then(|v| v.to_str().ok()) {
            state.last_known_etag = Some(etag.to_string());
        }

        let data: TeamMemoryData = response
            .json()
            .await
            .context("team memory pull: failed to parse response JSON")?;

        state.server_checksums.clear();

        for entry in &data.entries {
            validate_memory_path(&entry.key)
                .with_context(|| format!("server returned unsafe path: {:?}", entry.key))?;

            state
                .server_checksums
                .insert(entry.key.clone(), entry.checksum.clone());

            let local_path = self.team_dir.join(&entry.key);
            if let Some(parent) = local_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("create_dir_all for {:?}", parent))?;
            }

            if entry.content.len() <= MAX_FILE_SIZE_BYTES {
                tokio::fs::write(&local_path, &entry.content)
                    .await
                    .with_context(|| format!("writing {:?}", local_path))?;
            }
            // Files exceeding MAX_FILE_SIZE_BYTES are silently skipped (same behaviour as push)
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Push
    // -----------------------------------------------------------------------

    /// Push local changes to the server using delta upload.
    ///
    /// Only entries whose local checksum differs from `state.server_checksums`
    /// are uploaded.  Changed entries are packed into batches ≤ `MAX_PUT_BODY_BYTES`.
    pub async fn push(&self, state: &mut SyncState) -> Result<()> {
        let local_entries = self
            .scan_local_files()
            .await
            .context("team memory push: scanning local files")?;

        // Delta: entries where local hash ≠ last-known server hash
        let changed: Vec<TeamMemoryEntry> = local_entries
            .into_iter()
            .filter(|entry| {
                state.server_checksums.get(&entry.key).map(|s| s.as_str()) != Some(&entry.checksum)
            })
            .collect();

        if changed.is_empty() {
            return Ok(());
        }

        let batches = self.pack_batches(changed);
        for batch in batches {
            self.upload_batch(batch, state)
                .await
                .context("team memory push: uploading batch")?;
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    /// Greedy bin-packing: pack entries into batches that each serialise to
    /// ≤ `MAX_PUT_BODY_BYTES`.  Entries that individually exceed the limit go
    /// into singleton batches (server will reject them with 413, but that is
    /// the caller's problem).
    fn pack_batches(&self, entries: Vec<TeamMemoryEntry>) -> Vec<Vec<TeamMemoryEntry>> {
        let mut batches: Vec<Vec<TeamMemoryEntry>> = Vec::new();
        let mut current: Vec<TeamMemoryEntry> = Vec::new();
        let mut current_size: usize = 0;

        for entry in entries {
            // Rough size estimate: key + content + JSON envelope overhead
            let entry_size = entry.key.len() + entry.content.len() + 100;

            if entry_size > MAX_PUT_BODY_BYTES {
                // Oversized entry goes solo
                if !current.is_empty() {
                    batches.push(std::mem::take(&mut current));
                    current_size = 0;
                }
                batches.push(vec![entry]);
                continue;
            }

            if current_size + entry_size > MAX_PUT_BODY_BYTES && !current.is_empty() {
                batches.push(std::mem::take(&mut current));
                current_size = 0;
            }

            current_size += entry_size;
            current.push(entry);
        }

        if !current.is_empty() {
            batches.push(current);
        }

        batches
    }

    async fn upload_batch(&self, batch: Vec<TeamMemoryEntry>, state: &mut SyncState) -> Result<()> {
        let client = reqwest::Client::new();
        let url = format!(
            "{}/api/claude_code/team_memory?repo={}",
            self.api_base,
            urlencoding::encode(&self.repo),
        );

        let body = serde_json::json!({ "entries": batch });

        let mut req = client.put(&url).bearer_auth(&self.token).json(&body);

        if let Some(etag) = &state.last_known_etag {
            req = req.header("If-Match", etag);
        }

        let response = req
            .send()
            .await
            .context("team memory: PUT request failed")?;

        let status = response.status().as_u16();

        match status {
            200 | 201 | 204 => {
                if let Some(etag) = response.headers().get("etag").and_then(|v| v.to_str().ok()) {
                    state.last_known_etag = Some(etag.to_string());
                }
                // Update local checksum map to reflect uploaded state
                for entry in &batch {
                    state
                        .server_checksums
                        .insert(entry.key.clone(), entry.checksum.clone());
                }
                Ok(())
            }
            412 => anyhow::bail!("Conflict (412 Precondition Failed): ETag mismatch, retry needed"),
            413 => anyhow::bail!("Payload too large (413)"),
            401 | 403 => anyhow::bail!("Authentication error ({})", status),
            _ => anyhow::bail!("Upload failed with status {}", status),
        }
    }

    /// Recursively scan `team_dir` for `.md` files, returning entries sorted by key.
    async fn scan_local_files(&self) -> Result<Vec<TeamMemoryEntry>> {
        let mut entries = Vec::new();

        if !self.team_dir.exists() {
            return Ok(entries);
        }

        // Iterative DFS using an explicit stack to avoid deep recursion
        let mut stack = vec![self.team_dir.clone()];

        while let Some(dir) = stack.pop() {
            let mut read_dir = tokio::fs::read_dir(&dir)
                .await
                .with_context(|| format!("read_dir {:?}", dir))?;

            while let Some(entry) = read_dir.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().map(|e| e == "md").unwrap_or(false) {
                    let content = tokio::fs::read_to_string(&path)
                        .await
                        .with_context(|| format!("reading {:?}", path))?;

                    if content.len() > MAX_FILE_SIZE_BYTES {
                        continue; // Skip files that are too large
                    }

                    let key = path
                        .strip_prefix(&self.team_dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");

                    let checksum = content_checksum(&content);
                    entries.push(TeamMemoryEntry {
                        key,
                        content,
                        checksum,
                    });
                }
            }
        }

        entries.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// Secret scanner
// ---------------------------------------------------------------------------

/// A pattern matched during secret scanning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMatch {
    /// Short label identifying the secret type, e.g. `"Anthropic API key"`.
    pub label: String,
}

/// Scan `content` for common high-confidence secret patterns.
///
/// Returns one [`SecretMatch`] per distinct pattern that fired.  The actual
/// matched text is intentionally **not** returned to avoid logging credentials.
pub fn scan_for_secrets(content: &str) -> Vec<SecretMatch> {
    // Each tuple: (regex source, human-readable label)
    // Patterns ordered by likelihood of appearing in dev-team memory content.
    const PATTERNS: &[(&str, &str)] = &[
        // Cloud providers
        (
            r"(?:A3T[A-Z0-9]|AKIA|ASIA|ABIA|ACCA)[A-Z2-7]{16}",
            "AWS access key",
        ),
        (r"AIza[\w-]{35}", "GCP API key"),
        // AI APIs
        (r"sk-ant-api03-[a-zA-Z0-9_\-]{93}AA", "Anthropic API key"),
        (
            r"sk-ant-admin01-[a-zA-Z0-9_\-]{93}AA",
            "Anthropic admin API key",
        ),
        (
            r"sk-[a-zA-Z0-9]{20}T3BlbkFJ[a-zA-Z0-9]{20}",
            "OpenAI API key",
        ),
        // Version control
        (r"ghp_[0-9a-zA-Z]{36}", "GitHub personal access token"),
        (r"github_pat_\w{82}", "GitHub fine-grained PAT"),
        (r"(?:ghu|ghs)_[0-9a-zA-Z]{36}", "GitHub app token"),
        (r"gho_[0-9a-zA-Z]{36}", "GitHub OAuth token"),
        (r"glpat-[\w-]{20}", "GitLab PAT"),
        // Communication
        (
            r"xoxb-[0-9]{10,13}-[0-9]{10,13}[a-zA-Z0-9-]*",
            "Slack bot token",
        ),
        // Crypto / private keys
        (r"-----BEGIN[ A-Z0-9_-]{0,100}PRIVATE KEY", "Private key"),
        // Payments
        (
            r"(?:sk|rk)_(?:test|live|prod)_[a-zA-Z0-9]{10,99}",
            "Stripe secret key",
        ),
        // NPM
        (r"npm_[a-zA-Z0-9]{36}", "NPM access token"),
    ];

    let mut findings: Vec<SecretMatch> = Vec::new();

    for (pattern, label) in PATTERNS {
        // Lazily compile; the fn is not hot enough to warrant a static cache here
        if let Ok(re) = regex::Regex::new(pattern) {
            if re.is_match(content) {
                findings.push(SecretMatch {
                    label: label.to_string(),
                });
            }
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // --- content_checksum ---

    #[test]
    fn test_checksum_format() {
        let cs = content_checksum("hello");
        assert!(
            cs.starts_with("sha256:"),
            "checksum should start with sha256:"
        );
        assert_eq!(cs.len(), "sha256:".len() + 64, "sha256 hex is 64 chars");
    }

    #[test]
    fn test_checksum_deterministic() {
        assert_eq!(content_checksum("foo"), content_checksum("foo"));
    }

    #[test]
    fn test_checksum_distinct() {
        assert_ne!(content_checksum("foo"), content_checksum("bar"));
    }

    // --- validate_memory_path ---

    #[test]
    fn test_valid_paths_accepted() {
        let ok_paths = [
            "MEMORY.md",
            "sub/dir/file.md",
            "sub/dir/another-file.md",
            "a.md",
        ];
        for p in &ok_paths {
            assert!(validate_memory_path(p).is_ok(), "should accept: {}", p);
        }
    }

    #[test]
    fn test_null_byte_rejected() {
        assert!(validate_memory_path("foo\0bar").is_err());
    }

    #[test]
    fn test_url_encoded_dot_rejected() {
        assert!(validate_memory_path("%2e%2e/secret").is_err());
    }

    #[test]
    fn test_url_encoded_slash_rejected() {
        assert!(validate_memory_path("foo%2Fbar").is_err());
    }

    #[test]
    fn test_backslash_rejected() {
        assert!(validate_memory_path("foo\\bar").is_err());
    }

    #[test]
    fn test_absolute_unix_rejected() {
        assert!(validate_memory_path("/etc/passwd").is_err());
    }

    #[test]
    fn test_absolute_windows_rejected() {
        assert!(validate_memory_path("C:foo").is_err());
    }

    #[test]
    fn test_dotdot_rejected() {
        assert!(validate_memory_path("../secret").is_err());
        assert!(validate_memory_path("a/../../secret").is_err());
    }

    // --- pack_batches ---

    fn make_sync() -> TeamMemorySync {
        TeamMemorySync::new(
            "https://example.com".to_string(),
            "owner/repo".to_string(),
            "token123".to_string(),
            PathBuf::from("/tmp/team"),
        )
    }

    fn entry(key: &str, size: usize) -> TeamMemoryEntry {
        let content = "x".repeat(size);
        let checksum = content_checksum(&content);
        TeamMemoryEntry {
            key: key.to_string(),
            content,
            checksum,
        }
    }

    #[test]
    fn test_pack_batches_empty() {
        let sync = make_sync();
        let batches = sync.pack_batches(vec![]);
        assert!(batches.is_empty());
    }

    #[test]
    fn test_pack_batches_single_entry() {
        let sync = make_sync();
        let batches = sync.pack_batches(vec![entry("a.md", 100)]);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 1);
    }

    #[test]
    fn test_pack_batches_oversized_solo() {
        let sync = make_sync();
        // Entry > MAX_PUT_BODY_BYTES → goes solo
        let big = entry("big.md", MAX_PUT_BODY_BYTES + 1);
        let small = entry("small.md", 100);
        let batches = sync.pack_batches(vec![big, small]);
        // big is solo, small may be in a separate batch
        assert!(batches.len() >= 2);
        assert_eq!(batches[0].len(), 1, "oversized entry is solo");
    }

    #[test]
    fn test_pack_batches_groups_small_entries() {
        let sync = make_sync();
        // Many small entries that each fit in one batch
        let entries: Vec<_> = (0..5).map(|i| entry(&format!("{i}.md"), 1024)).collect();
        let batches = sync.pack_batches(entries);
        // All 5 should fit in one batch (5 * ~1124 bytes << 200KB)
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 5);
    }

    // --- scan_for_secrets ---

    #[test]
    fn test_no_secrets_clean() {
        let findings = scan_for_secrets("# Team notes\n\nSome markdown content here.");
        assert!(findings.is_empty());
    }

    #[test]
    fn test_detects_github_pat() {
        let content = format!("token: ghp_{}", "A".repeat(36));
        let findings = scan_for_secrets(&content);
        assert!(
            findings.iter().any(|m| m.label.contains("GitHub")),
            "should detect GitHub PAT"
        );
    }

    #[test]
    fn test_detects_aws_key() {
        let content = "key=AKIAIOSFODNN7EXAMPLE";
        let findings = scan_for_secrets(content);
        assert!(
            findings.iter().any(|m| m.label.contains("AWS")),
            "should detect AWS key"
        );
    }

    #[test]
    fn test_detects_private_key() {
        let content = "-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n";
        let findings = scan_for_secrets(content);
        assert!(
            findings.iter().any(|m| m.label.contains("Private key")),
            "should detect private key"
        );
    }

    // --- scan_local_files (integration-style) ---

    #[tokio::test]
    async fn test_scan_local_files_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let sync = TeamMemorySync::new(
            "https://example.com".to_string(),
            "r".to_string(),
            "t".to_string(),
            tmp.path().to_path_buf(),
        );
        let entries = sync.scan_local_files().await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_scan_local_files_finds_md() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("MEMORY.md"), "# Memory")
            .await
            .unwrap();
        tokio::fs::write(tmp.path().join("ignore.txt"), "not md")
            .await
            .unwrap();

        let sync = TeamMemorySync::new(
            "https://example.com".to_string(),
            "r".to_string(),
            "t".to_string(),
            tmp.path().to_path_buf(),
        );
        let entries = sync.scan_local_files().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "MEMORY.md");
    }

    #[tokio::test]
    async fn test_scan_local_files_sorted() {
        let tmp = TempDir::new().unwrap();
        tokio::fs::write(tmp.path().join("z.md"), "z")
            .await
            .unwrap();
        tokio::fs::write(tmp.path().join("a.md"), "a")
            .await
            .unwrap();
        tokio::fs::write(tmp.path().join("m.md"), "m")
            .await
            .unwrap();

        let sync = TeamMemorySync::new(
            "https://example.com".to_string(),
            "r".to_string(),
            "t".to_string(),
            tmp.path().to_path_buf(),
        );
        let entries = sync.scan_local_files().await.unwrap();
        let keys: Vec<_> = entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["a.md", "m.md", "z.md"]);
    }

    #[tokio::test]
    async fn test_scan_local_files_checksums_match() {
        let tmp = TempDir::new().unwrap();
        let content = "# Hello world";
        tokio::fs::write(tmp.path().join("MEMORY.md"), content)
            .await
            .unwrap();

        let sync = TeamMemorySync::new(
            "https://example.com".to_string(),
            "r".to_string(),
            "t".to_string(),
            tmp.path().to_path_buf(),
        );
        let entries = sync.scan_local_files().await.unwrap();
        assert_eq!(entries[0].checksum, content_checksum(content));
    }
}
