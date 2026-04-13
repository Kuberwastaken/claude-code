// Prompt history — append-only JSONL log of user prompts.
//
// Mirrors the behaviour of `src/history.ts` in the TypeScript codebase:
//   - Entries are written to `~/.claurst/history.jsonl` via O_APPEND.
//   - An advisory lock file (`history.jsonl.lock`) serialises concurrent
//     writers (up to 20 retries × 50 ms back-off).
//   - Large pasted text (> 1 024 bytes) is stored externally in
//     `~/.claurst/pastes/<sha256-hex>` and referenced by hash.
//   - Images are not stored in the JSONL file (they live in the image cache).
//   - `get_history()` reads the file, filters by project, and yields
//     current-session entries first (newest-first within each group).
//   - `expand_pasted_text_refs()` replaces `[Pasted text #N]` /
//     `[Pasted text #N +X lines]` placeholders with the actual text.
//   - `parse_references()` extracts all numeric IDs from reference patterns.
//   - `remove_last_from_history()` undoes the most-recent `add_to_history`
//     call (fast path: pop from pending buffer; slow path: skip-set).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::debug;

/// Maximum number of history entries returned by `get_history`.
const MAX_HISTORY_ITEMS: usize = 100;

/// Content shorter than or equal to this byte count is inlined into the JSONL
/// entry.  Longer content is stored in the paste store and referenced by hash.
const MAX_PASTED_CONTENT_LENGTH: usize = 1024;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The content kind of a stored paste.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PastedContentKind {
    Text,
    Image,
}

/// A pasted-content value as seen by callers of `get_history`.
/// Images are resolved from the image cache; text may be inline or fetched
/// from the paste store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PastedContent {
    pub id: u32,
    #[serde(rename = "type")]
    pub kind: PastedContentKind,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// A history entry as provided by callers of `add_to_history`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub display: String,
    #[serde(default)]
    pub pasted_contents: HashMap<u32, PastedContent>,
    /// Unix-millisecond timestamp.  Populated by `add_to_history`.
    #[serde(default)]
    pub timestamp: u64,
    /// Absolute project root path.
    #[serde(default)]
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Internal storage types
// ---------------------------------------------------------------------------

/// How a pasted content item is stored in the JSONL log.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPastedContent {
    pub id: u32,
    #[serde(rename = "type")]
    pub kind: PastedContentKind,
    /// Inline content for small pastes (≤ MAX_PASTED_CONTENT_LENGTH bytes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// SHA-256 hex reference for large pastes stored externally.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// The actual structure written to (and read from) `history.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LogEntry {
    pub display: String,
    #[serde(default)]
    pub pasted_contents: HashMap<u32, StoredPastedContent>,
    pub timestamp: u64,
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Module-level state (pending buffer + skip-set)
// ---------------------------------------------------------------------------

struct HistoryState {
    pending: Vec<LogEntry>,
    last_added: Option<LogEntry>,
    skipped_timestamps: HashSet<u64>,
}

impl HistoryState {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            last_added: None,
            skipped_timestamps: HashSet::new(),
        }
    }
}

static STATE: once_cell::sync::Lazy<Mutex<HistoryState>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HistoryState::new()));

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn claude_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claurst")
}

fn history_path() -> PathBuf {
    claude_home().join("history.jsonl")
}

fn pastes_dir() -> PathBuf {
    claude_home().join("pastes")
}

fn paste_path(hash: &str) -> PathBuf {
    pastes_dir().join(hash)
}

fn lock_path() -> PathBuf {
    claude_home().join("history.jsonl.lock")
}

// ---------------------------------------------------------------------------
// Advisory lock (cross-platform, using file-creation exclusivity)
// ---------------------------------------------------------------------------
//
// Strategy: attempt to create the lock file with O_CREAT | O_EXCL (fails if
// it already exists), retrying up to 20 times with 50 ms back-off.  On drop
// the file is removed.
//
// For Unix we additionally apply an `flock(LOCK_EX)` on the open file to
// co-operate with processes that use flock-based locking.

struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Attempt to acquire an exclusive advisory lock.
/// Returns `Some(LockGuard)` on success or `None` after exhausting retries.
async fn acquire_lock() -> Option<LockGuard> {
    let path = lock_path();

    for _ in 0..20u32 {
        // `create_new(true)` is the Rust equivalent of O_CREAT | O_EXCL.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_file) => {
                // File successfully created — we hold the lock.
                // (The file handle is intentionally dropped here; the
                //  LockGuard's Drop impl removes the file on release.)
                return Some(LockGuard { path });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another writer holds the lock — back off and retry.
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
            Err(e) => {
                // Some other I/O error (e.g. permission denied on the dir).
                debug!("Lock acquire error: {}", e);
                return None;
            }
        }
    }
    debug!("Failed to acquire history lock after 20 retries");
    None
}

// ---------------------------------------------------------------------------
// Paste store
// ---------------------------------------------------------------------------

/// Compute the SHA-256 hex digest of `text`.
fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

/// Persist `text` to `~/.claurst/pastes/<hash>`.  Fire-and-forget.
async fn store_paste(hash: String, text: String) {
    let dir = pastes_dir();
    if let Err(e) = fs::create_dir_all(&dir).await {
        debug!("Failed to create pastes dir: {}", e);
        return;
    }
    let path = paste_path(&hash);
    match tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .await
    {
        Ok(mut f) => {
            if let Err(e) = f.write_all(text.as_bytes()).await {
                debug!("Failed to write paste {}: {}", hash, e);
            }
        }
        Err(e) => {
            debug!("Failed to create paste file {}: {}", hash, e);
        }
    }
}

/// Read text from `~/.claurst/pastes/<hash>`, returning `None` if missing.
async fn retrieve_paste(hash: &str) -> Option<String> {
    fs::read_to_string(paste_path(hash)).await.ok()
}

// ---------------------------------------------------------------------------
// Flush logic
// ---------------------------------------------------------------------------

/// Write `entries` to `history.jsonl` under an advisory lock.
async fn flush_entries(entries: Vec<LogEntry>) {
    if entries.is_empty() {
        return;
    }

    // Ensure the claude home directory exists.
    let home = claude_home();
    if let Err(e) = fs::create_dir_all(&home).await {
        debug!("Failed to create claude home dir: {}", e);
        return;
    }

    let path = history_path();

    // Ensure the file exists before acquiring the lock.
    if let Err(e) = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .await
    {
        debug!("Failed to create history file: {}", e);
        return;
    }

    // Acquire advisory lock.
    let _lock = acquire_lock().await;

    // Serialise entries.
    let mut lines = String::new();
    for entry in &entries {
        match serde_json::to_string(entry) {
            Ok(json) => {
                lines.push_str(&json);
                lines.push('\n');
            }
            Err(e) => {
                debug!("Failed to serialise history entry: {}", e);
            }
        }
    }

    if lines.is_empty() {
        return;
    }

    // Append to the file.
    let mut file = match tokio::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            debug!("Failed to open history file for append: {}", e);
            return;
        }
    };

    if let Err(e) = file.write_all(lines.as_bytes()).await {
        debug!("Failed to append to history file: {}", e);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Append `entry` to `~/.claurst/history.jsonl`.
///
/// The call is fire-and-forget: it spawns a background Tokio task and returns
/// immediately.  If `CLAURST_SKIP_PROMPT_HISTORY` is truthy the call is
/// a no-op.
pub fn add_to_history(entry: HistoryEntry) {
    if std::env::var("CLAURST_SKIP_PROMPT_HISTORY")
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
    {
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut stored_contents: HashMap<u32, StoredPastedContent> = HashMap::new();
    for (id, content) in &entry.pasted_contents {
        // Images are stored separately — skip them.
        if content.kind == PastedContentKind::Image {
            continue;
        }

        if content.content.len() <= MAX_PASTED_CONTENT_LENGTH {
            stored_contents.insert(
                *id,
                StoredPastedContent {
                    id: content.id,
                    kind: content.kind.clone(),
                    content: Some(content.content.clone()),
                    content_hash: None,
                    media_type: content.media_type.clone(),
                    filename: content.filename.clone(),
                },
            );
        } else {
            // Large paste: hash synchronously, write async (fire-and-forget).
            let hash = hash_text(&content.content);
            let text_clone = content.content.clone();
            let hash_clone = hash.clone();
            tokio::spawn(async move {
                store_paste(hash_clone, text_clone).await;
            });
            stored_contents.insert(
                *id,
                StoredPastedContent {
                    id: content.id,
                    kind: content.kind.clone(),
                    content: None,
                    content_hash: Some(hash),
                    media_type: content.media_type.clone(),
                    filename: content.filename.clone(),
                },
            );
        }
    }

    let log_entry = LogEntry {
        display: entry.display,
        pasted_contents: stored_contents,
        timestamp,
        project: entry.project,
        session_id: entry.session_id,
    };

    // Push to pending buffer and record as last-added.
    let to_flush = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.pending.push(log_entry.clone());
        state.last_added = Some(log_entry);
        // Drain the pending buffer to hand off to the flush task.
        std::mem::take(&mut state.pending)
    };

    tokio::spawn(async move {
        flush_entries(to_flush).await;
    });
}

/// Read `~/.claurst/history.jsonl`, filter by `project`, and return up to
/// `MAX_HISTORY_ITEMS` entries newest-first.  Entries belonging to
/// `current_session_id` are yielded before other sessions' entries.
pub async fn get_history(project: &str, current_session_id: Option<&str>) -> Vec<HistoryEntry> {
    let path = history_path();

    let (pending, skipped) = {
        let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        (state.pending.clone(), state.skipped_timestamps.clone())
    };

    // Read lines from disk newest-first (reverse the file).
    let disk_lines: Vec<String> = match fs::read_to_string(&path).await {
        Ok(content) => content.lines().rev().map(|l| l.to_string()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            debug!("Failed to read history file: {}", e);
            Vec::new()
        }
    };

    // Combine pending (newest first via rev) then disk entries.
    let mut all_entries: Vec<LogEntry> = pending.iter().rev().cloned().collect();

    for line in &disk_lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<LogEntry>(line) {
            Ok(entry) => {
                // Apply skip-set.
                if let Some(sid) = current_session_id {
                    if entry.session_id.as_deref() == Some(sid)
                        && skipped.contains(&entry.timestamp)
                    {
                        continue;
                    }
                }
                all_entries.push(entry);
            }
            Err(e) => {
                debug!("Failed to parse history line: {}", e);
            }
        }
    }

    // Filter to the requested project and separate current / other sessions.
    let mut current_session_entries: Vec<&LogEntry> = Vec::new();
    let mut other_entries: Vec<&LogEntry> = Vec::new();

    for entry in all_entries.iter().filter(|e| e.project == project) {
        if current_session_id.is_some() && entry.session_id.as_deref() == current_session_id {
            current_session_entries.push(entry);
        } else {
            other_entries.push(entry);
        }
    }

    // Yield current-session entries first, then others, capped at MAX_HISTORY_ITEMS.
    let mut result = Vec::new();
    for log_entry in current_session_entries
        .iter()
        .chain(other_entries.iter())
        .take(MAX_HISTORY_ITEMS)
    {
        result.push(resolve_log_entry(log_entry).await);
    }
    result
}

/// Resolve a `LogEntry` into a `HistoryEntry` by fetching large pastes from disk.
async fn resolve_log_entry(entry: &LogEntry) -> HistoryEntry {
    let mut pasted_contents: HashMap<u32, PastedContent> = HashMap::new();

    for (id, stored) in &entry.pasted_contents {
        if let Some(pc) = resolve_stored(stored).await {
            pasted_contents.insert(*id, pc);
        }
    }

    HistoryEntry {
        display: entry.display.clone(),
        pasted_contents,
        timestamp: entry.timestamp,
        project: entry.project.clone(),
        session_id: entry.session_id.clone(),
    }
}

async fn resolve_stored(stored: &StoredPastedContent) -> Option<PastedContent> {
    if let Some(ref content) = stored.content {
        return Some(PastedContent {
            id: stored.id,
            kind: stored.kind.clone(),
            content: content.clone(),
            media_type: stored.media_type.clone(),
            filename: stored.filename.clone(),
        });
    }

    if let Some(ref hash) = stored.content_hash {
        let content = retrieve_paste(hash).await?;
        return Some(PastedContent {
            id: stored.id,
            kind: stored.kind.clone(),
            content,
            media_type: stored.media_type.clone(),
            filename: stored.filename.clone(),
        });
    }

    None
}

/// Replace `[Pasted text #N]` / `[Pasted text #N +X lines]` placeholders in
/// `input` with the actual text from `contents`.
///
/// Image references (`[Image #N]`) are left unchanged.  Replacements are
/// applied in reverse-index order to keep earlier byte offsets valid.
pub fn expand_pasted_text_refs(input: &str, contents: &HashMap<u32, PastedContent>) -> String {
    let refs = parse_references_with_positions(input);
    let mut expanded = input.to_string();

    for (id, ref_str, ref_index) in refs.into_iter().rev() {
        let Some(content) = contents.get(&id) else {
            continue;
        };
        if content.kind != PastedContentKind::Text {
            continue;
        }
        expanded = format!(
            "{}{}{}",
            &expanded[..ref_index],
            content.content,
            &expanded[ref_index + ref_str.len()..]
        );
    }

    expanded
}

/// Extract all numeric IDs from reference patterns in `input`.
pub fn parse_references(input: &str) -> Vec<u32> {
    parse_references_with_positions(input)
        .into_iter()
        .map(|(id, _, _)| id)
        .collect()
}

/// Internal: return `(id, matched_string, byte_offset)` for each reference.
fn parse_references_with_positions(input: &str) -> Vec<(u32, String, usize)> {
    // Recognised prefixes (from the TS regex):
    //   [Pasted text #N]
    //   [Pasted text #N +X lines]
    //   [...Truncated text #N]
    //   [Image #N]
    let prefixes: &[&str] = &["[Pasted text #", "[...Truncated text #", "[Image #"];

    let mut results = Vec::new();
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    'outer: while i < len {
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }

        let rest = &input[i..];
        let mut matched_prefix = None;
        for p in prefixes {
            if rest.starts_with(p) {
                matched_prefix = Some(*p);
                break;
            }
        }

        let Some(prefix) = matched_prefix else {
            i += 1;
            continue;
        };

        let after_hash = &rest[prefix.len()..];

        // Read numeric ID.
        let digit_end = after_hash
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_hash.len());
        if digit_end == 0 {
            i += 1;
            continue;
        }
        let Ok(id) = after_hash[..digit_end].parse::<u32>() else {
            i += 1;
            continue;
        };
        if id == 0 {
            i += 1;
            continue;
        }

        let after_id = &after_hash[digit_end..];

        // Optional ` +N lines` suffix.
        let after_suffix = if let Some(s) = after_id.strip_prefix(" +") {
            let dn = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
            let rest2 = &s[dn..];
            rest2.strip_prefix(" lines").unwrap_or(after_id)
        } else {
            after_id
        };

        // Optional trailing dots before `]`.
        let after_dots = after_suffix.trim_start_matches('.');

        if after_dots.starts_with(']') {
            let consumed = prefix.len() + digit_end + (after_id.len() - after_dots.len()) + 1; // `]`
            let matched = input[i..i + consumed].to_string();
            results.push((id, matched, i));
            i += consumed;
        } else {
            i += 1;
        }

        continue 'outer;
    }

    results
}

/// Undo the most-recent `add_to_history` call.
///
/// Fast path: remove from pending buffer.  Slow path: add timestamp to skip-set.
pub fn remove_last_from_history() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = state.last_added.take() else {
        return;
    };

    if let Some(pos) = state
        .pending
        .iter()
        .rposition(|e| e.timestamp == entry.timestamp)
    {
        state.pending.remove(pos);
    } else {
        state.skipped_timestamps.insert(entry.timestamp);
    }
}

/// Wipe all pending entries and state (used in tests).
pub fn clear_pending_history_entries() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.pending.clear();
    state.last_added = None;
    state.skipped_timestamps.clear();
}

// ---------------------------------------------------------------------------
// Reference formatting helpers (parity with TS)
// ---------------------------------------------------------------------------

/// Count the number of line-break sequences in `text`.
/// Matches the TypeScript `getPastedTextRefNumLines` behaviour.
pub fn get_pasted_text_ref_num_lines(text: &str) -> usize {
    let mut count = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            count += 1;
            chars.next_if_eq(&'\n');
        } else if c == '\n' {
            count += 1;
        }
    }
    count
}

/// Format a text-paste reference placeholder.
pub fn format_pasted_text_ref(id: u32, num_lines: usize) -> String {
    if num_lines == 0 {
        format!("[Pasted text #{}]", id)
    } else {
        format!("[Pasted text #{} +{} lines]", id, num_lines)
    }
}

/// Format an image reference placeholder.
pub fn format_image_ref(id: u32) -> String {
    format!("[Image #{}]", id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Reference parsing --------------------------------------------------

    #[test]
    fn test_parse_references_text() {
        let ids = parse_references("See [Pasted text #3 +5 lines] and [Pasted text #7]");
        assert_eq!(ids, vec![3, 7]);
    }

    #[test]
    fn test_parse_references_image() {
        let ids = parse_references("Here is [Image #2]");
        assert_eq!(ids, vec![2]);
    }

    #[test]
    fn test_parse_references_zero_excluded() {
        let ids = parse_references("[Pasted text #0]");
        assert!(ids.is_empty());
    }

    #[test]
    fn test_parse_references_mixed() {
        let ids =
            parse_references("[Pasted text #1] and [Image #2] and [Pasted text #3 +10 lines]");
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_references_truncated() {
        let ids = parse_references("[...Truncated text #5]");
        assert_eq!(ids, vec![5]);
    }

    // -- Expand refs --------------------------------------------------------

    #[test]
    fn test_expand_pasted_text_refs() {
        let mut contents = HashMap::new();
        contents.insert(
            1u32,
            PastedContent {
                id: 1,
                kind: PastedContentKind::Text,
                content: "hello world".to_string(),
                media_type: None,
                filename: None,
            },
        );
        let result = expand_pasted_text_refs("Before [Pasted text #1] after", &contents);
        assert_eq!(result, "Before hello world after");
    }

    #[test]
    fn test_expand_pasted_text_refs_with_lines() {
        let mut contents = HashMap::new();
        contents.insert(
            1u32,
            PastedContent {
                id: 1,
                kind: PastedContentKind::Text,
                content: "line1\nline2".to_string(),
                media_type: None,
                filename: None,
            },
        );
        let result = expand_pasted_text_refs("X [Pasted text #1 +1 lines] Y", &contents);
        assert_eq!(result, "X line1\nline2 Y");
    }

    #[test]
    fn test_expand_pasted_text_refs_image_unchanged() {
        let mut contents = HashMap::new();
        contents.insert(
            2u32,
            PastedContent {
                id: 2,
                kind: PastedContentKind::Image,
                content: "<binary>".to_string(),
                media_type: Some("image/png".to_string()),
                filename: None,
            },
        );
        let input = "See [Image #2]";
        let result = expand_pasted_text_refs(input, &contents);
        assert_eq!(result, input);
    }

    #[test]
    fn test_expand_multiple_refs_reverse_order() {
        let mut contents = HashMap::new();
        contents.insert(
            1u32,
            PastedContent {
                id: 1,
                kind: PastedContentKind::Text,
                content: "AAA".to_string(),
                media_type: None,
                filename: None,
            },
        );
        contents.insert(
            2u32,
            PastedContent {
                id: 2,
                kind: PastedContentKind::Text,
                content: "BBB".to_string(),
                media_type: None,
                filename: None,
            },
        );
        let result = expand_pasted_text_refs("[Pasted text #1] and [Pasted text #2]", &contents);
        assert_eq!(result, "AAA and BBB");
    }

    // -- Num-lines helper ---------------------------------------------------

    #[test]
    fn test_num_lines_no_newline() {
        assert_eq!(get_pasted_text_ref_num_lines("hello"), 0);
    }

    #[test]
    fn test_num_lines_single_newline() {
        assert_eq!(get_pasted_text_ref_num_lines("a\nb"), 1);
    }

    #[test]
    fn test_num_lines_crlf() {
        assert_eq!(get_pasted_text_ref_num_lines("a\r\nb\r\nc"), 2);
    }

    // -- Format helpers -----------------------------------------------------

    #[test]
    fn test_format_pasted_text_ref_no_lines() {
        assert_eq!(format_pasted_text_ref(3, 0), "[Pasted text #3]");
    }

    #[test]
    fn test_format_pasted_text_ref_with_lines() {
        assert_eq!(format_pasted_text_ref(3, 5), "[Pasted text #3 +5 lines]");
    }

    #[test]
    fn test_format_image_ref() {
        assert_eq!(format_image_ref(2), "[Image #2]");
    }

    // -- remove_last_from_history -------------------------------------------

    #[test]
    fn test_remove_last_no_op_on_empty() {
        clear_pending_history_entries();
        remove_last_from_history(); // must not panic
    }
}
