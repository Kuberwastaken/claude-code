// command_queue.rs — T1-4: Command Queue Draining
//
// A priority queue shared between the TUI input thread and the query loop.
// Commands are drained at the start of each turn, before the API call.
//
// Mirrors the TypeScript `messageQueueManager.js` behaviour.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Priority
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPriority {
    Interrupt = 3,
    High = 2,
    Normal = 1,
    Low = 0,
}

impl PartialOrd for CommandPriority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CommandPriority {
    fn cmp(&self, other: &Self) -> Ordering {
        (*self as u8).cmp(&(*other as u8))
    }
}

// ---------------------------------------------------------------------------
// Command variants
// ---------------------------------------------------------------------------

/// Commands that can be enqueued by the TUI or other components.
#[derive(Debug, Clone)]
pub enum QueuedCommand {
    /// /compact — compress conversation context
    Compact,
    /// /clear — reset conversation history
    Clear,
    /// Change the active model
    SetModel(String),
    /// Inject a plain user message into the conversation
    InjectUserMessage(String),
    /// Inject a system-level message (sent as a user message with [System] prefix)
    InjectSystemMessage(String),
    /// Trigger a named skill
    TriggerSkill(String),
}

// ---------------------------------------------------------------------------
// Internal heap entry
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct QueueEntry {
    command: QueuedCommand,
    priority: CommandPriority,
    /// Milliseconds since UNIX epoch — used as tie-breaker (older = higher priority).
    timestamp: u64,
}

impl PartialEq for QueueEntry {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.timestamp == other.timestamp
    }
}

impl Eq for QueueEntry {}

impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority wins; among equal priorities, the older entry wins.
        self.priority
            .cmp(&other.priority)
            .then(other.timestamp.cmp(&self.timestamp))
    }
}

// ---------------------------------------------------------------------------
// CommandQueue
// ---------------------------------------------------------------------------

/// Thread-safe priority queue of commands.
///
/// Cloning the handle yields a second handle to the *same* queue (Arc semantics).
#[derive(Debug, Clone)]
pub struct CommandQueue(Arc<Mutex<BinaryHeap<QueueEntry>>>);

impl CommandQueue {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(BinaryHeap::new())))
    }

    /// Push a command with the given priority.
    pub fn push(&self, command: QueuedCommand, priority: CommandPriority) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut heap = self.0.lock().unwrap();
        heap.push(QueueEntry {
            command,
            priority,
            timestamp: ts,
        });
    }

    /// Drain all pending commands in priority order (highest first).
    pub fn drain(&self) -> Vec<QueuedCommand> {
        let mut heap = self.0.lock().unwrap();
        let mut out = Vec::with_capacity(heap.len());
        while let Some(entry) = heap.pop() {
            out.push(entry.command);
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.0.lock().unwrap().is_empty()
    }
}

impl Default for CommandQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// drain_command_queue
// ---------------------------------------------------------------------------

/// Consume all pending commands and convert them to `Message`s that can be
/// prepended to the conversation before the next API call.
///
/// Commands that are handled purely by the TUI/app layer (Compact, Clear,
/// SetModel, TriggerSkill) are silently dropped here — the query loop does
/// not need to act on them directly.
pub fn drain_command_queue(queue: &CommandQueue) -> Vec<claurst_core::types::Message> {
    use claurst_core::types::Message;

    let commands = queue.drain();
    let mut messages = Vec::new();

    for cmd in commands {
        match cmd {
            QueuedCommand::InjectUserMessage(text) => {
                messages.push(Message::user(text));
            }
            QueuedCommand::InjectSystemMessage(text) => {
                // System messages are injected as user turns with a [System]
                // prefix so they remain compatible with the Anthropic Messages API.
                messages.push(Message::user(format!("[System]: {text}")));
            }
            QueuedCommand::SetModel(_)
            | QueuedCommand::Compact
            | QueuedCommand::Clear
            | QueuedCommand::TriggerSkill(_) => {
                // Handled at the TUI/app layer; the query loop ignores them.
            }
        }
    }

    messages
}
