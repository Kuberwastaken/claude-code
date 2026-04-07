//! Status notice definitions and priority ordering.
//! Mirrors src/utils/statusNotices.ts

use serde::{Deserialize, Serialize};

/// Priority ordering for status notices (highest priority shown first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NoticePriority {
    Critical = 3,
    High = 2,
    Normal = 1,
    Low = 0,
}

/// A status notice to display in the TUI status line or banner area.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusNotice {
    pub id: String,
    pub message: String,
    pub priority: NoticePriority,
    /// If true, the notice dismisses itself after one turn.
    pub ephemeral: bool,
    /// If set, the notice expires at this Unix timestamp (ms).
    pub expires_at_ms: Option<u64>,
}

impl StatusNotice {
    pub fn new(
        id: impl Into<String>,
        message: impl Into<String>,
        priority: NoticePriority,
    ) -> Self {
        Self {
            id: id.into(),
            message: message.into(),
            priority,
            ephemeral: false,
            expires_at_ms: None,
        }
    }

    pub fn ephemeral(mut self) -> Self {
        self.ephemeral = true;
        self
    }

    pub fn expires_in_ms(mut self, ms: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.expires_at_ms = Some(now + ms);
        self
    }

    pub fn is_expired(&self) -> bool {
        if let Some(exp) = self.expires_at_ms {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            now >= exp
        } else {
            false
        }
    }
}

/// Well-known notice IDs used throughout the codebase.
pub mod notice_ids {
    pub const COMPACT_WARNING: &str = "compact-warning";
    pub const COMPACT_CRITICAL: &str = "compact-critical";
    pub const RATE_LIMIT: &str = "rate-limit";
    pub const BRIDGE_DISCONNECTED: &str = "bridge-disconnected";
    pub const HOOK_ERROR: &str = "hook-error";
    pub const MAX_TOKENS_HIT: &str = "max-tokens-hit";
    pub const NEW_VERSION: &str = "new-version";
}

/// Build the standard compact warning notice.
pub fn compact_warning_notice(fill_pct: f64) -> StatusNotice {
    if fill_pct >= 0.95 {
        StatusNotice::new(
            notice_ids::COMPACT_CRITICAL,
            format!(
                "Context {:.0}% full — run /compact now to avoid data loss",
                fill_pct * 100.0
            ),
            NoticePriority::Critical,
        )
    } else {
        StatusNotice::new(
            notice_ids::COMPACT_WARNING,
            format!(
                "Context {:.0}% full — consider running /compact",
                fill_pct * 100.0
            ),
            NoticePriority::High,
        )
    }
}

/// Sort notices by priority (highest first), then by ID for stability.
pub fn sort_notices(notices: &mut Vec<StatusNotice>) {
    notices.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));
}
