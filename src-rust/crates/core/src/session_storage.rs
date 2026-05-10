// session_storage.rs — JSONL transcript persistence for Claurst.
//
// File layout:  ~/.claurst/projects/{base64url(project_root)}/{session_id}.jsonl
//
// Each line is a JSON object ("entry") whose `type` field is the discriminant.
// The schema is kept compatible with the TypeScript `Entry` union in
// `src/types/logs.ts` so that files written by the TS CLI can be read here
// and vice-versa.
//
// Only the entry types that the Rust port generates are implemented here.
// Unknown/future entry types round-trip as `Other(Value)` so they are
// preserved when rewriting the file (tombstone path).

use std::path::{Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::Message;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum transcript file size for read operations (load / tombstone rewrite).
/// Files larger than this are not read to avoid OOM on huge sessions.
pub const MAX_TRANSCRIPT_BYTES: u64 = 50 * 1024 * 1024; // 50 MB

// ---------------------------------------------------------------------------
// TranscriptEntry — the wire-format discriminated union
// ---------------------------------------------------------------------------

/// A single line in a `.jsonl` transcript file.
///
/// Variants are serialised with a `"type"` field that matches the TypeScript
/// `Entry` union.  Only variants the Rust port actively uses are named; every
/// other entry type is preserved as a raw `serde_json::Value` via `Other`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TranscriptEntry {
    /// A user turn.
    User(TranscriptMessage),
    /// An assistant turn.
    Assistant(TranscriptMessage),
    /// An inline attachment (image, file, etc.).
    Attachment(TranscriptMessage),
    /// A system message.
    System(TranscriptMessage),
    /// A compacted-context summary produced by the compaction logic.
    Summary(SummaryEntry),
    /// An AI-generated session title (written by the auto-titler, not the user).
    #[serde(rename = "ai-title")]
    AiTitle(AiTitleEntry),
    /// A user-set custom session title.
    #[serde(rename = "custom-title")]
    CustomTitle(CustomTitleEntry),
    /// The most-recent user prompt, re-appended at session exit for fast tail reads.
    #[serde(rename = "last-prompt")]
    LastPrompt(LastPromptEntry),
    /// Marks an entry as deleted. The Rust port uses this to remove messages
    /// without rewriting the entire file.
    #[serde(rename = "tombstone")]
    Tombstone(TombstoneEntry),
    /// Any other entry type we do not need to inspect — round-tripped verbatim.
    #[serde(other, skip_serializing)]
    Unknown,
}

impl TranscriptEntry {
    /// Returns the `uuid` of the underlying message, if this is a transcript
    /// message type (user / assistant / attachment / system).
    pub fn uuid(&self) -> Option<&str> {
        match self {
            Self::User(m) | Self::Assistant(m) | Self::Attachment(m) | Self::System(m) => {
                m.uuid.as_deref()
            }
            _ => None,
        }
    }

    /// Return true if this entry is a user or assistant message
    /// (i.e. contributes to the conversation chain).
    pub fn is_chain_participant(&self) -> bool {
        matches!(self, Self::User(_) | Self::Assistant(_))
    }
}

// ---------------------------------------------------------------------------
// TranscriptMessage — the shape shared by user / assistant / system entries.
//
// Fields map to the TypeScript `TranscriptMessage` type in `src/types/logs.ts`.
// ---------------------------------------------------------------------------

/// A conversation message as stored in the transcript JSONL.
///
/// This is the serialised form of a single `user` or `assistant` turn.
/// It embeds a `message` object (the payload sent/received by the Anthropic API)
/// plus session-book-keeping fields used by the resume / history UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptMessage {
    /// Stable UUID for this entry (used as the primary key in the chain).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,

    /// UUID of the preceding chain-participant entry, or `null` at the start.
    pub parent_uuid: Option<String>,

    /// ISO-8601 timestamp when this entry was written.
    pub timestamp: String,

    /// Session ID (UUID) this entry belongs to.
    pub session_id: String,

    /// Working directory when this entry was written.
    pub cwd: String,

    /// The API message payload (role + content).
    pub message: Message,

    /// Whether this message belongs to a sidechain / sub-agent transcript.
    #[serde(default)]
    pub is_sidechain: bool,

    /// `"external"` | `"internal"` — mirrors TS `getUserType()`.
    #[serde(default = "default_user_type")]
    pub user_type: String,

    /// Version of the Claurst binary, mirrors `MACRO.VERSION`.
    #[serde(default)]
    pub version: String,

    /// Git branch at the time this message was written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,

    /// Agent role in the managed-agent architecture: "manager" | "executor".
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub agent_role: Option<String>,

    /// Managed session ID linking manager and executor transcripts.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub managed_session_id: Option<String>,

    /// Catch-all for any other fields written by the TS CLI that we don't
    /// need to inspect.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

fn default_user_type() -> String {
    "external".to_string()
}

// ---------------------------------------------------------------------------
// Metadata-only entry types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryEntry {
    /// UUID of the leaf message that this summary replaces.
    pub leaf_uuid: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTitleEntry {
    pub session_id: String,
    pub ai_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomTitleEntry {
    pub session_id: String,
    pub custom_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastPromptEntry {
    pub session_id: String,
    pub last_prompt: String,
}

/// Written to mark a message UUID as deleted (soft-delete via append-only
/// tombstoning). The loader skips any entry whose uuid appears in a tombstone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TombstoneEntry {
    /// UUID of the entry being deleted.
    pub deleted_uuid: String,
}

// ---------------------------------------------------------------------------
// SessionSummary — lightweight metadata returned by list_sessions()
// ---------------------------------------------------------------------------

/// Lightweight metadata for one session, extracted from the last entry in
/// the transcript without loading the full file.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub path: PathBuf,
    pub mtime: std::time::SystemTime,
    /// The last-prompt text found in the tail, if any.
    pub last_prompt: Option<String>,
    /// The custom title found in the tail, if any.
    pub title: Option<String>,
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Returns the base projects directory: `~/.claurst/projects/`.
pub fn projects_dir() -> PathBuf {
    crate::config::Settings::config_dir().join("projects")
}

/// Returns the per-project transcript directory.
///
/// The project root path is encoded using **URL-safe base64 without padding**
/// to produce a stable, platform-safe directory name that is fully reversible
/// (unlike the TS `sanitizePath` which just replaces chars with hyphens).
pub fn transcript_dir(project_root: &Path) -> PathBuf {
    let encoded = URL_SAFE_NO_PAD.encode(project_root.to_string_lossy().as_bytes());
    projects_dir().join(encoded)
}

/// Returns the full path to a session's JSONL transcript file.
pub fn transcript_path(project_root: &Path, session_id: &str) -> PathBuf {
    transcript_dir(project_root).join(format!("{}.jsonl", session_id))
}

// ---------------------------------------------------------------------------
// Core I/O operations
// ---------------------------------------------------------------------------

/// Append a single entry to a JSONL transcript file.
///
/// * Creates parent directories if they do not exist.
/// * Is a no-op (returns `Ok(())`) when the file already exceeds
///   [`MAX_TRANSCRIPT_BYTES`] to avoid unbounded growth.
/// * Uses `OpenOptions::append(true)` which results in an atomic positional
///   write on POSIX (O_APPEND) and a best-effort append on Windows.
pub async fn write_transcript_entry(
    path: &Path,
    entry: &TranscriptEntry,
) -> crate::Result<()> {
    // Guard: do not grow files beyond the cap.
    if let Ok(meta) = tokio::fs::metadata(path).await {
        if meta.len() >= MAX_TRANSCRIPT_BYTES {
            return Ok(());
        }
    }

    // Serialise to a single compact JSON line terminated by '\n'.
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');

    // Ensure parent directory exists before attempting the write.
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Open in append mode; create if absent.
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;

    file.write_all(line.as_bytes()).await?;
    Ok(())
}

/// Load all non-tombstoned entries from a JSONL transcript file.
///
/// * Returns an empty `Vec` if the file does not exist.
/// * Bails out with an error if the file exceeds [`MAX_TRANSCRIPT_BYTES`]
///   to protect against OOM.
/// * Lines that fail to parse are silently skipped (forward-compatibility).
/// * Any entry whose uuid appears in a `Tombstone` entry is excluded.
pub async fn load_transcript(path: &Path) -> crate::Result<Vec<TranscriptEntry>> {
    // Fast-path: file absent → empty session.
    match tokio::fs::metadata(path).await {
        Ok(meta) if meta.len() > MAX_TRANSCRIPT_BYTES => {
            return Err(crate::ClaudeError::Other(format!(
                "Transcript file too large to load ({} bytes, max {})",
                meta.len(),
                MAX_TRANSCRIPT_BYTES,
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(vec![]);
        }
        Err(e) => return Err(e.into()),
        Ok(_) => {}
    }

    let raw = tokio::fs::read_to_string(path).await?;

    // First pass: collect tombstoned UUIDs.
    let mut tombstoned: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Cheap structural check before full parse.
        if trimmed.contains("\"type\":\"tombstone\"") || trimmed.contains("\"type\": \"tombstone\"") {
            if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(trimmed) {
                if let TranscriptEntry::Tombstone(t) = entry {
                    tombstoned.insert(t.deleted_uuid);
                }
            }
        }
    }

    // Second pass: collect valid non-tombstoned entries.
    let mut entries = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let entry: TranscriptEntry = match serde_json::from_str(trimmed) {
            Ok(e) => e,
            Err(_) => continue, // skip malformed lines
        };

        // Skip tombstones themselves and tombstoned entries.
        match &entry {
            TranscriptEntry::Tombstone(_) => continue,
            TranscriptEntry::Unknown => continue,
            _ => {}
        }

        if let Some(uuid) = entry.uuid() {
            if tombstoned.contains(uuid) {
                continue;
            }
        }

        entries.push(entry);
    }

    Ok(entries)
}

/// List all `.jsonl` session files under the project's transcript directory,
/// sorted by modification time (newest first).
///
/// For each file, a cheap tail-read extracts the `last-prompt` and
/// `custom-title` metadata without loading the full transcript.
pub async fn list_sessions(project_root: &Path) -> crate::Result<Vec<SessionSummary>> {
    let dir = transcript_dir(project_root);

    let mut dir_entries = match tokio::fs::read_dir(&dir).await {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(vec![]);
        }
        Err(e) => return Err(e.into()),
    };

    let mut sessions: Vec<SessionSummary> = Vec::new();

    while let Ok(Some(entry)) = dir_entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }

        // Extract session ID from the stem.
        let session_id = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = meta
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        // Read the tail of the file (up to 64 KB) to extract metadata.
        let (last_prompt, title) = read_session_tail_metadata(&path).await;

        sessions.push(SessionSummary {
            session_id,
            path,
            mtime,
            last_prompt,
            title,
        });
    }

    // Sort newest-first.
    sessions.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    Ok(sessions)
}

/// Append a `Tombstone` entry that marks `uuid` as deleted.
///
/// This is the append-only soft-delete path.  On resume,
/// [`load_transcript`] will skip the tombstoned entry.
///
/// If the file exceeds [`MAX_TRANSCRIPT_BYTES`] the tombstone is not written
/// (same guard as [`write_transcript_entry`]).
pub async fn tombstone_entry(path: &Path, uuid: &str) -> crate::Result<()> {
    let entry = TranscriptEntry::Tombstone(TombstoneEntry {
        deleted_uuid: uuid.to_string(),
    });
    write_transcript_entry(path, &entry).await
}

/// Truncate a session transcript at the entry whose `uuid` matches `from_uuid`,
/// removing that entry and all subsequent entries.
///
/// Used by `/revert` to discard assistant turns after a given message.
/// Rewrites the file atomically (load → filter → overwrite).
pub async fn truncate_after(path: &Path, from_uuid: &str) -> crate::Result<()> {
    let entries = load_transcript(path).await?;
    let mut keep = Vec::new();
    let mut found = false;
    for entry in entries {
        if found { continue; }
        match &entry {
            TranscriptEntry::User(m) | TranscriptEntry::Assistant(m) => {
                if m.message.uuid.as_deref() == Some(from_uuid) {
                    found = true;
                    continue; // drop this entry and everything after
                }
            }
            _ => {}
        }
        keep.push(entry);
    }
    // Rewrite the file with only the kept entries.
    let mut lines = String::new();
    for e in &keep {
        lines.push_str(&serde_json::to_string(e).map_err(crate::error::ClaudeError::from)?);
        lines.push('\n');
    }
    tokio::fs::write(path, lines).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helper: read tail metadata without a full parse
// ---------------------------------------------------------------------------

/// Reads up to 64 KB from the end of `path` and extracts `last-prompt` and
/// `custom-title` values by scanning JSONL lines.
///
/// Returns `(last_prompt, custom_title)`.  Both are `None` if the relevant
/// entries are absent or the file cannot be read.
async fn read_session_tail_metadata(path: &Path) -> (Option<String>, Option<String>) {
    const TAIL_BUF: u64 = 65_536; // 64 KB

    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => return (None, None),
    };
    let meta = match file.metadata().await {
        Ok(m) => m,
        Err(_) => return (None, None),
    };
    let file_size = meta.len();
    if file_size == 0 {
        return (None, None);
    }

    // Seek to the start of the tail window.
    let offset = file_size.saturating_sub(TAIL_BUF);
    let mut buf = vec![0u8; (file_size - offset) as usize];

    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut file = file;
    if let Err(_) = file
        .seek(std::io::SeekFrom::Start(offset))
        .await
    {
        return (None, None);
    }
    if let Err(_) = file.read_exact(&mut buf).await {
        return (None, None);
    }

    // Scan lines in reverse order so we get the last occurrence of each field.
    let text = String::from_utf8_lossy(&buf);
    let mut last_prompt: Option<String> = None;
    let mut title: Option<String> = None;

    for line in text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if last_prompt.is_none()
            && (trimmed.contains("\"type\":\"last-prompt\"")
                || trimmed.contains("\"type\": \"last-prompt\""))
        {
            if let Ok(e) = serde_json::from_str::<TranscriptEntry>(trimmed) {
                if let TranscriptEntry::LastPrompt(lp) = e {
                    last_prompt = Some(lp.last_prompt);
                }
            }
        }

        if title.is_none()
            && (trimmed.contains("\"type\":\"custom-title\"")
                || trimmed.contains("\"type\": \"custom-title\""))
        {
            if let Ok(e) = serde_json::from_str::<TranscriptEntry>(trimmed) {
                if let TranscriptEntry::CustomTitle(ct) = e {
                    title = Some(ct.custom_title);
                }
            }
        }

        if last_prompt.is_some() && title.is_some() {
            break;
        }
    }

    (last_prompt, title)
}

// ---------------------------------------------------------------------------
// Convenience constructor helpers used by main.rs
// ---------------------------------------------------------------------------

/// Build a `TranscriptEntry::User` from a bare `Message`.
///
/// `parent_uuid` is the UUID of the preceding chain-participant entry.
pub fn make_user_entry(
    message: Message,
    uuid: &str,
    parent_uuid: Option<&str>,
    session_id: &str,
    cwd: &str,
) -> TranscriptEntry {
    TranscriptEntry::User(TranscriptMessage {
        uuid: Some(uuid.to_string()),
        parent_uuid: parent_uuid.map(|s| s.to_string()),
        timestamp: chrono::Utc::now().to_rfc3339(),
        session_id: session_id.to_string(),
        cwd: cwd.to_string(),
        message,
        is_sidechain: false,
        user_type: "external".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_branch: None,
        agent_role: None,
        managed_session_id: None,
        extra: Default::default(),
    })
}

/// Build a `TranscriptEntry::Assistant` from a bare `Message`.
pub fn make_assistant_entry(
    message: Message,
    uuid: &str,
    parent_uuid: Option<&str>,
    session_id: &str,
    cwd: &str,
) -> TranscriptEntry {
    TranscriptEntry::Assistant(TranscriptMessage {
        uuid: Some(uuid.to_string()),
        parent_uuid: parent_uuid.map(|s| s.to_string()),
        timestamp: chrono::Utc::now().to_rfc3339(),
        session_id: session_id.to_string(),
        cwd: cwd.to_string(),
        message,
        is_sidechain: false,
        user_type: "external".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_branch: None,
        agent_role: None,
        managed_session_id: None,
        extra: Default::default(),
    })
}

/// Reconstruct `Vec<Message>` from a loaded transcript, in conversation order.
///
/// Only `user` and `assistant` entries are returned; metadata entries
/// (summary, custom-title, etc.) are discarded.  The order matches the on-disk
/// parentUuid chain: messages are returned in the order they appear in the
/// file, which is append-order and therefore chronological for the main chain.
pub fn messages_from_transcript(entries: &[TranscriptEntry]) -> Vec<Message> {
    entries
        .iter()
        .filter_map(|e| match e {
            TranscriptEntry::User(m) | TranscriptEntry::Assistant(m) => {
                Some(m.message.clone())
            }
            _ => None,
        })
        .collect()
}

/// Filter transcript entries by agent role ("manager" or "executor").
///
/// Returns only User and Assistant entries whose `agent_role` matches `role`.
pub fn filter_by_agent_role<'a>(entries: &'a [TranscriptEntry], role: &str) -> Vec<&'a TranscriptEntry> {
    entries.iter().filter(|e| {
        match e {
            TranscriptEntry::User(msg) | TranscriptEntry::Assistant(msg) => {
                msg.agent_role.as_deref() == Some(role)
            }
            _ => false,
        }
    }).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use crate::types::{Message, MessageContent, Role};

    fn make_msg(role: Role) -> Message {
        Message {
            role,
            content: MessageContent::Text("hello".to_string()),
            uuid: Some(uuid::Uuid::new_v4().to_string()),
            cost: None,
            snapshot_patch: None,
        }
    }

    #[tokio::test]
    async fn round_trip_user_message() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.jsonl");

        let msg = make_msg(Role::User);
        let uuid = uuid::Uuid::new_v4().to_string();
        let entry = make_user_entry(msg.clone(), &uuid, None, "sess-1", "/home/user/proj");
        write_transcript_entry(&path, &entry).await.unwrap();

        let loaded = load_transcript(&path).await.unwrap();
        assert_eq!(loaded.len(), 1);
        if let TranscriptEntry::User(m) = &loaded[0] {
            assert_eq!(m.uuid.as_deref(), Some(uuid.as_str()));
        } else {
            panic!("expected User entry");
        }
    }

    #[tokio::test]
    async fn tombstone_removes_entry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.jsonl");

        let uuid = uuid::Uuid::new_v4().to_string();
        let msg = make_msg(Role::User);
        let entry = make_user_entry(msg, &uuid, None, "sess-1", "/proj");
        write_transcript_entry(&path, &entry).await.unwrap();

        tombstone_entry(&path, &uuid).await.unwrap();

        let loaded = load_transcript(&path).await.unwrap();
        assert_eq!(loaded.len(), 0, "tombstoned entry should be excluded");
    }

    #[tokio::test]
    async fn list_sessions_returns_sorted() {
        let tmp = tempdir().unwrap();
        let project_root = tmp.path().join("myproject");
        tokio::fs::create_dir_all(&project_root).await.unwrap();

        let tdir = transcript_dir(&project_root);
        tokio::fs::create_dir_all(&tdir).await.unwrap();

        for id in ["aaaa", "bbbb"] {
            let p = tdir.join(format!("{}.jsonl", id));
            let msg = make_msg(Role::User);
            let uuid_val = uuid::Uuid::new_v4().to_string();
            let entry = make_user_entry(msg, &uuid_val, None, id, "/proj");
            write_transcript_entry(&p, &entry).await.unwrap();
            // Small sleep to ensure different mtimes.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let sessions = list_sessions(&project_root).await.unwrap();
        assert_eq!(sessions.len(), 2);
        // Newest first.
        assert_eq!(sessions[0].session_id, "bbbb");
        assert_eq!(sessions[1].session_id, "aaaa");
    }

    #[test]
    fn transcript_path_encoding_is_reversible() {
        let root = Path::new("/Users/alice/my-project");
        let path = transcript_path(root, "test-session");
        // The directory component after "projects/" should decode back to the root.
        let encoded_dir = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let decoded = URL_SAFE_NO_PAD.decode(encoded_dir).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), root.to_str().unwrap());
    }
}
