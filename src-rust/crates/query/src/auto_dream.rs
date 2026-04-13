//! AutoDream: automatic memory consolidation daemon
//!
//! Background memory consolidation. Fires a consolidation prompt as a forked
//! subagent when the time gate passes AND enough sessions have accumulated.
//!
//! Gate order (cheapest first):
//!   1. Time:     hours since last_consolidated_at >= min_hours  (one stat)
//!   2. Sessions: transcript count with mtime > last_consolidated_at >= min_sessions
//!   3. Lock:     no other process mid-consolidation (stale after 1 hour)

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs;

// Scan throttle: when time-gate passes but session-gate doesn't, the lock
// mtime doesn't advance, so the time-gate keeps passing every turn.
pub const SESSION_SCAN_INTERVAL_SECS: u64 = 10 * 60; // 10 minutes

/// GrowthBook-sourced scheduling config (with defaults)
#[derive(Debug, Clone)]
pub struct AutoDreamConfig {
    /// Minimum hours between consolidations (default: 24)
    pub min_hours: f64,
    /// Minimum new-session count to trigger (default: 5)
    pub min_sessions: usize,
}

impl Default for AutoDreamConfig {
    fn default() -> Self {
        Self {
            min_hours: 24.0,
            min_sessions: 5,
        }
    }
}

/// Persisted state written to `.consolidation_state.json`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConsolidationState {
    /// Unix timestamp (seconds) of last successful consolidation.
    /// `None` means never consolidated.
    pub last_consolidated_at: Option<u64>,
    /// ETag / opaque lock token – reserved for future distributed locking.
    pub lock_etag: Option<String>,
}

/// Data returned by `AutoDream::maybe_trigger` when consolidation should proceed.
/// Pass this to `AutoDream::finish_consolidation` after the agent completes.
#[derive(Debug, Clone)]
pub struct ConsolidationTask {
    /// The consolidation prompt to send to the sub-agent.
    pub prompt: String,
    /// Working directory for the sub-agent.
    pub memory_dir: PathBuf,
    /// Path to the state file (written after consolidation completes).
    pub state_file: PathBuf,
    /// Path to the lock file (removed after consolidation completes).
    pub lock_file: PathBuf,
}

/// Core AutoDream logic; owns path state, delegates I/O to async methods.
pub struct AutoDream {
    config: AutoDreamConfig,
    memory_dir: PathBuf,
    conversations_dir: PathBuf,
    lock_file: PathBuf,
    state_file: PathBuf,
}

impl AutoDream {
    pub fn new(memory_dir: PathBuf, conversations_dir: PathBuf) -> Self {
        let lock_file = memory_dir.join(".consolidation_lock");
        let state_file = memory_dir.join(".consolidation_state.json");
        Self {
            config: AutoDreamConfig::default(),
            memory_dir,
            conversations_dir,
            lock_file,
            state_file,
        }
    }

    /// Construct with explicit config (for testing / feature-flag overrides).
    pub fn with_config(
        config: AutoDreamConfig,
        memory_dir: PathBuf,
        conversations_dir: PathBuf,
    ) -> Self {
        let lock_file = memory_dir.join(".consolidation_lock");
        let state_file = memory_dir.join(".consolidation_state.json");
        Self {
            config,
            memory_dir,
            conversations_dir,
            lock_file,
            state_file,
        }
    }

    // -------------------------------------------------------------------------
    // Gate checks
    // -------------------------------------------------------------------------

    /// Check all gates cheapest-first.  Returns `true` if consolidation should run.
    pub async fn should_consolidate(&self, state: &ConsolidationState) -> Result<bool> {
        // Gate 1: Time gate (cheapest – one arithmetic check)
        if !self.time_gate_passes(state) {
            return Ok(false);
        }

        // Gate 2: Session gate (directory scan)
        if !self.session_gate_passes(state).await? {
            return Ok(false);
        }

        // Gate 3: Lock gate (no other process mid-consolidation)
        if !self.lock_gate_passes().await? {
            return Ok(false);
        }

        Ok(true)
    }

    fn time_gate_passes(&self, state: &ConsolidationState) -> bool {
        let now_secs = now_secs();
        match state.last_consolidated_at {
            None => true, // Never consolidated → always pass
            Some(last) => {
                let hours_elapsed = (now_secs.saturating_sub(last)) as f64 / 3600.0;
                hours_elapsed >= self.config.min_hours
            }
        }
    }

    async fn session_gate_passes(&self, state: &ConsolidationState) -> Result<bool> {
        let last_secs = state.last_consolidated_at.unwrap_or(0);
        let mut count = 0usize;

        if !self.conversations_dir.exists() {
            return Ok(false);
        }

        let mut dir = fs::read_dir(&self.conversations_dir).await?;
        while let Some(entry) = dir.next_entry().await? {
            let metadata = entry.metadata().await?;
            if let Ok(mtime) = metadata.modified() {
                let mtime_secs = mtime
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or(Duration::ZERO)
                    .as_secs();
                if mtime_secs > last_secs {
                    count += 1;
                    if count >= self.config.min_sessions {
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
    }

    async fn lock_gate_passes(&self) -> Result<bool> {
        if !self.lock_file.exists() {
            return Ok(true);
        }

        // Stale lock (>1 hour) is treated as released
        match fs::metadata(&self.lock_file).await {
            Ok(meta) => {
                if let Ok(mtime) = meta.modified() {
                    let age_secs = SystemTime::now()
                        .duration_since(mtime)
                        .unwrap_or(Duration::ZERO)
                        .as_secs();
                    Ok(age_secs > 3600)
                } else {
                    // Cannot stat mtime → conservative: gate passes (treat as stale)
                    Ok(true)
                }
            }
            Err(_) => Ok(true), // File disappeared between exists() and metadata()
        }
    }

    // -------------------------------------------------------------------------
    // Lock management
    // -------------------------------------------------------------------------

    /// Write a timestamp to the lock file, creating it if absent.
    pub async fn acquire_lock(&self) -> Result<()> {
        if let Some(parent) = self.lock_file.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&self.lock_file, now_secs().to_string()).await?;
        Ok(())
    }

    /// Remove the lock file.  No-op if it doesn't exist.
    pub async fn release_lock(&self) -> Result<()> {
        if self.lock_file.exists() {
            fs::remove_file(&self.lock_file).await?;
        }
        Ok(())
    }

    // -------------------------------------------------------------------------
    // State persistence
    // -------------------------------------------------------------------------

    /// Stamp `last_consolidated_at = now` and persist.
    pub async fn update_state(&self, state: &mut ConsolidationState) -> Result<()> {
        state.last_consolidated_at = Some(now_secs());
        let json = serde_json::to_string_pretty(state)?;
        if let Some(parent) = self.state_file.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&self.state_file, json).await?;
        Ok(())
    }

    /// Load persisted state; returns `Default` on any error (missing file, parse failure).
    pub async fn load_state(&self) -> ConsolidationState {
        match fs::read_to_string(&self.state_file).await {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => ConsolidationState::default(),
        }
    }

    // -------------------------------------------------------------------------
    // High-level trigger
    // -------------------------------------------------------------------------

    /// Check all gates and, if they pass, acquire the lock and return the info
    /// needed to run the consolidation subagent.
    ///
    /// Returns `Ok(Some(task))` if consolidation should proceed (lock acquired),
    /// `Ok(None)` if gated out, or `Err` for hard I/O failures.
    ///
    /// The caller is responsible for actually running the agent and calling
    /// `finish_consolidation` when done.
    pub async fn maybe_trigger(&self) -> Result<Option<ConsolidationTask>> {
        let state = self.load_state().await;
        if !self.should_consolidate(&state).await? {
            return Ok(None);
        }
        self.acquire_lock().await?;
        Ok(Some(ConsolidationTask {
            prompt: self.consolidation_prompt(),
            memory_dir: self.memory_dir.clone(),
            state_file: self.state_file.clone(),
            lock_file: self.lock_file.clone(),
        }))
    }

    /// Persist the updated consolidation state and release the lock.
    /// Call this after the consolidation subagent completes (or fails).
    pub async fn finish_consolidation(task: &ConsolidationTask) {
        let updated = ConsolidationState {
            last_consolidated_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or(std::time::Duration::ZERO)
                    .as_secs(),
            ),
            lock_etag: None,
        };
        if let Ok(json) = serde_json::to_string_pretty(&updated) {
            if let Some(parent) = task.state_file.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&task.state_file, json).await;
        }
        if task.lock_file.exists() {
            let _ = tokio::fs::remove_file(&task.lock_file).await;
        }
    }

    // -------------------------------------------------------------------------
    // Prompt construction
    // -------------------------------------------------------------------------

    /// Build the consolidation prompt for the forked subagent.
    pub fn consolidation_prompt(&self) -> String {
        format!(
            r#"# Dream: Memory Consolidation

You are performing a dream — a reflective pass over your memory files. Synthesize what you have learned recently into durable, well-organized memories so that future sessions can orient quickly.

Memory directory: `{memory_dir}`

Session transcripts: `{conv_dir}` (large JSONL files — grep narrowly, do not read whole files)

---

## Phase 1 — Orient

- `ls` the memory directory to see what already exists
- Read `MEMORY.md` to understand the current index
- Skim existing topic files so you improve them rather than creating duplicates

## Phase 2 — Gather recent signal

Look for new information worth persisting:

1. **Daily logs** (`logs/YYYY/MM/YYYY-MM-DD.md`) if present
2. **Existing memories that drifted** — facts that contradict what you see now
3. **Transcript search** — grep narrowly for specific terms:
   `grep -rn "<narrow term>" {conv_dir}/ --include="*.jsonl" | tail -50`

Do not exhaustively read transcripts. Look only for things you already suspect matter.

## Phase 3 — Consolidate

For each thing worth remembering, write or update a memory file. Focus on:
- Merging new signal into existing topic files rather than creating near-duplicates
- Converting relative dates to absolute dates
- Deleting contradicted facts

## Phase 4 — Prune and index

Update `MEMORY.md` so it stays under 200 lines and ~25 KB. It is an **index**, not a dump.
Each entry: `- [Title](file.md) — one-line hook`

- Remove pointers to stale, wrong, or superseded memories
- Shorten verbose entries; move detail into topic files
- Add pointers to newly important memories
- Resolve contradictions

---

Return a brief summary of what you consolidated, updated, or pruned. If nothing changed, say so.

**Tool constraints for this run:** Use only read-only Bash commands (ls, find, grep, cat, stat, wc, head, tail). Anything that writes, redirects to a file, or modifies state will be denied.
"#,
            memory_dir = self.memory_dir.display(),
            conv_dir = self.conversations_dir.display(),
        )
    }
}

// -------------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_dream(tmp: &TempDir) -> AutoDream {
        let mem = tmp.path().join("memory");
        let conv = tmp.path().join("conversations");
        AutoDream::new(mem, conv)
    }

    // --- time_gate_passes ---

    #[test]
    fn test_time_gate_never_consolidated() {
        let tmp = TempDir::new().unwrap();
        let dream = make_dream(&tmp);
        let state = ConsolidationState::default();
        assert!(
            dream.time_gate_passes(&state),
            "no prior consolidation → gate passes"
        );
    }

    #[test]
    fn test_time_gate_recent_consolidation() {
        let tmp = TempDir::new().unwrap();
        let dream = AutoDream::with_config(
            AutoDreamConfig {
                min_hours: 24.0,
                min_sessions: 5,
            },
            tmp.path().join("memory"),
            tmp.path().join("conversations"),
        );
        let state = ConsolidationState {
            last_consolidated_at: Some(now_secs()), // just now
            lock_etag: None,
        };
        assert!(
            !dream.time_gate_passes(&state),
            "just consolidated → gate blocked"
        );
    }

    #[test]
    fn test_time_gate_old_consolidation() {
        let tmp = TempDir::new().unwrap();
        let dream = AutoDream::with_config(
            AutoDreamConfig {
                min_hours: 24.0,
                min_sessions: 5,
            },
            tmp.path().join("memory"),
            tmp.path().join("conversations"),
        );
        // 25 hours ago
        let old = now_secs().saturating_sub(25 * 3600);
        let state = ConsolidationState {
            last_consolidated_at: Some(old),
            lock_etag: None,
        };
        assert!(
            dream.time_gate_passes(&state),
            "consolidated 25h ago → gate passes"
        );
    }

    // --- lock_gate_passes (sync-friendly via tokio::test) ---

    #[tokio::test]
    async fn test_lock_gate_no_lock_file() {
        let tmp = TempDir::new().unwrap();
        let dream = make_dream(&tmp);
        assert!(dream.lock_gate_passes().await.unwrap());
    }

    #[tokio::test]
    async fn test_lock_gate_fresh_lock_blocks() {
        let tmp = TempDir::new().unwrap();
        let dream = make_dream(&tmp);
        std::fs::create_dir_all(&dream.memory_dir).unwrap();
        std::fs::write(&dream.lock_file, "12345").unwrap();
        // Fresh file → gate blocked
        assert!(!dream.lock_gate_passes().await.unwrap());
    }

    // --- consolidation_prompt sanity ---

    #[test]
    fn test_consolidation_prompt_contains_paths() {
        let tmp = TempDir::new().unwrap();
        let dream = make_dream(&tmp);
        let prompt = dream.consolidation_prompt();
        assert!(prompt.contains("MEMORY.md"));
        assert!(prompt.contains("Memory Consolidation"));
        assert!(prompt.contains("Phase 1"));
        assert!(prompt.contains("Phase 4"));
    }

    // --- update_state / load_state round-trip ---

    #[tokio::test]
    async fn test_state_round_trip() {
        let tmp = TempDir::new().unwrap();
        let dream = make_dream(&tmp);
        std::fs::create_dir_all(&dream.memory_dir).unwrap();

        let mut state = ConsolidationState::default();
        dream.update_state(&mut state).await.unwrap();

        assert!(state.last_consolidated_at.is_some());
        let loaded = dream.load_state().await;
        assert_eq!(loaded.last_consolidated_at, state.last_consolidated_at);
    }

    // --- acquire_lock / release_lock ---

    #[tokio::test]
    async fn test_acquire_release_lock() {
        let tmp = TempDir::new().unwrap();
        let dream = make_dream(&tmp);

        dream.acquire_lock().await.unwrap();
        assert!(dream.lock_file.exists());

        dream.release_lock().await.unwrap();
        assert!(!dream.lock_file.exists());
    }
}
