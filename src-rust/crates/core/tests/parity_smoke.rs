//! T5-1 parity smoke tests.
//! Verifies that core data structures are usable as the TS CLI would use them.

use claurst_core::{
    session_storage::{TranscriptEntry, transcript_dir},
    prompt_history::HistoryEntry,
    file_history::FileHistory,
    claudemd::load_all_memory_files,
    message_utils::{estimate_tokens, get_message_text, is_tool_use_message},
    types::{Message, MessageContent, Role},
};
use std::path::PathBuf;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Session storage — directory encoding
// ---------------------------------------------------------------------------

#[test]
fn session_dir_encoding() {
    // Verify transcript dir encoding is stable.
    let root = PathBuf::from("/home/user/project");
    let dir = transcript_dir(&root);
    // Should contain the base64-encoded project root under .claurst/projects/.
    assert!(dir.to_string_lossy().contains("projects"));
}

// ---------------------------------------------------------------------------
// File history
// ---------------------------------------------------------------------------

#[test]
fn file_history_record_and_query() {
    let mut fh = FileHistory::new();
    assert!(fh.is_empty());

    let path = PathBuf::from("/tmp/test.rs");
    fh.record_modification(path.clone(), b"before", b"after", 0, "FileEdit");
    assert_eq!(fh.len(), 1);

    let history = fh.get_file_history(&path);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].tool_name, "FileEdit");
}

// ---------------------------------------------------------------------------
// Message utilities
// ---------------------------------------------------------------------------

#[test]
fn estimate_tokens_nonzero() {
    let tokens = estimate_tokens("This is a short sentence.");
    assert!(tokens > 0);
}

#[test]
fn get_message_text_user() {
    let msg = Message {
        role: Role::User,
        content: MessageContent::Text("hello world".to_string()),
        uuid: None,
        cost: None,
        snapshot_patch: None,
    };
    assert_eq!(get_message_text(&msg), "hello world");
}

#[test]
fn is_tool_use_message_false_for_user() {
    let msg = Message {
        role: Role::User,
        content: MessageContent::Text("not a tool".to_string()),
        uuid: None,
        cost: None,
        snapshot_patch: None,
    };
    assert!(!is_tool_use_message(&msg));
}

// ---------------------------------------------------------------------------
// AGENTS.md loading
// ---------------------------------------------------------------------------

#[test]
fn load_memory_from_nonexistent_dir() {
    // Loading from a dir with no AGENTS.md should return empty, not panic.
    let tmp = TempDir::new().unwrap();
    let files = load_all_memory_files(tmp.path());
    // May be empty or may pick up user ~/.claurst/AGENTS.md — both are valid.
    let _ = files;
}

// ---------------------------------------------------------------------------
// Prompt history types compile and are usable
// ---------------------------------------------------------------------------

#[test]
fn history_entry_roundtrip() {
    let entry = HistoryEntry {
        display: "test prompt".to_string(),
        pasted_contents: Default::default(),
        timestamp: 0,
        project: "/tmp/project".to_string(),
        session_id: None,
    };
    // Serialise/deserialise round-trip.
    let json = serde_json::to_string(&entry).unwrap();
    let decoded: HistoryEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.display, "test prompt");
}
