//! Complete PromptInput — multi-line text editor for the TUI.
//! Mirrors src/components/PromptInput/ (21 files) and src/vim/ (5 files).
//!
//! Features:
//! - Multi-line editing (Shift+Enter for newlines)
//! - Vim Normal/Insert/Visual modes
//! - History navigation (↑↓ through history.jsonl)
//! - Slash command typeahead
//! - Paste handling (large pastes → placeholder)
//! - Character count + token estimate

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

const CLAUDE_ORANGE: Color = Color::Rgb(233, 30, 99);
const PROMPT_POINTER: &str = "\u{276f}";

// ---------------------------------------------------------------------------
// Vim mode
// ---------------------------------------------------------------------------

/// Vim editor mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimMode {
    #[default]
    Insert,
    Normal,
    Visual,
    /// Linewise visual selection (V).
    VisualLine,
    /// Block visual selection (Ctrl+V).
    VisualBlock,
    /// Command-line mode (:).
    Command,
    /// In-prompt forward search (/).
    Search,
}

impl VimMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Insert => "INSERT",
            Self::Normal => "NORMAL",
            Self::Visual => "VISUAL",
            Self::VisualLine => "VISUAL LINE",
            Self::VisualBlock => "VISUAL BLOCK",
            Self::Command => "COMMAND",
            Self::Search => "SEARCH",
        }
    }

    pub fn color(&self) -> Color {
        match self {
            Self::Insert => Color::Blue,
            Self::Normal => Color::Green,
            Self::Visual | Self::VisualLine | Self::VisualBlock => Color::Magenta,
            Self::Command | Self::Search => Color::Cyan,
        }
    }
}

// ---------------------------------------------------------------------------
// Extended vim state types (full state machine)
// ---------------------------------------------------------------------------

/// Pending multi-key vim command state.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum VimPendingState {
    #[default]
    None,
    /// Accumulating count digits before a command (e.g. `3` before `w`).
    Count { digits: String },
    /// Received `g`, waiting for second key.
    G { count: usize },
    /// Received operator (d/c/y), waiting for motion.
    Operator { op: VimOperator, count: usize },
    /// Received operator then additional count digits.
    OperatorCount { op: VimOperator, count: usize, digits: String },
    /// Received `dg`/`cg`/`yg`, waiting for second g key.
    OperatorG { op: VimOperator, count: usize },
    /// Received `f/F/t/T`, waiting for target char.
    Find { kind: VimFindKind, count: usize },
    /// Received `r`, waiting for replacement char.
    Replace { count: usize },
    /// Received `>` or `<`, waiting for second `>` or `<`.
    Indent { dir: char, count: usize },
    /// Received `"`, waiting for register name char.
    Register(char),
    /// After `"reg`, waiting for operator (y/d/p).
    RegisterOp(char),
    /// Received `m`, waiting for mark name char.
    Mark,
    /// Received `'`, waiting for mark name char for jump.
    JumpMark,
    /// Received `q`, waiting for register char to record into.
    MacroRecord,
    /// Received `@`, waiting for register char to replay.
    MacroReplay,
}

/// Vim operator type used with motion + operator combos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimOperator {
    Delete,
    Change,
    Yank,
    /// Uppercase region (gU).
    Uppercase,
    /// Lowercase region (gu).
    Lowercase,
}

/// Vim character-find direction and variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimFindKind {
    /// `f{c}` — forward, cursor lands on char
    F,
    /// `F{c}` — backward, cursor lands on char
    BigF,
    /// `t{c}` — forward, cursor stops before char
    T,
    /// `T{c}` — backward, cursor stops after char
    BigT,
}

/// Stores enough information to replay the last modifying vim command (`.`).
#[derive(Clone, Debug)]
pub enum DotRepeatAction {
    /// Insert text at current cursor (from i, a, A, o, O, s).
    Insert { text: String, mode_after_insert: bool },
    /// Simplified: re-delete the same number of chars.
    DeleteChars { count: usize },
    /// Change: delete + insert.
    Change { deleted: String, inserted: String },
    /// Replace char.
    ReplaceChar { ch: char },
}

// ---------------------------------------------------------------------------
// Motion helper functions (byte-safe, work on UTF-8 byte offsets)
// ---------------------------------------------------------------------------

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Convert a char-index within `text` to a byte offset.
fn char_idx_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

/// `w` — start of next word.
fn motion_w(text: &str, cursor: usize) -> usize {
    let rest = &text[cursor..];
    let chars: Vec<char> = rest.chars().collect();
    let n = chars.len();
    if n == 0 { return cursor; }
    let mut i = 0;
    if is_word_char(chars[0]) {
        while i < n && is_word_char(chars[i]) { i += 1; }
    } else if !chars[0].is_whitespace() {
        while i < n && !is_word_char(chars[i]) && !chars[i].is_whitespace() { i += 1; }
    }
    while i < n && chars[i].is_whitespace() { i += 1; }
    cursor + char_idx_to_byte(rest, i)
}

/// `b` — start of previous word.
fn motion_b(text: &str, cursor: usize) -> usize {
    if cursor == 0 { return 0; }
    let before = &text[..cursor];
    let chars: Vec<char> = before.chars().collect();
    let n = chars.len();
    if n == 0 { return 0; }
    let mut i = n;
    while i > 0 && chars[i - 1].is_whitespace() { i -= 1; }
    if i == 0 { return 0; }
    if is_word_char(chars[i - 1]) {
        while i > 0 && is_word_char(chars[i - 1]) { i -= 1; }
    } else {
        while i > 0 && !is_word_char(chars[i - 1]) && !chars[i - 1].is_whitespace() { i -= 1; }
    }
    char_idx_to_byte(before, i)
}

/// `e` — end of current/next word.
fn motion_e(text: &str, cursor: usize) -> usize {
    let chars: Vec<(usize, char)> = text[cursor..]
        .char_indices()
        .map(|(b, c)| (cursor + b, c))
        .collect();
    let n = chars.len();
    if n == 0 { return cursor; }
    let at_end = n == 1
        || chars[1].1.is_whitespace()
        || is_word_char(chars[0].1) != is_word_char(chars[1].1);
    let mut i = 0;
    if at_end {
        i = 1;
        while i < n && chars[i].1.is_whitespace() { i += 1; }
    }
    if i >= n { return cursor; }
    let wc = is_word_char(chars[i].1);
    while i + 1 < n && !chars[i + 1].1.is_whitespace() && is_word_char(chars[i + 1].1) == wc {
        i += 1;
    }
    chars[i].0
}

/// `W` — start of next WORD (any non-whitespace run).
#[allow(non_snake_case)]
fn motion_W(text: &str, cursor: usize) -> usize {
    let rest = &text[cursor..];
    let chars: Vec<char> = rest.chars().collect();
    let n = chars.len();
    if n == 0 { return cursor; }
    let mut i = 0;
    while i < n && !chars[i].is_whitespace() { i += 1; }
    while i < n && chars[i].is_whitespace() { i += 1; }
    cursor + char_idx_to_byte(rest, i)
}

/// `B` — start of previous WORD.
#[allow(non_snake_case)]
fn motion_B(text: &str, cursor: usize) -> usize {
    if cursor == 0 { return 0; }
    let before = &text[..cursor];
    let chars: Vec<char> = before.chars().collect();
    let n = chars.len();
    let mut i = n;
    while i > 0 && chars[i - 1].is_whitespace() { i -= 1; }
    while i > 0 && !chars[i - 1].is_whitespace() { i -= 1; }
    char_idx_to_byte(before, i)
}

/// `E` — end of current/next WORD.
#[allow(non_snake_case)]
fn motion_E(text: &str, cursor: usize) -> usize {
    let chars: Vec<(usize, char)> = text[cursor..]
        .char_indices()
        .map(|(b, c)| (cursor + b, c))
        .collect();
    let n = chars.len();
    if n == 0 { return cursor; }
    let at_end = n == 1 || chars[1].1.is_whitespace();
    let mut i = 0;
    if at_end {
        i = 1;
        while i < n && chars[i].1.is_whitespace() { i += 1; }
    }
    if i >= n { return cursor; }
    while i + 1 < n && !chars[i + 1].1.is_whitespace() { i += 1; }
    chars[i].0
}

/// `^` — first non-blank character on the current line.
fn motion_first_nonblank(text: &str, cursor: usize) -> usize {
    let line_start = text[..cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let rest = &text[line_start..];
    let skip_bytes = rest
        .char_indices()
        .take_while(|(_, c)| *c == ' ' || *c == '\t')
        .last()
        .map(|(b, c)| b + c.len_utf8())
        .unwrap_or(0);
    line_start + skip_bytes
}

/// `G` — first char of the last line.
#[allow(non_snake_case)]
fn motion_G(text: &str) -> usize {
    text.rfind('\n').map(|p| p + 1).unwrap_or(0)
}

/// `gg` / line-N — go to start of line `line_num` (1-indexed; 0 or 1 → start of text).
fn motion_gg(text: &str, line_num: usize) -> usize {
    if line_num <= 1 { return 0; }
    let mut line = 1usize;
    for (b, c) in text.char_indices() {
        if c == '\n' {
            line += 1;
            if line == line_num {
                return b + 1;
            }
        }
    }
    text.rfind('\n').map(|p| p + 1).unwrap_or(0)
}

/// `f/F/t/T{char}` — find character in text. Returns new cursor byte offset.
fn motion_find_char(
    text: &str,
    cursor: usize,
    target: char,
    kind: VimFindKind,
    count: usize,
) -> Option<usize> {
    match kind {
        VimFindKind::F | VimFindKind::T => {
            let search_start = text[cursor..].char_indices().nth(1).map(|(b, _)| cursor + b)?;
            let mut hits = 0usize;
            for (b, c) in text[search_start..].char_indices() {
                if c == target {
                    hits += 1;
                    if hits == count {
                        let pos = search_start + b;
                        if matches!(kind, VimFindKind::T) {
                            return text[cursor..pos]
                                .char_indices()
                                .last()
                                .map(|(lb, _)| cursor + lb);
                        }
                        return Some(pos);
                    }
                }
            }
            None
        }
        VimFindKind::BigF | VimFindKind::BigT => {
            let before = &text[..cursor];
            let mut hits = 0usize;
            for (b, c) in before.char_indices().rev() {
                if c == target {
                    hits += 1;
                    if hits == count {
                        if matches!(kind, VimFindKind::BigT) {
                            return text[b..].char_indices().nth(1).map(|(nb, _)| b + nb).or(Some(cursor));
                        }
                        return Some(b);
                    }
                }
            }
            None
        }
    }
}

/// Convert text region to uppercase.
fn uppercase_region(text: &str) -> String {
    text.chars().map(|c| {
        c.to_uppercase().next().unwrap_or(c)
    }).collect()
}

/// Convert text region to lowercase.
fn lowercase_region(text: &str) -> String {
    text.chars().map(|c| {
        c.to_lowercase().next().unwrap_or(c)
    }).collect()
}

/// Apply an operator (d/c/y/gU/gu) to the range [from, to) in text.
/// Returns `(new_text, new_cursor)`. For Change, sets mode to Insert.
fn apply_operator_range(
    op: VimOperator,
    text: &str,
    from: usize,
    to: usize,
    yank_buf: &mut String,
    mode: &mut VimMode,
) -> (String, usize) {
    let to = to.min(text.len());
    let from = from.min(to);
    let selected = &text[from..to];
    *yank_buf = selected.to_string();
    match op {
        VimOperator::Yank => (text.to_string(), from),
        VimOperator::Delete => {
            let new_text = format!("{}{}", &text[..from], &text[to..]);
            let new_cursor = from.min(new_text.len().saturating_sub(if new_text.is_empty() { 0 } else { 1 }));
            (new_text, new_cursor)
        }
        VimOperator::Change => {
            let new_text = format!("{}{}", &text[..from], &text[to..]);
            *mode = VimMode::Insert;
            (new_text, from)
        }
        VimOperator::Uppercase => {
            let upper = uppercase_region(selected);
            let new_text = format!("{}{}{}", &text[..from], upper, &text[to..]);
            (new_text, from)
        }
        VimOperator::Lowercase => {
            let lower = lowercase_region(selected);
            let new_text = format!("{}{}{}", &text[..from], lower, &text[to..]);
            (new_text, from)
        }
    }
}

// ---------------------------------------------------------------------------
// Full vim key handler (state machine)
// ---------------------------------------------------------------------------

/// Process a single key press in vim mode.
/// Returns `true` when text was modified (caller should push undo snapshot).
pub fn apply_vim_key(
    mode: &mut VimMode,
    text: &mut String,
    cursor: &mut usize,
    key: &str,
    yank_buf: &mut String,
    pending: &mut VimPendingState,
    last_find: &mut Option<(VimFindKind, char)>,
) -> bool {
    // Escape always cancels pending state and returns to Normal
    if key == "Escape" {
        *mode = VimMode::Normal;
        *pending = VimPendingState::None;
        return false;
    }

    match std::mem::replace(pending, VimPendingState::None) {
        VimPendingState::None => {
            vim_idle(mode, text, cursor, key, yank_buf, pending, last_find)
        }
        VimPendingState::Count { digits } => {
            vim_count(mode, text, cursor, key, yank_buf, pending, last_find, digits)
        }
        VimPendingState::G { count } => {
            vim_g(text, cursor, key, pending, count)
        }
        VimPendingState::Operator { op, count } => {
            vim_operator(mode, text, cursor, key, yank_buf, pending, last_find, op, count)
        }
        VimPendingState::OperatorCount { op, count, digits } => {
            vim_operator_count(mode, text, cursor, key, yank_buf, pending, last_find, op, count, digits)
        }
        VimPendingState::OperatorG { op, count } => {
            vim_operator_g(mode, text, cursor, key, yank_buf, op, count)
        }
        VimPendingState::Find { kind, count } => {
            if key.len() == 1 {
                let c = key.chars().next().unwrap();
                if let Some(new_pos) = motion_find_char(text, *cursor, c, kind, count) {
                    *cursor = new_pos;
                    *last_find = Some((kind, c));
                }
            }
            false
        }
        VimPendingState::Replace { count } => {
            if key.len() == 1 {
                let c = key.chars().next().unwrap();
                let mut modified = false;
                let mut pos = *cursor;
                for _ in 0..count.max(1) {
                    if pos >= text.len() { break; }
                    let clen = text[pos..].chars().next().map(|ch| ch.len_utf8()).unwrap_or(1);
                    text.replace_range(pos..pos + clen, &c.to_string());
                    pos += c.len_utf8();
                    modified = true;
                }
                *cursor = (*cursor).min(text.len().saturating_sub(if text.is_empty() { 0 } else { 1 }));
                modified
            } else {
                false
            }
        }
        VimPendingState::Indent { dir, count } => {
            if key == dir.to_string().as_str() {
                let indent = "  ";
                let current_line = text[..*cursor].chars().filter(|&c| c == '\n').count();
                let mut new_lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
                for i in 0..count.max(1) {
                    let idx = current_line + i;
                    if idx >= new_lines.len() { break; }
                    if dir == '>' {
                        new_lines[idx] = format!("{}{}", indent, new_lines[idx]);
                    } else if new_lines[idx].starts_with(indent) {
                        new_lines[idx] = new_lines[idx][indent.len()..].to_string();
                    } else {
                        let trimmed = new_lines[idx].trim_start_matches('\t').trim_start_matches(' ');
                        new_lines[idx] = trimmed.to_string();
                    }
                }
                *text = new_lines.join("\n");
                *cursor = (*cursor).min(text.len());
                true
            } else {
                false
            }
        }
        // These pending states are fully handled in PromptInputState::vim_command
        // before apply_vim_key is called, but we need arms for exhaustiveness.
        VimPendingState::Register(_)
        | VimPendingState::RegisterOp(_)
        | VimPendingState::Mark
        | VimPendingState::JumpMark
        | VimPendingState::MacroRecord
        | VimPendingState::MacroReplay => false,
    }
}

fn vim_idle(
    mode: &mut VimMode,
    text: &mut String,
    cursor: &mut usize,
    key: &str,
    yank_buf: &mut String,
    pending: &mut VimPendingState,
    last_find: &mut Option<(VimFindKind, char)>,
) -> bool {
    // Count prefix (1-9 only; 0 is the line-start motion)
    if key.len() == 1 {
        let ch = key.chars().next().unwrap();
        if ch.is_ascii_digit() && ch != '0' {
            *pending = VimPendingState::Count { digits: key.to_string() };
            return false;
        }
    }
    vim_normal(mode, text, cursor, key, yank_buf, pending, last_find, 1)
}

fn vim_count(
    mode: &mut VimMode,
    text: &mut String,
    cursor: &mut usize,
    key: &str,
    yank_buf: &mut String,
    pending: &mut VimPendingState,
    last_find: &mut Option<(VimFindKind, char)>,
    digits: String,
) -> bool {
    if key.len() == 1 && key.chars().next().unwrap().is_ascii_digit() {
        let new_digits = format!("{}{}", digits, key);
        let count: usize = new_digits.parse().unwrap_or(10000).min(10000);
        *pending = VimPendingState::Count { digits: count.to_string() };
        return false;
    }
    let count: usize = digits.parse().unwrap_or(1);
    vim_normal(mode, text, cursor, key, yank_buf, pending, last_find, count)
}

#[allow(clippy::too_many_arguments)]
fn vim_normal(
    mode: &mut VimMode,
    text: &mut String,
    cursor: &mut usize,
    key: &str,
    yank_buf: &mut String,
    pending: &mut VimPendingState,
    last_find: &mut Option<(VimFindKind, char)>,
    count: usize,
) -> bool {
    let n = count.max(1);
    match key {
        // ---- Mode transitions ----
        "i" => { *mode = VimMode::Insert; false }
        "a" => {
            *mode = VimMode::Insert;
            if *cursor < text.len() {
                *cursor = text[*cursor..].char_indices().nth(1).map(|(b, _)| *cursor + b).unwrap_or(text.len());
            }
            false
        }
        "I" => { *mode = VimMode::Insert; *cursor = motion_first_nonblank(text, *cursor); false }
        "A" => {
            *mode = VimMode::Insert;
            *cursor = text[*cursor..].find('\n').map(|p| *cursor + p).unwrap_or(text.len());
            false
        }
        "v" => { *mode = VimMode::Visual; false }
        // ---- Simple motions ----
        "h" => {
            for _ in 0..n {
                if *cursor > 0 {
                    let prev = text[..*cursor].char_indices().last().map(|(b, _)| b).unwrap_or(0);
                    *cursor = prev;
                }
            }
            false
        }
        "l" => {
            for _ in 0..n {
                if *cursor < text.len() {
                    *cursor = text[*cursor..].char_indices().nth(1).map(|(b, _)| *cursor + b).unwrap_or(text.len());
                }
            }
            false
        }
        "0" => { *cursor = text[..*cursor].rfind('\n').map(|p| p + 1).unwrap_or(0); false }
        "^" => { *cursor = motion_first_nonblank(text, *cursor); false }
        "$" => { *cursor = text[*cursor..].find('\n').map(|p| *cursor + p).unwrap_or(text.len()); false }
        "w" => { for _ in 0..n { *cursor = motion_w(text, *cursor); } false }
        "b" => { for _ in 0..n { *cursor = motion_b(text, *cursor); } false }
        "e" => { for _ in 0..n { *cursor = motion_e(text, *cursor); } false }
        "W" => { for _ in 0..n { *cursor = motion_W(text, *cursor); } false }
        "B" => { for _ in 0..n { *cursor = motion_B(text, *cursor); } false }
        "E" => { for _ in 0..n { *cursor = motion_E(text, *cursor); } false }
        "G" => {
            *cursor = if n == 1 { motion_G(text) } else { motion_gg(text, n) };
            false
        }
        "g" => { *pending = VimPendingState::G { count: n }; false }
        // ---- Find motions ----
        "f" => { *pending = VimPendingState::Find { kind: VimFindKind::F, count: n }; false }
        "F" => { *pending = VimPendingState::Find { kind: VimFindKind::BigF, count: n }; false }
        "t" => { *pending = VimPendingState::Find { kind: VimFindKind::T, count: n }; false }
        "T" => { *pending = VimPendingState::Find { kind: VimFindKind::BigT, count: n }; false }
        ";" => {
            if let Some((kind, c)) = *last_find {
                if let Some(pos) = motion_find_char(text, *cursor, c, kind, n) { *cursor = pos; }
            }
            false
        }
        "," => {
            if let Some((kind, c)) = *last_find {
                let rev = match kind {
                    VimFindKind::F => VimFindKind::BigF, VimFindKind::BigF => VimFindKind::F,
                    VimFindKind::T => VimFindKind::BigT, VimFindKind::BigT => VimFindKind::T,
                };
                if let Some(pos) = motion_find_char(text, *cursor, c, rev, n) { *cursor = pos; }
            }
            false
        }
        // ---- Operators ----
        "d" => { *pending = VimPendingState::Operator { op: VimOperator::Delete, count: n }; false }
        "c" => { *pending = VimPendingState::Operator { op: VimOperator::Change, count: n }; false }
        "y" => { *pending = VimPendingState::Operator { op: VimOperator::Yank, count: n }; false }
        // ---- Single-char delete/change shortcuts ----
        "x" => {
            if *cursor < text.len() {
                let clen = text[*cursor..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                *yank_buf = text[*cursor..*cursor + clen].to_string();
                text.drain(*cursor..*cursor + clen);
                *cursor = (*cursor).min(text.len().saturating_sub(if text.is_empty() { 0 } else { 1 }));
                return true;
            }
            false
        }
        "X" => {
            if *cursor > 0 {
                let prev = text[..*cursor].char_indices().last().map(|(b, _)| b).unwrap_or(0);
                *yank_buf = text[prev..*cursor].to_string();
                text.drain(prev..*cursor);
                *cursor = prev;
                return true;
            }
            false
        }
        "D" => {
            let end = text[*cursor..].find('\n').map(|p| *cursor + p).unwrap_or(text.len());
            if end > *cursor {
                *yank_buf = text[*cursor..end].to_string();
                text.drain(*cursor..end);
                return true;
            }
            false
        }
        "C" => {
            let end = text[*cursor..].find('\n').map(|p| *cursor + p).unwrap_or(text.len());
            *yank_buf = text[*cursor..end].to_string();
            text.drain(*cursor..end);
            *mode = VimMode::Insert;
            true
        }
        "s" => {
            if *cursor < text.len() {
                let clen = text[*cursor..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                *yank_buf = text[*cursor..*cursor + clen].to_string();
                text.drain(*cursor..*cursor + clen);
                *mode = VimMode::Insert;
                return true;
            }
            false
        }
        "S" => {
            let ls = text[..*cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
            let le = text[*cursor..].find('\n').map(|p| *cursor + p).unwrap_or(text.len());
            *yank_buf = text[ls..le].to_string();
            text.drain(ls..le);
            *cursor = ls;
            *mode = VimMode::Insert;
            true
        }
        // ---- Yank shortcuts ----
        "Y" | "yy" => {
            let ls = text[..*cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
            let le = text[*cursor..].find('\n').map(|p| *cursor + p + 1).unwrap_or(text.len());
            *yank_buf = text[ls..le].to_string();
            false
        }
        // ---- Paste ----
        "p" => {
            if !yank_buf.is_empty() {
                let buf = yank_buf.clone();
                let insert_pos = if *cursor < text.len() {
                    text[*cursor..].char_indices().nth(1).map(|(b, _)| *cursor + b).unwrap_or(text.len())
                } else { text.len() };
                text.insert_str(insert_pos, &buf);
                *cursor = (insert_pos + buf.len()).saturating_sub(1);
                return true;
            }
            false
        }
        "P" => {
            if !yank_buf.is_empty() {
                let buf = yank_buf.clone();
                text.insert_str(*cursor, &buf);
                *cursor = (*cursor + buf.len()).saturating_sub(1);
                return true;
            }
            false
        }
        // ---- Replace ----
        "r" => { *pending = VimPendingState::Replace { count: n }; false }
        // ---- Toggle case ----
        "~" => {
            if *cursor < text.len() {
                let clen = text[*cursor..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                let old: String = text[*cursor..*cursor + clen].to_string();
                let new: String = old.chars().map(|c| {
                    if c.is_uppercase() { c.to_lowercase().next().unwrap_or(c) }
                    else { c.to_uppercase().next().unwrap_or(c) }
                }).collect();
                text.replace_range(*cursor..*cursor + clen, &new);
                if *cursor < text.len() {
                    *cursor = text[*cursor..].char_indices().nth(1).map(|(b, _)| *cursor + b).unwrap_or(text.len());
                }
                return true;
            }
            false
        }
        // ---- Indent ----
        ">" => { *pending = VimPendingState::Indent { dir: '>', count: n }; false }
        "<" => { *pending = VimPendingState::Indent { dir: '<', count: n }; false }
        // ---- Join lines ----
        "J" => {
            if let Some(nl_pos) = text[*cursor..].find('\n').map(|p| *cursor + p) {
                text.remove(nl_pos);
                if text.as_bytes().get(nl_pos) != Some(&b' ') {
                    text.insert(nl_pos, ' ');
                }
                return true;
            }
            false
        }
        // ---- Open line ----
        "o" => {
            let le = text[*cursor..].find('\n').map(|p| *cursor + p).unwrap_or(text.len());
            text.insert(le, '\n');
            *cursor = le + 1;
            *mode = VimMode::Insert;
            true
        }
        "O" => {
            let ls = text[..*cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
            text.insert(ls, '\n');
            *cursor = ls;
            *mode = VimMode::Insert;
            true
        }
        // ---- dd/yy (multi-char fallthrough from legacy apply_vim_command) ----
        "dd" => {
            let ls = text[..*cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
            let le = text[*cursor..].find('\n').map(|p| *cursor + p + 1).unwrap_or(text.len());
            *yank_buf = text[ls..le].to_string();
            text.drain(ls..le);
            *cursor = ls.min(text.len());
            true
        }
        // ---- Register, marks, macros — set pending; actual work done in vim_command ----
        "\"" => { *pending = VimPendingState::Register('\0'); false }
        "m" => { *pending = VimPendingState::Mark; false }
        "'" => { *pending = VimPendingState::JumpMark; false }
        "q" => { *pending = VimPendingState::MacroRecord; false }
        "@" => { *pending = VimPendingState::MacroReplay; false }
        _ => false,
    }
}

fn vim_g(
    text: &mut String,
    cursor: &mut usize,
    key: &str,
    pending: &mut VimPendingState,
    count: usize,
) -> bool {
    match key {
        "g" => { *cursor = if count > 1 { motion_gg(text, count) } else { 0 }; false }
        "e" => {
            // `ge` — end of previous word
            for _ in 0..count.max(1) {
                if *cursor == 0 { break; }
                let before = &text[..*cursor];
                let chars: Vec<char> = before.chars().collect();
                let n = chars.len();
                let mut i = n;
                while i > 0 && chars[i - 1].is_whitespace() { i -= 1; }
                if i == 0 { *cursor = 0; break; }
                let is_wc = is_word_char(chars[i - 1]);
                while i > 1 && is_word_char(chars[i - 2]) == is_wc && !chars[i - 2].is_whitespace() { i -= 1; }
                *cursor = char_idx_to_byte(before, i - 1);
            }
            false
        }
        "E" => {
            // `gE` — end of previous WORD
            for _ in 0..count.max(1) {
                if *cursor == 0 { break; }
                let before = &text[..*cursor];
                let chars: Vec<char> = before.chars().collect();
                let n = chars.len();
                let mut i = n;
                while i > 0 && chars[i - 1].is_whitespace() { i -= 1; }
                while i > 1 && !chars[i - 2].is_whitespace() { i -= 1; }
                *cursor = char_idx_to_byte(before, i - 1);
            }
            false
        }
        "U" => {
            // `gU` — start case conversion uppercase operator
            *pending = VimPendingState::Operator { op: VimOperator::Uppercase, count };
            false
        }
        "u" => {
            // `gu` — start case conversion lowercase operator
            *pending = VimPendingState::Operator { op: VimOperator::Lowercase, count };
            false
        }
        _ => { *pending = VimPendingState::None; false }
    }
}

#[allow(clippy::too_many_arguments)]
fn vim_operator(
    mode: &mut VimMode,
    text: &mut String,
    cursor: &mut usize,
    key: &str,
    yank_buf: &mut String,
    pending: &mut VimPendingState,
    _last_find: &mut Option<(VimFindKind, char)>,
    op: VimOperator,
    count: usize,
) -> bool {
    let op_char = match op {
        VimOperator::Delete => "d",
        VimOperator::Change => "c",
        VimOperator::Yank => "y",
        VimOperator::Uppercase => "U",
        VimOperator::Lowercase => "u",
    };
    // Doubled operator = line op (dd, cc, yy, gUU, guu)
    if key == op_char {
        let ls = text[..*cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let mut le = *cursor;
        for _ in 0..count.max(1) {
            match text[le..].find('\n') {
                Some(n) => le += n + 1,
                None => { le = text.len(); break; }
            }
        }
        let le = le.min(text.len());
        let selected = &text[ls..le];
        *yank_buf = selected.to_string();
        if op != VimOperator::Yank {
            let new_content = match op {
                VimOperator::Delete => String::new(),
                VimOperator::Change => {
                    *mode = VimMode::Insert;
                    String::new()
                }
                VimOperator::Uppercase => uppercase_region(selected),
                VimOperator::Lowercase => lowercase_region(selected),
                VimOperator::Yank => unreachable!(),
            };
            text.drain(ls..le);
            text.insert_str(ls, &new_content);
            *cursor = ls;
            return true;
        }
        return false;
    }
    // Count prefix after operator (e.g. d3w)
    if key.len() == 1 && key.chars().next().unwrap().is_ascii_digit() {
        *pending = VimPendingState::OperatorCount { op, count, digits: key.to_string() };
        return false;
    }
    // `g` prefix
    if key == "g" { *pending = VimPendingState::OperatorG { op, count }; return false; }
    // Simple motions
    let target = match key {
        "h" => { let mut p = *cursor; for _ in 0..count.max(1) { if p > 0 { p -= 1; } } p }
        "l" => { let mut p = *cursor; for _ in 0..count.max(1) { if p < text.len() { p = text[p..].char_indices().nth(1).map(|(b,_)| p+b).unwrap_or(text.len()); } } p }
        "w" => { let mut p = *cursor; for _ in 0..count.max(1) { p = motion_w(text, p); } p }
        "b" => { let mut p = *cursor; for _ in 0..count.max(1) { p = motion_b(text, p); } p }
        "e" => { let mut p = *cursor; for _ in 0..count.max(1) { p = motion_e(text, p); } p }
        "W" => { let mut p = *cursor; for _ in 0..count.max(1) { p = motion_W(text, p); } p }
        "B" => { let mut p = *cursor; for _ in 0..count.max(1) { p = motion_B(text, p); } p }
        "E" => { let mut p = *cursor; for _ in 0..count.max(1) { p = motion_E(text, p); } p }
        "0" => text[..*cursor].rfind('\n').map(|p| p+1).unwrap_or(0),
        "^" => motion_first_nonblank(text, *cursor),
        "$" => text[*cursor..].find('\n').map(|p| *cursor+p).unwrap_or(text.len()),
        "G" => if count == 1 { motion_G(text) } else { motion_gg(text, count) },
        _ => { return false; }
    };
    if target == *cursor { return false; }
    let (from, to) = if target < *cursor { (target, *cursor) } else { (*cursor, target) };
    // Inclusive adjustment for e, E, $
    let to_adj = if matches!(key, "e" | "E" | "$") {
        text[to..].char_indices().nth(1).map(|(b,_)| to+b).unwrap_or(text.len())
    } else { to };
    let (new_text, new_cursor) = apply_operator_range(op, text, from, to_adj, yank_buf, mode);
    *text = new_text;
    *cursor = new_cursor.min(text.len());
    op != VimOperator::Yank
}

#[allow(clippy::too_many_arguments)]
fn vim_operator_count(
    mode: &mut VimMode,
    text: &mut String,
    cursor: &mut usize,
    key: &str,
    yank_buf: &mut String,
    pending: &mut VimPendingState,
    last_find: &mut Option<(VimFindKind, char)>,
    op: VimOperator,
    count: usize,
    digits: String,
) -> bool {
    if key.len() == 1 && key.chars().next().unwrap().is_ascii_digit() {
        let new_digits = format!("{}{}", digits, key);
        let d: usize = new_digits.parse().unwrap_or(10000).min(10000);
        *pending = VimPendingState::OperatorCount { op, count, digits: d.to_string() };
        return false;
    }
    let motion_count: usize = digits.parse().unwrap_or(1);
    let effective = count.saturating_mul(motion_count).min(10000);
    *pending = VimPendingState::Operator { op, count: effective };
    vim_operator(mode, text, cursor, key, yank_buf, pending, last_find, op, effective)
}

fn vim_operator_g(
    mode: &mut VimMode,
    text: &mut String,
    cursor: &mut usize,
    key: &str,
    yank_buf: &mut String,
    op: VimOperator,
    count: usize,
) -> bool {
    match key {
        "g" => {
            let target = if count > 1 { motion_gg(text, count) } else { 0 };
            let (from, to) = (target.min(*cursor), target.max(*cursor));
            let to_le = text[to..].find('\n').map(|p| to+p+1).unwrap_or(text.len());
            let (new_text, new_cursor) = apply_operator_range(op, text, from, to_le, yank_buf, mode);
            *text = new_text;
            *cursor = new_cursor.min(text.len());
            op != VimOperator::Yank
        }
        _ => false,
    }
}

/// Apply a vim normal-mode motion/command to `text`/`cursor`.
/// Returns the new (text, cursor_pos) after the command.
/// Covers: h j k l w b e 0 $ i a I A dd yy x p
pub fn apply_vim_command(
    mode: &mut VimMode,
    text: &mut String,
    cursor: &mut usize,
    key: &str,
    yank_buf: &mut String,
) {
    match key {
        // Mode transitions
        "i" if *mode == VimMode::Normal => { *mode = VimMode::Insert; }
        "a" if *mode == VimMode::Normal => {
            *mode = VimMode::Insert;
            if *cursor < text.len() { *cursor += 1; }
        }
        "I" if *mode == VimMode::Normal => {
            *mode = VimMode::Insert;
            *cursor = 0;
        }
        "A" if *mode == VimMode::Normal => {
            *mode = VimMode::Insert;
            *cursor = text.len();
        }
        "Escape" => { *mode = VimMode::Normal; }
        // Normal mode motions
        "h" if *mode == VimMode::Normal => {
            *cursor = cursor.saturating_sub(1);
        }
        "l" if *mode == VimMode::Normal => {
            if *cursor < text.len() { *cursor += 1; }
        }
        "0" if *mode == VimMode::Normal => { *cursor = 0; }
        "$" if *mode == VimMode::Normal => { *cursor = text.len(); }
        "w" if *mode == VimMode::Normal => {
            // Move to start of next word
            let rest = &text[*cursor..];
            let skip_word = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').count();
            let skip_space = rest[skip_word..].chars().take_while(|c| c.is_whitespace()).count();
            *cursor = (*cursor + skip_word + skip_space).min(text.len());
        }
        "b" if *mode == VimMode::Normal => {
            // Move to start of previous word
            let before = &text[..*cursor];
            let skip_space = before.chars().rev().take_while(|c| c.is_whitespace()).count();
            let skip_word = before[..before.len() - skip_space].chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').count();
            *cursor = cursor.saturating_sub(skip_space + skip_word);
        }
        "x" if *mode == VimMode::Normal => {
            // Delete char under cursor
            if *cursor < text.len() {
                *yank_buf = text.chars().nth(*cursor).unwrap_or_default().to_string();
                text.remove(*cursor);
                if *cursor > 0 && *cursor >= text.len() { *cursor = text.len().saturating_sub(1); }
            }
        }
        "dd" if *mode == VimMode::Normal => {
            // Delete current line
            *yank_buf = text.clone();
            text.clear();
            *cursor = 0;
        }
        "yy" if *mode == VimMode::Normal => {
            *yank_buf = text.clone();
        }
        "p" if *mode == VimMode::Normal => {
            // Paste after cursor
            let insert_pos = (*cursor + 1).min(text.len());
            text.insert_str(insert_pos, yank_buf);
            *cursor = insert_pos + yank_buf.len();
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Typeahead / autocomplete
// ---------------------------------------------------------------------------

/// Typeahead source.
#[derive(Debug, Clone)]
pub enum TypeaheadSource {
    SlashCommand,
    FileRef,
    History,
}

/// A single typeahead suggestion.
#[derive(Debug, Clone)]
pub struct TypeaheadSuggestion {
    pub text: String,
    pub description: String,
    pub source: TypeaheadSource,
}

/// Compute typeahead suggestions for the current input.
pub fn compute_typeahead(
    input: &str,
    slash_commands: &[(&str, &str)],
) -> Vec<TypeaheadSuggestion> {
    let mut suggestions = Vec::new();

    if let Some(cmd_prefix) = input.strip_prefix('/') {
        let prefix_lower = cmd_prefix.to_lowercase();
        for (name, desc) in slash_commands {
            if name.to_lowercase().starts_with(&prefix_lower) {
                suggestions.push(TypeaheadSuggestion {
                    text: format!("/{}", name),
                    description: desc.to_string(),
                    source: TypeaheadSource::SlashCommand,
                });
            }
        }
    }

    suggestions
}

// ---------------------------------------------------------------------------
// Paste handling
// ---------------------------------------------------------------------------

/// Handle a paste event.
///
/// Large pastes (≥3 lines or >150 chars) are replaced with a compact
/// placeholder like `[Pasted ~12 lines #3]` while the real content is stored
/// in `paste_contents` for retrieval at submit time.  This mirrors opencode's
/// behaviour and prevents the input box from flooding with multi-hundred-line
/// pastes.  Single-line short strings are inserted verbatim.
pub fn handle_paste(
    content: &str,
    paste_counter: &mut u32,
) -> (String, Option<String>) {
    let line_count = content.lines().count();
    let is_large = line_count >= 3 || content.len() > 150;
    if !is_large {
        return (content.to_string(), None);
    }
    *paste_counter += 1;
    let placeholder = format!("[Pasted ~{} lines #{}]", line_count, paste_counter);
    (placeholder, Some(content.to_string()))
}

/// Normalize a pasted string into a filesystem path if it looks like one.
///
/// Handles:
/// - `file:///path/to/file` — URL-encoded paths
/// - `"C:\path"` / `'/path'` — quoted paths (strips quotes)
/// - Bare absolute paths (`/home/...`, `C:\...`)
///
/// Returns `None` if the text is multiline, not path-shaped, or the resolved
/// path does not exist on the filesystem.  Callers can use the returned
/// `PathBuf` to decide whether to treat the paste as a file attachment.
pub fn detect_pasted_path(text: &str) -> Option<std::path::PathBuf> {
    let trimmed = text.trim();
    // Multiline content is never a bare path.
    if trimmed.contains('\n') {
        return None;
    }
    // Strip outer matching quotes.
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| trimmed.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(trimmed);

    // file:// URL — strip the scheme (skip the leading //).
    let candidate = if let Some(rest) = unquoted.strip_prefix("file://") {
        rest
    } else {
        unquoted
    };

    let path = std::path::Path::new(candidate);
    if path.is_absolute() && path.exists() {
        Some(path.to_path_buf())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Kill ring (Emacs-style kill/yank system)
// ---------------------------------------------------------------------------

/// Kill ring stores accumulated kills (deleted text) for cycling through with Alt+Y.
/// Maintains a FIFO list of kills with a current index for cycling backward.
#[derive(Debug, Clone)]
pub struct KillRing {
    /// List of killed text entries. Most recent is last.
    pub entries: Vec<String>,
    /// Maximum number of entries to keep (prevents unbounded growth).
    max_size: usize,
    /// Current position in kill ring when cycling with Alt+Y (None = most recent).
    pub current_index: Option<usize>,
    /// Tracks whether the last action was a kill (for combining consecutive kills).
    pub last_was_kill: bool,
}

impl KillRing {
    /// Create a new kill ring with default capacity.
    pub fn new() -> Self {
        Self {
            entries: Vec::with_capacity(32),
            max_size: 64,
            current_index: None,
            last_was_kill: false,
        }
    }

    /// Add a kill entry. If the last operation was a kill, append to the most recent entry
    /// instead of creating a new one (for combining consecutive kills).
    pub fn push(&mut self, text: String) {
        if text.is_empty() {
            return;
        }

        if self.last_was_kill && !self.entries.is_empty() {
            // Append to the most recent entry (last_was_kill combines consecutive kills)
            self.entries.last_mut().unwrap().push_str(&text);
        } else {
            // New kill entry
            self.entries.push(text);
            if self.entries.len() > self.max_size {
                self.entries.remove(0);
            }
        }
        self.current_index = None; // Reset cycling to most recent
        self.last_was_kill = true;
    }

    /// Get the current kill to paste (most recent or current index if cycling).
    pub fn get_current(&self) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }

        match self.current_index {
            None => self.entries.last().map(|s| s.as_str()),
            Some(idx) => self.entries.get(idx).map(|s| s.as_str()),
        }
    }

    /// Cycle backward through kill ring (Alt+Y after paste).
    pub fn cycle_backward(&mut self) {
        if self.entries.is_empty() {
            return;
        }

        match self.current_index {
            None => {
                // Start cycling from the second-to-last entry
                if self.entries.len() > 1 {
                    self.current_index = Some(self.entries.len() - 2);
                }
            }
            Some(0) => {
                // Wrap around to the end
                self.current_index = Some(self.entries.len() - 1);
            }
            Some(idx) => {
                self.current_index = Some(idx - 1);
            }
        }
    }

    /// Mark that a non-kill action occurred (resets consecutive kill combination).
    pub fn mark_non_kill(&mut self) {
        self.last_was_kill = false;
    }
}

impl Default for KillRing {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// PromptInput state
// ---------------------------------------------------------------------------

/// Input mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Default,
    Plan,
    Readonly,
}

/// Full state for the prompt input widget.
#[derive(Debug, Clone)]
pub struct PromptInputState {
    /// Current text content.
    pub text: String,
    /// Cursor position (byte offset into `text`).
    pub cursor: usize,
    /// Current vim mode.
    pub vim_mode: VimMode,
    /// Whether vim mode is enabled.
    pub vim_enabled: bool,
    /// Input mode (default / plan / readonly).
    pub mode: InputMode,
    /// Typeahead suggestions.
    pub suggestions: Vec<TypeaheadSuggestion>,
    /// Currently selected suggestion index.
    pub suggestion_index: Option<usize>,
    /// History entries for ↑↓ navigation.
    pub history: Vec<String>,
    /// Current history position (-1 = not browsing history).
    pub history_pos: Option<usize>,
    /// Saved draft while browsing history.
    pub history_draft: String,
    /// Paste counter for placeholder numbering.
    pub paste_counter: u32,
    /// Stored paste contents: counter → content.
    pub paste_contents: std::collections::HashMap<u32, String>,
    /// Yank buffer for vim operations.
    pub yank_buf: String,
    /// Estimated token count for current text.
    pub token_estimate: usize,
    /// Pending multi-key vim command state (persists across keystrokes).
    pub vim_pending: VimPendingState,
    /// Undo stack: Vec of (text, cursor) snapshots before modifications.
    pub undo_stack: Vec<(String, usize)>,
    /// Visual mode selection anchor (byte offset).
    pub visual_anchor: Option<usize>,
    /// Last f/F/t/T find for `;`/`,` repeat.
    pub last_find: Option<(VimFindKind, char)>,
    /// Named registers: key is the register name char (a-z, 0-9, etc.), value is text.
    pub vim_registers: std::collections::HashMap<char, String>,
    /// Macro recording state: Some(register_name) when recording.
    pub vim_macro_recording: Option<char>,
    /// Recorded macro content (accumulates key descriptions while recording).
    pub vim_macro_content: std::collections::HashMap<char, Vec<String>>,
    /// Named marks: maps mark char to (text, cursor) snapshots.
    pub vim_marks: std::collections::HashMap<char, (String, usize)>,
    /// The last modifying command for dot-repeat.
    pub vim_dot_action: Option<DotRepeatAction>,
    /// Pending insert-mode text (accumulates between entering and leaving insert mode).
    vim_insert_text_before: Option<String>,
    /// Command-line buffer for `:` command mode.
    pub vim_command_buf: String,
    /// In-prompt search buffer for `/` search mode.
    pub vim_search_buf: String,
    /// Last executed search pattern for `n`/`N` navigation.
    pub vim_search_last: Option<String>,
    /// Set by `:q`/`:wq` — the app loop should check and honour this.
    pub vim_quit_requested: bool,
    /// Pending image attachments (from clipboard paste) to be sent with next message.
    pub pending_images: Vec<crate::image_paste::PastedImage>,
    /// Emacs-style kill ring for Ctrl+K, Ctrl+U, Ctrl+W operations.
    pub kill_ring: KillRing,
}

impl PromptInputState {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            vim_mode: VimMode::Insert,
            vim_enabled: false,
            mode: InputMode::Default,
            suggestions: Vec::new(),
            suggestion_index: None,
            history: Vec::new(),
            history_pos: None,
            history_draft: String::new(),
            paste_counter: 0,
            paste_contents: std::collections::HashMap::new(),
            yank_buf: String::new(),
            token_estimate: 0,
            vim_pending: VimPendingState::None,
            undo_stack: Vec::new(),
            visual_anchor: None,
            last_find: None,
            vim_registers: std::collections::HashMap::new(),
            vim_macro_recording: None,
            vim_macro_content: std::collections::HashMap::new(),
            vim_marks: std::collections::HashMap::new(),
            vim_dot_action: None,
            vim_insert_text_before: None,
            vim_command_buf: String::new(),
            vim_search_buf: String::new(),
            vim_search_last: None,
            vim_quit_requested: false,
            pending_images: Vec::new(),
            kill_ring: KillRing::new(),
        }
    }

    /// Add a clipboard image attachment to the pending list.
    pub fn add_image(&mut self, img: crate::image_paste::PastedImage) {
        self.pending_images.push(img);
    }

    /// Drain and return all pending image attachments (called at send time).
    pub fn clear_images(&mut self) -> Vec<crate::image_paste::PastedImage> {
        std::mem::take(&mut self.pending_images)
    }

    /// Insert a character at cursor position.
    pub fn insert_char(&mut self, c: char) {
        if self.mode == InputMode::Readonly { return; }
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.update_token_estimate();
    }

    /// Insert a newline (Shift+Enter).
    pub fn insert_newline(&mut self) {
        if self.mode == InputMode::Readonly { return; }
        self.insert_char('\n');
    }

    /// Delete the character before cursor.
    pub fn backspace(&mut self) {
        if self.cursor == 0 || self.mode == InputMode::Readonly { return; }
        let prev = self.text[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.text.remove(prev);
        self.cursor = prev;
        self.update_token_estimate();
    }

    /// Delete the character at cursor.
    pub fn delete(&mut self) {
        if self.cursor >= self.text.len() || self.mode == InputMode::Readonly { return; }
        self.text.remove(self.cursor);
        self.update_token_estimate();
    }

    /// Move cursor left.
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            let prev = self.text[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.cursor = prev;
        }
    }

    /// Move cursor right.
    pub fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            let next = self.text[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.text.len());
            self.cursor = next;
        }
    }

    /// Navigate history up (older).
    pub fn history_up(&mut self) {
        if self.history.is_empty() { return; }
        match self.history_pos {
            None => {
                self.history_draft = self.text.clone();
                self.history_pos = Some(self.history.len() - 1);
            }
            Some(0) => {}
            Some(n) => {
                self.history_pos = Some(n - 1);
            }
        }
        if let Some(pos) = self.history_pos {
            self.text = self.history[pos].clone();
            self.cursor = self.text.len();
            self.update_token_estimate();
        }
    }

    /// Navigate history down (newer).
    pub fn history_down(&mut self) {
        match self.history_pos {
            None => {}
            Some(n) if n + 1 >= self.history.len() => {
                self.history_pos = None;
                self.text = self.history_draft.clone();
                self.cursor = self.text.len();
                self.update_token_estimate();
            }
            Some(n) => {
                self.history_pos = Some(n + 1);
                self.text = self.history[n + 1].clone();
                self.cursor = self.text.len();
                self.update_token_estimate();
            }
        }
    }

    /// Handle a paste event.
    pub fn paste(&mut self, content: &str) {
        let (text, stored) = handle_paste(content, &mut self.paste_counter);
        if let Some(stored_content) = stored {
            self.paste_contents.insert(self.paste_counter, stored_content);
        }
        for c in text.chars() {
            self.text.insert(self.cursor, c);
            self.cursor += c.len_utf8();
        }
        self.update_token_estimate();
        self.kill_ring.mark_non_kill();
    }

    /// Ctrl+K: Cut from cursor to end of line and save to kill ring.
    pub fn kill_line(&mut self) {
        if self.mode == InputMode::Readonly { return; }
        let line_end = self.text[self.cursor..].find('\n')
            .map(|p| self.cursor + p)
            .unwrap_or(self.text.len());

        if line_end > self.cursor {
            let killed = self.text.drain(self.cursor..line_end).collect::<String>();
            self.kill_ring.push(killed);
            self.update_token_estimate();
        }
    }

    /// Ctrl+U: Cut from line start to cursor and save to kill ring.
    pub fn kill_line_backward(&mut self) {
        if self.mode == InputMode::Readonly { return; }
        let line_start = self.text[..self.cursor].rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(0);

        if self.cursor > line_start {
            let killed = self.text.drain(line_start..self.cursor).collect::<String>();
            self.kill_ring.push(killed);
            self.cursor = line_start;
            self.update_token_estimate();
        }
    }

    /// Ctrl+W: Cut previous word and save to kill ring.
    pub fn kill_word_backward(&mut self) {
        if self.mode == InputMode::Readonly || self.cursor == 0 { return; }
        let before = &self.text[..self.cursor];
        let chars: Vec<char> = before.chars().collect();
        let mut idx = chars.len();
        while idx > 0 && chars[idx - 1].is_whitespace() {
            idx -= 1;
        }
        if idx == 0 {
            return;
        }
        if is_word_char(chars[idx - 1]) {
            while idx > 0 && is_word_char(chars[idx - 1]) {
                idx -= 1;
            }
        } else {
            while idx > 0 && !is_word_char(chars[idx - 1]) && !chars[idx - 1].is_whitespace() {
                idx -= 1;
            }
        }
        let kill_start = char_idx_to_byte(before, idx);
        if kill_start < self.cursor {
            let killed = self.text.drain(kill_start..self.cursor).collect::<String>();
            self.kill_ring.push(killed);
            self.cursor = kill_start;
            self.update_token_estimate();
        }
    }

    /// Ctrl+Y: Paste from kill ring (most recent).
    pub fn yank(&mut self) {
        if self.mode == InputMode::Readonly { return; }
        if let Some(text) = self.kill_ring.get_current() {
            for c in text.chars() {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            self.update_token_estimate();
            self.kill_ring.mark_non_kill();
        }
    }

    /// Alt+Y: Cycle through kill ring backward.
    pub fn yank_pop(&mut self) {
        if self.mode == InputMode::Readonly { return; }
        self.kill_ring.cycle_backward();
    }

    /// Alt+Backspace: Delete word backward.
    pub fn delete_word_backward(&mut self) {
        if self.mode == InputMode::Readonly || self.cursor == 0 { return; }
        let before = &self.text[..self.cursor];
        let chars: Vec<char> = before.chars().collect();
        let mut idx = chars.len();
        while idx > 0 && chars[idx - 1].is_whitespace() {
            idx -= 1;
        }
        if idx == 0 {
            return;
        }
        if is_word_char(chars[idx - 1]) {
            while idx > 0 && is_word_char(chars[idx - 1]) {
                idx -= 1;
            }
        } else {
            while idx > 0 && !is_word_char(chars[idx - 1]) && !chars[idx - 1].is_whitespace() {
                idx -= 1;
            }
        }
        let delete_start = char_idx_to_byte(before, idx);
        if delete_start < self.cursor {
            self.text.drain(delete_start..self.cursor);
            self.cursor = delete_start;
            self.update_token_estimate();
            self.kill_ring.mark_non_kill();
        }
    }

    /// Alt+Delete: Delete word forward.
    pub fn delete_word_forward(&mut self) {
        if self.mode == InputMode::Readonly || self.cursor >= self.text.len() { return; }
        let rest = &self.text[self.cursor..];
        let chars: Vec<char> = rest.chars().collect();
        let mut idx = 0;
        while idx < chars.len() && chars[idx].is_whitespace() {
            idx += 1;
        }
        if idx >= chars.len() {
            return;
        }
        if is_word_char(chars[idx]) {
            while idx < chars.len() && is_word_char(chars[idx]) {
                idx += 1;
            }
        } else {
            while idx < chars.len() && !is_word_char(chars[idx]) && !chars[idx].is_whitespace() {
                idx += 1;
            }
        }
        let delete_end = self.cursor + char_idx_to_byte(rest, idx);
        if delete_end > self.cursor {
            self.text.drain(self.cursor..delete_end);
            self.update_token_estimate();
            self.kill_ring.mark_non_kill();
        }
    }

    /// Alt+B: Jump to previous word.
    pub fn move_word_backward(&mut self) {
        if self.cursor == 0 { return; }
        let before = &self.text[..self.cursor];
        let chars: Vec<char> = before.chars().collect();
        let mut idx = chars.len();
        while idx > 0 && chars[idx - 1].is_whitespace() {
            idx -= 1;
        }
        if idx == 0 {
            return;
        }
        if is_word_char(chars[idx - 1]) {
            while idx > 0 && is_word_char(chars[idx - 1]) {
                idx -= 1;
            }
        } else {
            while idx > 0 && !is_word_char(chars[idx - 1]) && !chars[idx - 1].is_whitespace() {
                idx -= 1;
            }
        }
        self.cursor = char_idx_to_byte(before, idx);
    }

    /// Alt+F: Jump to next word.
    pub fn move_word_forward(&mut self) {
        if self.cursor >= self.text.len() { return; }
        let rest = &self.text[self.cursor..];
        let chars: Vec<char> = rest.chars().collect();
        let mut idx = 0;
        if idx < chars.len() {
            if is_word_char(chars[idx]) {
                while idx < chars.len() && is_word_char(chars[idx]) {
                    idx += 1;
                }
            } else if !chars[idx].is_whitespace() {
                while idx < chars.len() && !is_word_char(chars[idx]) && !chars[idx].is_whitespace() {
                    idx += 1;
                }
            }
        }
        while idx < chars.len() && chars[idx].is_whitespace() {
            idx += 1;
        }
        self.cursor = self.cursor + char_idx_to_byte(rest, idx);
    }

    /// Alt+D: Delete word after cursor.
    pub fn delete_word_at_cursor(&mut self) {
        if self.mode == InputMode::Readonly || self.cursor >= self.text.len() { return; }
        let rest = &self.text[self.cursor..];
        let chars: Vec<char> = rest.chars().collect();
        let mut idx = 0;
        while idx < chars.len() && chars[idx].is_whitespace() {
            idx += 1;
        }
        if idx >= chars.len() {
            return;
        }
        if is_word_char(chars[idx]) {
            while idx < chars.len() && is_word_char(chars[idx]) {
                idx += 1;
            }
        } else {
            while idx < chars.len() && !is_word_char(chars[idx]) && !chars[idx].is_whitespace() {
                idx += 1;
            }
        }
        let delete_end = self.cursor + char_idx_to_byte(rest, idx);
        if delete_end > self.cursor {
            self.text.drain(self.cursor..delete_end);
            self.update_token_estimate();
            self.kill_ring.mark_non_kill();
        }
    }

    /// Apply a vim command using the full state-machine key handler.
    pub fn vim_command(&mut self, key: &str) {
        // ---- Escape always cancels recording, pending state, returns to Normal ----
        if key == "Escape" {
            // If leaving insert mode, finalise dot-repeat insert action
            if self.vim_mode == VimMode::Insert {
                if let Some(before) = self.vim_insert_text_before.take() {
                    // Compute inserted text as the new characters added since mode entry
                    let inserted = if self.text.len() >= before.len() {
                        // Simple case: text only grew (cursor at end of inserted span)
                        let from = before.len().min(self.cursor);
                        let _ = from; // use cursor-based diff below
                        // Find the diff between before/after texts at current cursor
                        // Inserted = text[insert_start..cursor] but we don't track start.
                        // Approximate: whole text minus before, substring at cursor.
                        // Better: store cursor-at-entry and extract.
                        self.text[before.len().min(self.text.len())..self.cursor.min(self.text.len())].to_string()
                    } else {
                        String::new()
                    };
                    if !inserted.is_empty() {
                        self.vim_dot_action = Some(DotRepeatAction::Insert {
                            text: inserted,
                            mode_after_insert: false,
                        });
                    }
                }
            }
            self.vim_mode = VimMode::Normal;
            self.vim_pending = VimPendingState::None;
            self.visual_anchor = None;
            self.normalize();
            return;
        }

        // ---- Command-line mode (`:`) ----
        if self.vim_mode == VimMode::Command {
            match key {
                "Backspace" => {
                    if self.vim_command_buf.is_empty() {
                        self.vim_mode = VimMode::Normal;
                    } else {
                        self.vim_command_buf.pop();
                    }
                }
                "Enter" => {
                    let cmd = self.vim_command_buf.trim().to_string();
                    self.vim_command_buf.clear();
                    self.vim_mode = VimMode::Normal;
                    self.execute_vim_cmdline(&cmd);
                }
                _ if key.len() == 1 => {
                    self.vim_command_buf.push(key.chars().next().unwrap());
                }
                _ => {}
            }
            return;
        }

        // ---- In-prompt search mode (`/`) ----
        if self.vim_mode == VimMode::Search {
            match key {
                "Backspace" => {
                    if self.vim_search_buf.is_empty() {
                        self.vim_mode = VimMode::Normal;
                    } else {
                        self.vim_search_buf.pop();
                    }
                }
                "Enter" => {
                    let pattern = self.vim_search_buf.clone();
                    if !pattern.is_empty() {
                        self.vim_search_last = Some(pattern.clone());
                        self.vim_search_forward(&pattern, 0);
                    }
                    self.vim_search_buf.clear();
                    self.vim_mode = VimMode::Normal;
                }
                _ if key.len() == 1 => {
                    self.vim_search_buf.push(key.chars().next().unwrap());
                }
                _ => {}
            }
            return;
        }

        // ---- Accumulate key into macro recording buffer ----
        if let Some(reg) = self.vim_macro_recording {
            // `q` in normal mode stops recording
            if key == "q" && self.vim_mode == VimMode::Normal
                && self.vim_pending == VimPendingState::None
            {
                self.stop_macro_recording();
                return;
            }
            if let Some(keys) = self.vim_macro_content.get_mut(&reg) {
                keys.push(key.to_string());
            }
        }

        // ---- Handle new pending states before apply_vim_key ----
        match self.vim_pending.clone() {
            VimPendingState::Register('\0') => {
                // Waiting for register name char after `"`
                if key.len() == 1 {
                    let reg = key.chars().next().unwrap();
                    self.vim_pending = VimPendingState::RegisterOp(reg);
                } else {
                    self.vim_pending = VimPendingState::None;
                }
                return;
            }
            VimPendingState::RegisterOp(reg) => {
                // Waiting for operator after `"<reg>`
                match key {
                    "y" => {
                        // Yank current line to register
                        let ls = self.text[..self.cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
                        let le = self.text[self.cursor..].find('\n')
                            .map(|p| self.cursor + p + 1)
                            .unwrap_or(self.text.len());
                        let yanked = self.text[ls..le].to_string();
                        self.yank_to_register(reg, &yanked);
                        self.yank_buf = yanked;
                    }
                    "d" => {
                        // Delete current line to register
                        let ls = self.text[..self.cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
                        let le = self.text[self.cursor..].find('\n')
                            .map(|p| self.cursor + p + 1)
                            .unwrap_or(self.text.len());
                        let deleted = self.text[ls..le].to_string();
                        self.push_undo();
                        self.yank_to_register(reg, &deleted);
                        self.yank_buf = deleted;
                        let le = le.min(self.text.len());
                        self.text.drain(ls..le);
                        self.cursor = ls.min(self.text.len());
                        self.vim_pending = VimPendingState::None;
                        self.normalize();
                        return;
                    }
                    "p" => {
                        // Paste from register after cursor
                        if let Some(buf) = self.paste_from_register(reg) {
                            let insert_pos = if self.cursor < self.text.len() {
                                self.text[self.cursor..].char_indices().nth(1)
                                    .map(|(b, _)| self.cursor + b)
                                    .unwrap_or(self.text.len())
                            } else {
                                self.text.len()
                            };
                            self.push_undo();
                            self.text.insert_str(insert_pos, &buf);
                            self.cursor = (insert_pos + buf.len()).saturating_sub(1);
                            self.vim_pending = VimPendingState::None;
                            self.normalize();
                            return;
                        }
                    }
                    _ => {}
                }
                self.vim_pending = VimPendingState::None;
                return;
            }
            VimPendingState::Mark => {
                // `m<char>` — set mark
                if key.len() == 1 {
                    let name = key.chars().next().unwrap();
                    self.set_mark(name);
                }
                self.vim_pending = VimPendingState::None;
                return;
            }
            VimPendingState::JumpMark => {
                // `'<char>` — jump to mark
                if key.len() == 1 {
                    let name = key.chars().next().unwrap();
                    self.jump_to_mark(name);
                }
                self.vim_pending = VimPendingState::None;
                return;
            }
            VimPendingState::MacroRecord => {
                // `q<char>` — start recording into register; clear pending first.
                self.vim_pending = VimPendingState::None;
                if key.len() == 1 {
                    let reg = key.chars().next().unwrap();
                    self.start_macro_recording(reg);
                }
                return;
            }
            VimPendingState::MacroReplay => {
                // `@<char>` — replay macro; clear pending BEFORE recursing so
                // recursive vim_command calls don't re-enter this arm.
                self.vim_pending = VimPendingState::None;
                if key.len() == 1 {
                    let reg = key.chars().next().unwrap();
                    let keys = self.replay_macro(reg);
                    // Replay each recorded key (avoid infinite loops by cloning)
                    for k in keys {
                        // Guard: don't replay if we somehow entered macro record for same reg
                        if self.vim_macro_recording == Some(reg) { break; }
                        self.vim_command(&k.clone());
                    }
                }
                return;
            }
            _ => {}
        }

        // ---- Dot-repeat `.` — replay last modifying action ----
        if key == "." && self.vim_mode == VimMode::Normal
            && self.vim_pending == VimPendingState::None
        {
            if let Some(action) = self.vim_dot_action.clone() {
                match action {
                    DotRepeatAction::Insert { text: ins, .. } => {
                        self.push_undo();
                        self.text.insert_str(self.cursor, &ins);
                        self.cursor += ins.len();
                        self.normalize();
                        return;
                    }
                    DotRepeatAction::DeleteChars { count } => {
                        self.push_undo();
                        let mut deleted = 0usize;
                        while deleted < count && self.cursor < self.text.len() {
                            let clen = self.text[self.cursor..].chars().next()
                                .map(|c| c.len_utf8()).unwrap_or(1);
                            self.text.drain(self.cursor..self.cursor + clen);
                            deleted += 1;
                        }
                        self.normalize();
                        return;
                    }
                    DotRepeatAction::Change { deleted: _del, inserted: ins } => {
                        self.push_undo();
                        self.text.insert_str(self.cursor, &ins);
                        self.cursor += ins.len();
                        self.normalize();
                        return;
                    }
                    DotRepeatAction::ReplaceChar { ch } => {
                        if self.cursor < self.text.len() {
                            self.push_undo();
                            let clen = self.text[self.cursor..].chars().next()
                                .map(|c| c.len_utf8()).unwrap_or(1);
                            self.text.replace_range(self.cursor..self.cursor + clen, &ch.to_string());
                            self.normalize();
                        }
                        return;
                    }
                }
            }
            return;
        }

        // ---- Track when entering insert mode for dot-repeat ----
        let was_normal = self.vim_mode == VimMode::Normal;
        let prev_text_len = self.text.len();

        // `u` — undo: restore previous text/cursor snapshot
        if key == "u" && self.vim_mode == VimMode::Normal && self.vim_pending == VimPendingState::None {
            if let Some((t, c)) = self.undo_stack.pop() {
                self.text = t;
                self.cursor = c;
                self.normalize();
            }
            return;
        }
        // Enter visual mode with `v` — anchor the selection start
        if key == "v" && self.vim_mode == VimMode::Normal && self.vim_pending == VimPendingState::None {
            self.vim_mode = VimMode::Visual;
            self.visual_anchor = Some(self.cursor);
            return;
        }
        // Enter command-line mode with `:`
        if key == ":" && self.vim_mode == VimMode::Normal && self.vim_pending == VimPendingState::None {
            self.vim_mode = VimMode::Command;
            self.vim_command_buf.clear();
            return;
        }
        // Enter in-prompt search with `/`
        if key == "/" && self.vim_mode == VimMode::Normal && self.vim_pending == VimPendingState::None {
            self.vim_mode = VimMode::Search;
            self.vim_search_buf.clear();
            return;
        }
        // Enter visual-line mode with `V`
        if key == "V" && self.vim_mode == VimMode::Normal && self.vim_pending == VimPendingState::None {
            self.vim_mode = VimMode::VisualLine;
            let ls = self.text[..self.cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
            self.visual_anchor = Some(ls);
            return;
        }
        // Enter visual-block mode with Ctrl+V
        if key == "\x16" && self.vim_mode == VimMode::Normal && self.vim_pending == VimPendingState::None {
            self.vim_mode = VimMode::VisualBlock;
            self.visual_anchor = Some(self.cursor);
            return;
        }
        // `n` — repeat last search forward
        if key == "n" && self.vim_mode == VimMode::Normal && self.vim_pending == VimPendingState::None {
            if let Some(pat) = self.vim_search_last.clone() {
                self.vim_search_forward(&pat, 1);
            }
            return;
        }
        // `N` — repeat last search backward
        if key == "N" && self.vim_mode == VimMode::Normal && self.vim_pending == VimPendingState::None {
            if let Some(pat) = self.vim_search_last.clone() {
                self.vim_search_backward(&pat);
            }
            return;
        }
        // In visual-line mode, `y`/`d`/`c` operate on whole lines, motion keys extend selection
        if self.vim_mode == VimMode::VisualLine {
            if let Some(anchor) = self.visual_anchor {
                let line_start = |pos: usize, s: &str| -> usize {
                    s[..pos].rfind('\n').map(|p| p + 1).unwrap_or(0)
                };
                let line_end = |pos: usize, s: &str| -> usize {
                    s[pos..].find('\n').map(|p| pos + p + 1).unwrap_or(s.len())
                };
                let sel_start = line_start(anchor.min(self.cursor), &self.text);
                let sel_end = line_end(anchor.max(self.cursor), &self.text);
                match key {
                    "y" => {
                        self.yank_buf = self.text[sel_start..sel_end].to_string();
                        self.cursor = sel_start;
                        self.vim_mode = VimMode::Normal;
                        self.visual_anchor = None;
                        return;
                    }
                    "d" | "x" => {
                        self.push_undo();
                        self.yank_buf = self.text[sel_start..sel_end].to_string();
                        let char_count = self.yank_buf.chars().count();
                        self.text.drain(sel_start..sel_end);
                        self.cursor = sel_start.min(self.text.len());
                        self.vim_mode = VimMode::Normal;
                        self.visual_anchor = None;
                        self.vim_dot_action = Some(DotRepeatAction::DeleteChars { count: char_count });
                        self.normalize();
                        return;
                    }
                    "c" => {
                        self.push_undo();
                        self.yank_buf = self.text[sel_start..sel_end].to_string();
                        self.text.drain(sel_start..sel_end);
                        self.cursor = sel_start;
                        self.vim_mode = VimMode::Insert;
                        self.visual_anchor = None;
                        self.vim_insert_text_before = Some(self.text.clone());
                        self.normalize();
                        return;
                    }
                    _ => {
                        // Motion keys extend the selection (handled by apply_vim_key below)
                    }
                }
            }
        }
        // In visual-block mode, treat like character-wise visual for single-line input
        if self.vim_mode == VimMode::VisualBlock {
            if let Some(anchor) = self.visual_anchor {
                let from = anchor.min(self.cursor);
                let to_excl = anchor.max(self.cursor);
                let to = self.text[to_excl..].char_indices().nth(1).map(|(b,_)| to_excl+b).unwrap_or(self.text.len());
                match key {
                    "y" => {
                        self.yank_buf = self.text[from..to].to_string();
                        self.cursor = from;
                        self.vim_mode = VimMode::Normal;
                        self.visual_anchor = None;
                        return;
                    }
                    "d" | "x" => {
                        self.push_undo();
                        self.yank_buf = self.text[from..to].to_string();
                        let char_count = self.yank_buf.chars().count();
                        self.text.drain(from..to);
                        self.cursor = from.min(self.text.len());
                        self.vim_mode = VimMode::Normal;
                        self.visual_anchor = None;
                        self.vim_dot_action = Some(DotRepeatAction::DeleteChars { count: char_count });
                        self.normalize();
                        return;
                    }
                    "c" => {
                        self.push_undo();
                        self.yank_buf = self.text[from..to].to_string();
                        self.text.drain(from..to);
                        self.cursor = from;
                        self.vim_mode = VimMode::Insert;
                        self.visual_anchor = None;
                        self.vim_insert_text_before = Some(self.text.clone());
                        self.normalize();
                        return;
                    }
                    _ => {}
                }
            }
        }
        // In visual mode, `y`/`d`/`c` operate on the selection, Escape exits
        if self.vim_mode == VimMode::Visual {
            if let Some(anchor) = self.visual_anchor {
                let from = anchor.min(self.cursor);
                let to_excl = anchor.max(self.cursor);
                let to = self.text[to_excl..].char_indices().nth(1).map(|(b,_)| to_excl+b).unwrap_or(self.text.len());
                match key {
                    "y" => {
                        self.yank_buf = self.text[from..to].to_string();
                        self.cursor = from;
                        self.vim_mode = VimMode::Normal;
                        self.visual_anchor = None;
                        return;
                    }
                    "d" | "x" => {
                        self.push_undo();
                        self.yank_buf = self.text[from..to].to_string();
                        // Count chars to delete BEFORE mutating text
                        let char_count = self.yank_buf.chars().count();
                        self.text.drain(from..to);
                        self.cursor = from.min(self.text.len());
                        self.vim_mode = VimMode::Normal;
                        self.visual_anchor = None;
                        self.vim_dot_action = Some(DotRepeatAction::DeleteChars {
                            count: char_count,
                        });
                        self.normalize();
                        return;
                    }
                    "c" => {
                        self.push_undo();
                        self.yank_buf = self.text[from..to].to_string();
                        self.text.drain(from..to);
                        self.cursor = from;
                        self.vim_mode = VimMode::Insert;
                        self.visual_anchor = None;
                        self.vim_insert_text_before = Some(self.text.clone());
                        self.normalize();
                        return;
                    }
                    _ => {
                        // Motion keys still move cursor in visual mode
                    }
                }
            }
        }

        let snapshot_text = self.text.clone();
        let snapshot_cursor = self.cursor;
        let modified = apply_vim_key(
            &mut self.vim_mode,
            &mut self.text,
            &mut self.cursor,
            key,
            &mut self.yank_buf,
            &mut self.vim_pending,
            &mut self.last_find,
        );
        if modified {
            self.undo_stack.push((snapshot_text.clone(), snapshot_cursor));
            if self.undo_stack.len() > 100 {
                self.undo_stack.remove(0);
            }
            // Update dot-repeat for simple modifying commands (normal mode only)
            if was_normal {
                match key {
                    "x" => {
                        self.vim_dot_action = Some(DotRepeatAction::DeleteChars { count: 1 });
                    }
                    "X" => {
                        self.vim_dot_action = Some(DotRepeatAction::DeleteChars { count: 1 });
                    }
                    _ => {}
                }
            }
        }

        // If we just entered insert mode from normal mode, record text snapshot for dot-repeat
        if was_normal && self.vim_mode == VimMode::Insert {
            self.vim_insert_text_before = Some(self.text.clone());
        }

        // Handle `r` replace pending → after confirm, store dot action
        if let VimPendingState::None = self.vim_pending {
            if modified && was_normal {
                // Check if a replace happened (text changed by exactly 1 char at cursor)
                if self.text.len() == prev_text_len && self.text != snapshot_text {
                    // Likely a replace — extract the replacement char at snapshot_cursor
                    if let Some(ch) = self.text[snapshot_cursor..].chars().next() {
                        // Verify it's different from what was there before
                        let old_ch = snapshot_text[snapshot_cursor..].chars().next();
                        if old_ch != Some(ch) {
                            self.vim_dot_action = Some(DotRepeatAction::ReplaceChar { ch });
                        }
                    }
                }
            }
        }

        // Update visual anchor tracking when in visual mode
        if (self.vim_mode == VimMode::Visual || self.vim_mode == VimMode::VisualBlock) && self.visual_anchor.is_none() {
            self.visual_anchor = Some(self.cursor);
        }
        self.normalize();
    }

    /// Push the current (text, cursor) to the undo stack.
    pub fn push_undo(&mut self) {
        self.undo_stack.push((self.text.clone(), self.cursor));
        if self.undo_stack.len() > 100 {
            self.undo_stack.remove(0);
        }
    }

    // ---- Named registers ----

    /// Store `text` in the named register `register`.
    pub fn yank_to_register(&mut self, register: char, text: &str) {
        self.vim_registers.insert(register, text.to_string());
    }

    /// Retrieve text from the named register `register`, if any.
    pub fn paste_from_register(&mut self, register: char) -> Option<String> {
        self.vim_registers.get(&register).cloned()
    }

    // ---- Marks ----

    /// Set mark `name` at the current cursor position.
    pub fn set_mark(&mut self, name: char) {
        self.vim_marks.insert(name, (self.text.clone(), self.cursor));
    }

    /// Move cursor to the position recorded for mark `name`, if the text still matches.
    pub fn jump_to_mark(&mut self, name: char) {
        if let Some((_saved_text, saved_cursor)) = self.vim_marks.get(&name).cloned() {
            // Clamp to current text length in case text changed.
            let target = saved_cursor.min(self.text.len());
            // Ensure we land on a char boundary.
            let mut pos = target;
            while pos > 0 && !self.text.is_char_boundary(pos) {
                pos -= 1;
            }
            self.cursor = pos;
        }
    }

    // ---- Macro recording ----

    /// Begin recording a macro into register `register`.
    /// If already recording, stops the current recording first.
    pub fn start_macro_recording(&mut self, register: char) {
        self.vim_macro_recording = Some(register);
        self.vim_macro_content.insert(register, Vec::new());
    }

    /// Stop recording the current macro. Returns the register name that was being recorded.
    pub fn stop_macro_recording(&mut self) -> Option<char> {
        self.vim_macro_recording.take()
    }

    /// Return the recorded key sequence for `register`, or an empty vec.
    pub fn replay_macro(&self, register: char) -> Vec<String> {
        self.vim_macro_content.get(&register).cloned().unwrap_or_default()
    }

    // ---- Vim command-line execution ----

    /// Execute a `:` command-line command.
    /// Recognised: `q`/`quit`, `wq`, `set` (no-op), `noh` (clear search highlight).
    pub fn execute_vim_cmdline(&mut self, cmd: &str) {
        match cmd {
            "q" | "quit" | "wq" | "x" => {
                // In prompt context we can only signal quit by clearing + a special flag.
                // We set a dedicated field that the app loop can inspect.
                self.vim_quit_requested = true;
            }
            "noh" | "nohlsearch" => {
                self.vim_search_last = None;
            }
            s if s.starts_with("set ") => {
                // `:set vim` → enable, `:set novim` → disable (runtime toggle)
                let arg = s["set ".len()..].trim();
                match arg {
                    "vim" => { self.vim_enabled = true; }
                    "novim" => { self.vim_enabled = false; }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // ---- In-prompt search ----

    /// Move cursor to the next occurrence of `pattern` after `cursor + skip`.
    /// `skip = 0` finds from current position; `skip = 1` finds the *next* one.
    pub fn vim_search_forward(&mut self, pattern: &str, skip: usize) {
        if pattern.is_empty() { return; }
        let start = if skip > 0 {
            // Start after the current character to avoid re-matching same position
            let next = self.text[self.cursor..].char_indices().nth(1)
                .map(|(b, _)| self.cursor + b)
                .unwrap_or(0);
            next
        } else {
            self.cursor
        };
        // Search from `start` forward, then wrap around
        let text_lc = self.text.to_lowercase();
        let pat_lc = pattern.to_lowercase();
        if let Some(pos) = text_lc[start..].find(&pat_lc) {
            self.cursor = start + pos;
            return;
        }
        // Wrap: search from beginning
        if let Some(pos) = text_lc.find(&pat_lc) {
            self.cursor = pos;
        }
    }

    /// Move cursor to the previous occurrence of `pattern` before current cursor.
    pub fn vim_search_backward(&mut self, pattern: &str) {
        if pattern.is_empty() { return; }
        let text_lc = self.text.to_lowercase();
        let pat_lc = pattern.to_lowercase();
        // Find all occurrences, pick the last one before cursor
        let before = &text_lc[..self.cursor];
        if let Some(pos) = before.rfind(&pat_lc) {
            self.cursor = pos;
            return;
        }
        // Wrap: find last occurrence in whole text
        if let Some(pos) = text_lc.rfind(&pat_lc) {
            self.cursor = pos;
        }
    }

    /// Clear the input and reset state.
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.suggestions.clear();
        self.suggestion_index = None;
        self.history_pos = None;
        self.token_estimate = 0;
        self.vim_pending = VimPendingState::None;
        self.visual_anchor = None;
        self.vim_command_buf.clear();
        self.vim_search_buf.clear();
    }

    /// Take the current text, clearing the input.
    pub fn take(&mut self) -> String {
        let text = self.text.clone();
        self.clear();
        text
    }

    /// Update typeahead suggestions for the current text.
    pub fn update_suggestions(&mut self, slash_commands: &[(&str, &str)]) {
        self.suggestions = compute_typeahead(&self.text, slash_commands);
        if self.suggestions.is_empty() {
            self.suggestion_index = None;
        } else if self.text.starts_with('/') {
            let idx = self.suggestion_index.unwrap_or(0).min(self.suggestions.len() - 1);
            self.suggestion_index = Some(idx);
        } else {
            self.suggestion_index = None;
        }
    }

    /// Select the next suggestion.
    pub fn suggestion_next(&mut self) {
        if self.suggestions.is_empty() { return; }
        self.suggestion_index = Some(
            self.suggestion_index.map_or(0, |i| (i + 1) % self.suggestions.len())
        );
    }

    /// Select the previous suggestion.
    pub fn suggestion_prev(&mut self) {
        if self.suggestions.is_empty() { return; }
        self.suggestion_index = Some(
            self.suggestion_index
                .map_or(0, |i| if i == 0 { self.suggestions.len() - 1 } else { i - 1 })
        );
    }

    /// Accept the current suggestion.
    pub fn accept_suggestion(&mut self) {
        if let Some(idx) = self.suggestion_index {
            if let Some(s) = self.suggestions.get(idx) {
                self.text = s.text.clone();
                self.cursor = self.text.len();
                self.suggestions.clear();
                self.suggestion_index = None;
                self.update_token_estimate();
            }
        }
    }

    /// Replace the full text buffer and move the cursor to the end.
    pub fn replace_text(&mut self, text: String) {
        self.text = text;
        self.cursor = self.text.len();
        self.history_pos = None;
        self.suggestion_index = None;
        self.update_token_estimate();
    }

    /// Normalize cursor and metadata after external field updates.
    pub fn normalize(&mut self) {
        self.cursor = self.cursor.min(self.text.len());
        while self.cursor > 0 && !self.text.is_char_boundary(self.cursor) {
            self.cursor -= 1;
        }
        self.update_token_estimate();
    }

    /// Rough token estimate: ~4 chars per token.
    fn update_token_estimate(&mut self) {
        self.token_estimate = (self.text.len() + 3) / 4;
    }

    pub fn is_empty(&self) -> bool { self.text.trim().is_empty() }
}

impl Default for PromptInputState {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Return the number of rows needed to render the input for the given text.
/// Minimum 4 (1 top-line + 1 text row + 1 bottom-line + 1 breathing room), capped at 12.
pub fn input_height(state: &PromptInputState) -> u16 {
    let line_count = if state.text.is_empty() {
        1
    } else {
        state.text.lines().count().max(1)
    };
    // top-line + text rows + bottom-line + 1 breathing-room row, at least 4, at most 12
    let base = ((line_count as u16) + 3).max(4).min(12);
    // +1 for image pill row when images are pending
    base + if state.pending_images.is_empty() { 0 } else { 1 }
}

/// Render the prompt input widget in the same low-chrome style as Claurst:
/// multi-line input rows (one per logical line in the text) plus an accent
/// underline. Suggestions are rendered by the footer, not as a boxed dropdown
/// here.
pub fn render_prompt_input(
    state: &PromptInputState,
    area: Rect,
    buf: &mut Buffer,
    focused: bool,
    mode: InputMode,
    accent_override: Color,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // If images are pending, render a pill row above everything else and shrink area.
    let (area, image_row_y) = if !state.pending_images.is_empty() && area.height > 1 {
        let pill_y = area.y;
        let rest = Rect { x: area.x, y: area.y + 1, width: area.width, height: area.height - 1 };
        (rest, Some(pill_y))
    } else {
        (area, None)
    };

    if let Some(pill_y) = image_row_y {
        let mut pills: Vec<Span<'static>> = Vec::new();
        for img in &state.pending_images {
            let label = if let Some((w, h)) = img.dimensions {
                format!(" \u{f03e} {} {}x{} ", img.label, w, h)  // nerd-font image icon, fallback to plain text
            } else {
                format!(" \u{f03e} {} ", img.label)
            };
            pills.push(Span::styled(label, Style::default().fg(Color::Black).bg(Color::Cyan)));
            pills.push(Span::raw(" "));
        }
        if !pills.is_empty() {
            Paragraph::new(Line::from(pills))
                .render(Rect { x: area.x, y: pill_y, width: area.width, height: 1 }, buf);
        }
    }

    let accent = match mode {
        InputMode::Readonly => CLAUDE_ORANGE,   // locked while streaming — always pink
        _ => accent_override,                   // use mode-aware accent color
    };
    let prompt_prefix = format!("{PROMPT_POINTER} ");
    let prefix_width = prompt_prefix.chars().count() as u16;
    let available_width = area.width.saturating_sub(prefix_width) as usize;
    // Blink the cursor at ~1 Hz (530 ms on, 530 ms off).  We use wall-clock
    // time so the period is stable regardless of frame rate.
    let cursor_blink_on = {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        (ms / 530) % 2 == 0
    };
    let cursor = if focused && cursor_blink_on { "\u{2588}" } else { "" };

    // Build the full content string (with cursor embedded).
    let full_content: String = if state.text.is_empty() {
        if focused {
            cursor.to_string()
        } else if mode == InputMode::Default {
            "How can I help you?".to_string()
        } else {
            String::new()
        }
    } else if focused && state.cursor <= state.text.len() {
        let mut text = state.text.clone();
        text.insert_str(state.cursor, cursor);
        text
    } else {
        state.text.clone()
    };

    // Top separator line (matches bottom underline — visual "box" around the prompt).
    if area.height > 0 {
        Paragraph::new(Line::from(vec![Span::styled(
            "\u{2500}".repeat(area.width as usize),
            Style::default().fg(accent),
        )]))
        .render(Rect { x: area.x, y: area.y, width: area.width, height: 1 }, buf);
    }

    // Text rows start 1 row below the top separator.
    let text_start_y = area.y + 1;

    // Split into logical lines; guarantee at least one.
    let logical_lines: Vec<String> = {
        let collected: Vec<String> = full_content.lines().map(|l| l.to_string()).collect();
        if collected.is_empty() { vec![String::new()] } else { collected }
    };

    let text_style = if state.text.is_empty() && !focused {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White)
    };

    // Render each logical line (truncated to available width from the right).
    // Reserve 1 row at top (separator) + 1 at bottom (underline).
    let max_text_rows = area.height.saturating_sub(2) as usize;
    for (i, line_text) in logical_lines.iter().enumerate() {
        if i >= max_text_rows {
            break;
        }
        let row_y = text_start_y + i as u16;

        let visible: String = line_text
            .chars()
            .rev()
            .take(available_width)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let spans: Vec<Span<'static>> = if i == 0 {
            vec![
                Span::styled(prompt_prefix.clone(), Style::default().fg(accent).add_modifier(Modifier::BOLD)),
                Span::styled(visible, text_style),
            ]
        } else {
            // Continuation lines: indent to align with text after prefix
            vec![
                Span::raw(" ".repeat(prefix_width as usize)),
                Span::styled(visible, text_style),
            ]
        };

        Paragraph::new(Line::from(spans)).render(
            Rect { x: area.x, y: row_y, width: area.width, height: 1 },
            buf,
        );
    }

    // Vim command / search row (shown below text lines, before underline).
    let text_rows_rendered = logical_lines.len().min(max_text_rows);
    let cmd_line: Option<Line<'static>> = match state.vim_mode {
        VimMode::Command => {
            let buf_text = format!(":{}\u{2588}", state.vim_command_buf);
            Some(Line::from(vec![Span::styled(buf_text, Style::default().fg(Color::Cyan))]))
        }
        VimMode::Search => {
            let buf_text = format!("/{}\u{2588}", state.vim_search_buf);
            Some(Line::from(vec![Span::styled(buf_text, Style::default().fg(Color::Yellow))]))
        }
        _ => None,
    };

    let (cmdline_row, underline_row) = if let Some(ref _cl) = cmd_line {
        let cmd_y = text_start_y + text_rows_rendered as u16;
        let ul_y = cmd_y + 1;
        (Some(cmd_y), ul_y)
    } else {
        (None, text_start_y + text_rows_rendered as u16)
    };

    if let (Some(row), Some(cl)) = (cmdline_row, cmd_line) {
        if row < area.y + area.height {
            Paragraph::new(cl).render(
                Rect { x: area.x, y: row, width: area.width, height: 1 },
                buf,
            );
        }
    }

    if underline_row < area.y + area.height {
        Paragraph::new(Line::from(vec![Span::styled(
            "\u{2500}".repeat(area.width as usize),
            Style::default().fg(accent),
        )]))
        .render(
            Rect { x: area.x, y: underline_row, width: area.width, height: 1 },
            buf,
        );
    }

    // Token estimate overlay on the first text row (top-right corner).
    // Format mirrors TS formatTokens: compact "1.3k" for ≥1000, raw number below that.
    if state.text.len() > 1000 && area.height > 1 {
        let n = state.token_estimate;
        let formatted = if n >= 1000 {
            let k = n as f64 / 1000.0;
            // One decimal place, suppress trailing ".0" (e.g. 2000 → "2k", 1300 → "1.3k")
            if (k * 10.0).round() % 10.0 == 0.0 {
                format!("~{}k", k as u64)
            } else {
                format!("~{:.1}k", k)
            }
        } else {
            format!("~{}", n)
        };
        let count_str = formatted;
        let x = area.x + area.width.saturating_sub(count_str.len() as u16);
        Paragraph::new(Line::from(vec![Span::styled(
            count_str,
            Style::default().fg(Color::DarkGray),
        )]))
        .render(
            Rect {
                x,
                y: text_start_y,
                width: area.width.saturating_sub(x.saturating_sub(area.x)),
                height: 1,
            },
            buf,
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- VimMode --------------------------------------------------------

    #[test]
    fn vim_mode_labels() {
        assert_eq!(VimMode::Insert.label(), "INSERT");
        assert_eq!(VimMode::Normal.label(), "NORMAL");
        assert_eq!(VimMode::Visual.label(), "VISUAL");
    }

    #[test]
    fn vim_insert_to_normal_via_escape() {
        let mut mode = VimMode::Insert;
        let mut text = "hello".to_string();
        let mut cursor = 3;
        let mut yank = String::new();
        apply_vim_command(&mut mode, &mut text, &mut cursor, "Escape", &mut yank);
        assert_eq!(mode, VimMode::Normal);
    }

    #[test]
    fn vim_normal_i_enters_insert() {
        let mut mode = VimMode::Normal;
        let mut text = "hello".to_string();
        let mut cursor = 0;
        let mut yank = String::new();
        apply_vim_command(&mut mode, &mut text, &mut cursor, "i", &mut yank);
        assert_eq!(mode, VimMode::Insert);
        assert_eq!(cursor, 0);
    }

    #[test]
    fn vim_normal_a_goes_to_end() {
        let mut mode = VimMode::Normal;
        let mut text = "hello".to_string();
        let mut cursor = 0;
        let mut yank = String::new();
        apply_vim_command(&mut mode, &mut text, &mut cursor, "A", &mut yank);
        assert_eq!(mode, VimMode::Insert);
        assert_eq!(cursor, 5);
    }

    #[test]
    fn vim_h_moves_left() {
        let mut mode = VimMode::Normal;
        let mut text = "hello".to_string();
        let mut cursor = 3;
        let mut yank = String::new();
        apply_vim_command(&mut mode, &mut text, &mut cursor, "h", &mut yank);
        assert_eq!(cursor, 2);
    }

    #[test]
    fn vim_l_moves_right() {
        let mut mode = VimMode::Normal;
        let mut text = "hello".to_string();
        let mut cursor = 2;
        let mut yank = String::new();
        apply_vim_command(&mut mode, &mut text, &mut cursor, "l", &mut yank);
        assert_eq!(cursor, 3);
    }

    #[test]
    fn vim_dollar_goes_to_end() {
        let mut mode = VimMode::Normal;
        let mut text = "hello".to_string();
        let mut cursor = 0;
        let mut yank = String::new();
        apply_vim_command(&mut mode, &mut text, &mut cursor, "$", &mut yank);
        assert_eq!(cursor, 5);
    }

    #[test]
    fn vim_zero_goes_to_start() {
        let mut mode = VimMode::Normal;
        let mut text = "hello".to_string();
        let mut cursor = 4;
        let mut yank = String::new();
        apply_vim_command(&mut mode, &mut text, &mut cursor, "0", &mut yank);
        assert_eq!(cursor, 0);
    }

    #[test]
    fn vim_x_deletes_char() {
        let mut mode = VimMode::Normal;
        let mut text = "hello".to_string();
        let mut cursor = 1;
        let mut yank = String::new();
        apply_vim_command(&mut mode, &mut text, &mut cursor, "x", &mut yank);
        assert_eq!(text, "hllo");
        assert_eq!(yank, "e");
    }

    #[test]
    fn vim_dd_clears_text() {
        let mut mode = VimMode::Normal;
        let mut text = "hello world".to_string();
        let mut cursor = 3;
        let mut yank = String::new();
        apply_vim_command(&mut mode, &mut text, &mut cursor, "dd", &mut yank);
        assert!(text.is_empty());
        assert_eq!(cursor, 0);
        assert_eq!(yank, "hello world");
    }

    #[test]
    fn vim_yy_copies_text() {
        let mut mode = VimMode::Normal;
        let mut text = "hello".to_string();
        let mut cursor = 0;
        let mut yank = String::new();
        apply_vim_command(&mut mode, &mut text, &mut cursor, "yy", &mut yank);
        assert_eq!(yank, "hello");
        assert_eq!(text, "hello"); // unchanged
    }

    #[test]
    fn vim_p_pastes_after_cursor() {
        let mut mode = VimMode::Normal;
        let mut text = "ab".to_string();
        let mut cursor = 0;
        let mut yank = "XY".to_string();
        apply_vim_command(&mut mode, &mut text, &mut cursor, "p", &mut yank);
        assert_eq!(text, "aXYb");
    }

    // ---- PromptInputState -----------------------------------------------

    #[test]
    fn insert_char_updates_cursor() {
        let mut s = PromptInputState::new();
        s.insert_char('h');
        s.insert_char('i');
        assert_eq!(s.text, "hi");
        assert_eq!(s.cursor, 2);
    }

    #[test]
    fn insert_newline_works() {
        let mut s = PromptInputState::new();
        s.insert_char('a');
        s.insert_newline();
        s.insert_char('b');
        assert_eq!(s.text, "a\nb");
    }

    #[test]
    fn backspace_removes_previous_char() {
        let mut s = PromptInputState::new();
        s.text = "hello".to_string();
        s.cursor = 5;
        s.backspace();
        assert_eq!(s.text, "hell");
        assert_eq!(s.cursor, 4);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut s = PromptInputState::new();
        s.text = "hi".to_string();
        s.cursor = 0;
        s.backspace();
        assert_eq!(s.text, "hi");
    }

    #[test]
    fn delete_removes_char_at_cursor() {
        let mut s = PromptInputState::new();
        s.text = "hello".to_string();
        s.cursor = 1;
        s.delete();
        assert_eq!(s.text, "hllo");
        assert_eq!(s.cursor, 1);
    }

    #[test]
    fn move_left_right() {
        let mut s = PromptInputState::new();
        s.text = "abc".to_string();
        s.cursor = 1;
        s.move_right();
        assert_eq!(s.cursor, 2);
        s.move_left();
        assert_eq!(s.cursor, 1);
    }

    #[test]
    fn readonly_blocks_insert() {
        let mut s = PromptInputState::new();
        s.mode = InputMode::Readonly;
        s.insert_char('x');
        assert!(s.text.is_empty());
    }

    #[test]
    fn history_navigation_up_down() {
        let mut s = PromptInputState::new();
        s.history = vec!["first".to_string(), "second".to_string()];
        s.history_up();
        assert_eq!(s.text, "second");
        s.history_up();
        assert_eq!(s.text, "first");
        s.history_down();
        assert_eq!(s.text, "second");
        s.history_down();
        assert_eq!(s.text, "");
        assert!(s.history_pos.is_none());
    }

    #[test]
    fn history_draft_restored() {
        let mut s = PromptInputState::new();
        s.text = "draft text".to_string();
        s.cursor = 10;
        s.history = vec!["old entry".to_string()];
        s.history_up();
        assert_eq!(s.text, "old entry");
        s.history_down();
        assert_eq!(s.text, "draft text");
    }

    #[test]
    fn clear_resets_state() {
        let mut s = PromptInputState::new();
        s.text = "something".to_string();
        s.cursor = 5;
        s.token_estimate = 10;
        s.clear();
        assert!(s.text.is_empty());
        assert_eq!(s.cursor, 0);
        assert_eq!(s.token_estimate, 0);
    }

    #[test]
    fn take_returns_and_clears() {
        let mut s = PromptInputState::new();
        s.text = "hello".to_string();
        s.cursor = 5;
        let taken = s.take();
        assert_eq!(taken, "hello");
        assert!(s.text.is_empty());
    }

    #[test]
    fn is_empty_trims_whitespace() {
        let mut s = PromptInputState::new();
        s.text = "   \n  ".to_string();
        assert!(s.is_empty());
        s.text = "  x  ".to_string();
        assert!(!s.is_empty());
    }

    // ---- handle_paste ---------------------------------------------------

    #[test]
    fn paste_small_content_inline() {
        let mut counter = 0u32;
        let (result, stored) = handle_paste("short text", &mut counter);
        assert_eq!(result, "short text");
        assert!(stored.is_none());
        assert_eq!(counter, 0);
    }

    #[test]
    fn paste_large_content_placeholder() {
        let mut counter = 0u32;
        // >150 chars → triggers placeholder
        let big = "x".repeat(200);
        let (result, stored) = handle_paste(&big, &mut counter);
        assert!(result.starts_with("[Pasted ~"), "expected placeholder, got: {result}");
        assert!(result.contains("#1"), "expected counter in placeholder, got: {result}");
        assert!(stored.is_some());
        assert_eq!(counter, 1);
    }

    #[test]
    fn paste_large_multiline_placeholder() {
        let mut counter = 0u32;
        // ≥3 lines → triggers placeholder regardless of length
        let big = "line\n".repeat(300);
        let (result, stored) = handle_paste(&big, &mut counter);
        assert!(result.starts_with("[Pasted ~"), "expected placeholder, got: {result}");
        assert!(result.contains("lines"), "expected line count in placeholder, got: {result}");
        assert!(stored.is_some());
    }

    #[test]
    fn paste_three_lines_triggers_placeholder() {
        let mut counter = 0u32;
        // Exactly 3 lines (the threshold) should use a placeholder.
        let three_lines = "a\nb\nc";
        let (result, stored) = handle_paste(three_lines, &mut counter);
        assert!(result.starts_with("[Pasted ~"), "3-line paste should be placeholder, got: {result}");
        assert!(stored.is_some());
    }

    #[test]
    fn paste_two_lines_inline() {
        let mut counter = 0u32;
        // 2 lines, ≤150 chars → inserted verbatim
        let two_lines = "hello\nworld";
        let (result, stored) = handle_paste(two_lines, &mut counter);
        assert_eq!(result, two_lines);
        assert!(stored.is_none());
    }

    #[test]
    fn paste_counter_increments() {
        let mut counter = 0u32;
        let big = "x".repeat(2000);
        handle_paste(&big, &mut counter);
        handle_paste(&big, &mut counter);
        assert_eq!(counter, 2);
    }

    // ---- compute_typeahead ---------------------------------------------

    #[test]
    fn typeahead_slash_prefix_matches() {
        let cmds = [("help", "Show help"), ("history", "Show history"), ("compact", "Compact")];
        let suggestions = compute_typeahead("/h", &cmds);
        assert_eq!(suggestions.len(), 2);
        assert_eq!(suggestions[0].text, "/help");
        assert_eq!(suggestions[1].text, "/history");
    }

    #[test]
    fn typeahead_no_slash_returns_empty() {
        let cmds = [("help", "Show help")];
        let suggestions = compute_typeahead("hello", &cmds);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn typeahead_full_match() {
        let cmds = [("compact", "Compact conversation")];
        let suggestions = compute_typeahead("/compact", &cmds);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].text, "/compact");
        assert_eq!(suggestions[0].description, "Compact conversation");
    }

    #[test]
    fn typeahead_case_insensitive() {
        let cmds = [("Help", "Show help")];
        let suggestions = compute_typeahead("/H", &cmds);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].text, "/Help");
    }

    // ---- suggestion navigation -----------------------------------------

    #[test]
    fn suggestion_next_cycles() {
        let mut s = PromptInputState::new();
        let cmds = [("help", "Help"), ("history", "History"), ("compact", "Compact")];
        s.text = "/h".to_string();
        s.update_suggestions(&cmds);
        assert_eq!(s.suggestions.len(), 2);
        assert_eq!(s.suggestion_index, Some(0));
        s.suggestion_next();
        assert_eq!(s.suggestion_index, Some(1));
        s.suggestion_next();
        assert_eq!(s.suggestion_index, Some(0)); // wraps
    }

    #[test]
    fn accept_suggestion_fills_text() {
        let mut s = PromptInputState::new();
        let cmds = [("help", "Show help")];
        s.text = "/he".to_string();
        s.update_suggestions(&cmds);
        s.suggestion_next();
        s.accept_suggestion();
        assert_eq!(s.text, "/help");
        assert_eq!(s.cursor, 5);
        assert!(s.suggestions.is_empty());
    }

    // ---- token estimate -------------------------------------------------

    #[test]
    fn token_estimate_rough() {
        let mut s = PromptInputState::new();
        for _ in 0..40 {
            s.insert_char('a');
        }
        // 40 chars / 4 = 10 tokens
        assert_eq!(s.token_estimate, 10);
    }

    // ---- motion_w / motion_b -----------------------------------------------

    #[test]
    fn motion_w_basic() {
        assert_eq!(motion_w("hello world", 0), 6);
        assert_eq!(motion_w("hello world", 6), 11); // at start of 'world', moves to end
        assert_eq!(motion_w("  foo", 0), 2);         // skip leading spaces
    }

    #[test]
    fn motion_b_basic() {
        assert_eq!(motion_b("hello world", 6), 0); // 'w' → start of 'hello'
        assert_eq!(motion_b("hello world", 0), 0); // already at start
    }

    #[test]
    fn motion_e_basic() {
        assert_eq!(motion_e("hello world", 0), 4);  // cursor on 'h', end at 'o'
        assert_eq!(motion_e("hello world", 4), 10); // at 'o' (end), jump to 'd'
    }

    #[test]
    fn motion_W_B_basic() {
        // "foo.bar baz"  W from 0 → 8 ('b' of 'baz')
        assert_eq!(motion_W("foo.bar baz", 0), 8);
        assert_eq!(motion_B("foo.bar baz", 8), 0);
    }

    #[test]
    fn motion_E_basic() {
        assert_eq!(motion_E("foo.bar baz", 0), 6); // end of 'foo.bar' WORD
    }

    #[test]
    fn motion_first_nonblank_basic() {
        assert_eq!(motion_first_nonblank("  hello", 0), 2);
        assert_eq!(motion_first_nonblank("hello", 0), 0);
    }

    #[test]
    fn motion_G_basic() {
        assert_eq!(motion_G("foo\nbar"), 4);
        assert_eq!(motion_G("single line"), 0);
    }

    #[test]
    fn motion_gg_basic() {
        assert_eq!(motion_gg("foo\nbar\nbaz", 1), 0);
        assert_eq!(motion_gg("foo\nbar\nbaz", 2), 4);
        assert_eq!(motion_gg("foo\nbar\nbaz", 3), 8);
    }

    #[test]
    fn motion_find_char_f() {
        // f: cursor lands on 'o', count=1
        assert_eq!(motion_find_char("hello", 0, 'o', VimFindKind::F, 1), Some(4));
        // f: not found
        assert_eq!(motion_find_char("hello", 0, 'z', VimFindKind::F, 1), None);
    }

    #[test]
    fn motion_find_char_t() {
        // t: cursor stops before 'o'
        assert_eq!(motion_find_char("hello", 0, 'o', VimFindKind::T, 1), Some(3));
    }

    #[test]
    fn motion_find_char_bigF() {
        // F: search backward
        assert_eq!(motion_find_char("hello", 4, 'h', VimFindKind::BigF, 1), Some(0));
    }

    // ---- apply_vim_key new commands ----------------------------------------

    #[test]
    fn vim_key_e_motion() {
        let mut mode = VimMode::Normal;
        let mut text = "hello world".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "e", &mut yank, &mut pending, &mut last_find);
        assert_eq!(cursor, 4); // end of 'hello'
    }

    #[test]
    fn vim_key_W_motion() {
        let mut mode = VimMode::Normal;
        let mut text = "foo.bar baz".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "W", &mut yank, &mut pending, &mut last_find);
        assert_eq!(cursor, 8); // 'baz'
    }

    #[test]
    fn vim_key_G_last_line() {
        let mut mode = VimMode::Normal;
        let mut text = "first\nsecond\nthird".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "G", &mut yank, &mut pending, &mut last_find);
        assert_eq!(cursor, 13); // start of 'third'
    }

    #[test]
    fn vim_key_gg_first_line() {
        let mut mode = VimMode::Normal;
        let mut text = "first\nsecond".to_string();
        let mut cursor = 6usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        // 'g' sets pending G
        apply_vim_key(&mut mode, &mut text, &mut cursor, "g", &mut yank, &mut pending, &mut last_find);
        assert!(matches!(pending, VimPendingState::G { .. }));
        apply_vim_key(&mut mode, &mut text, &mut cursor, "g", &mut yank, &mut pending, &mut last_find);
        assert_eq!(cursor, 0);
    }

    #[test]
    fn vim_key_count_motion() {
        let mut mode = VimMode::Normal;
        let mut text = "a b c d e".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        // 3w — advance 3 words
        apply_vim_key(&mut mode, &mut text, &mut cursor, "3", &mut yank, &mut pending, &mut last_find);
        assert!(matches!(pending, VimPendingState::Count { .. }));
        apply_vim_key(&mut mode, &mut text, &mut cursor, "w", &mut yank, &mut pending, &mut last_find);
        assert_eq!(cursor, 6); // 3 words forward: a→b→c→d start = pos 6
    }

    #[test]
    fn vim_key_dw_delete_word() {
        let mut mode = VimMode::Normal;
        let mut text = "hello world".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "d", &mut yank, &mut pending, &mut last_find);
        assert!(matches!(pending, VimPendingState::Operator { op: VimOperator::Delete, .. }));
        apply_vim_key(&mut mode, &mut text, &mut cursor, "w", &mut yank, &mut pending, &mut last_find);
        assert_eq!(text, "world");
        assert_eq!(yank, "hello ");
    }

    #[test]
    fn vim_key_cw_change_word_enters_insert() {
        let mut mode = VimMode::Normal;
        let mut text = "hello world".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "c", &mut yank, &mut pending, &mut last_find);
        apply_vim_key(&mut mode, &mut text, &mut cursor, "w", &mut yank, &mut pending, &mut last_find);
        assert_eq!(mode, VimMode::Insert);
        assert_eq!(text, "world");
    }

    #[test]
    fn vim_key_dd_deletes_line() {
        let mut mode = VimMode::Normal;
        let mut text = "first\nsecond".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "d", &mut yank, &mut pending, &mut last_find);
        apply_vim_key(&mut mode, &mut text, &mut cursor, "d", &mut yank, &mut pending, &mut last_find);
        assert_eq!(text, "second");
        assert_eq!(yank, "first\n");
    }

    #[test]
    fn vim_key_r_replace_char() {
        let mut mode = VimMode::Normal;
        let mut text = "hello".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "r", &mut yank, &mut pending, &mut last_find);
        assert!(matches!(pending, VimPendingState::Replace { .. }));
        apply_vim_key(&mut mode, &mut text, &mut cursor, "H", &mut yank, &mut pending, &mut last_find);
        assert_eq!(text, "Hello");
        assert_eq!(mode, VimMode::Normal); // stays in Normal after replace
    }

    #[test]
    fn vim_key_find_f() {
        let mut mode = VimMode::Normal;
        let mut text = "hello world".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "f", &mut yank, &mut pending, &mut last_find);
        apply_vim_key(&mut mode, &mut text, &mut cursor, "o", &mut yank, &mut pending, &mut last_find);
        assert_eq!(cursor, 4); // first 'o' in 'hello'
        assert_eq!(last_find, Some((VimFindKind::F, 'o')));
    }

    #[test]
    fn vim_key_semicolon_repeat_find() {
        let mut mode = VimMode::Normal;
        let mut text = "a.b.c".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "f", &mut yank, &mut pending, &mut last_find);
        apply_vim_key(&mut mode, &mut text, &mut cursor, ".", &mut yank, &mut pending, &mut last_find);
        assert_eq!(cursor, 1);
        apply_vim_key(&mut mode, &mut text, &mut cursor, ";", &mut yank, &mut pending, &mut last_find);
        assert_eq!(cursor, 3); // repeated find → next '.'
    }

    #[test]
    fn vim_key_X_delete_before_cursor() {
        let mut mode = VimMode::Normal;
        let mut text = "hello".to_string();
        let mut cursor = 4usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "X", &mut yank, &mut pending, &mut last_find);
        assert_eq!(text, "helo");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn vim_key_tilde_toggle_case() {
        let mut mode = VimMode::Normal;
        let mut text = "hello".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "~", &mut yank, &mut pending, &mut last_find);
        assert_eq!(text, "Hello");
    }

    #[test]
    fn vim_key_o_open_line_below() {
        let mut mode = VimMode::Normal;
        let mut text = "first\nthird".to_string();
        let mut cursor = 0usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "o", &mut yank, &mut pending, &mut last_find);
        assert_eq!(mode, VimMode::Insert);
        assert!(text.contains('\n'));
        assert_eq!(cursor, 6); // after first newline
    }

    #[test]
    fn vim_key_D_delete_to_eol() {
        let mut mode = VimMode::Normal;
        let mut text = "hello world".to_string();
        let mut cursor = 6usize;
        let mut yank = String::new();
        let mut pending = VimPendingState::None;
        let mut last_find = None;
        apply_vim_key(&mut mode, &mut text, &mut cursor, "D", &mut yank, &mut pending, &mut last_find);
        assert_eq!(text, "hello ");
        assert_eq!(yank, "world");
    }

    // ---- PromptInputState undo ---------------------------------------------

    #[test]
    fn prompt_input_undo_restores_text() {
        let mut s = PromptInputState::new();
        s.vim_enabled = true;
        s.vim_mode = VimMode::Normal;
        s.text = "hello".to_string();
        s.cursor = 5;
        s.vim_command("x"); // deletes 'o' (but cursor at 5 = past end)
        // let's set cursor to 4 and delete
        s.cursor = 4;
        s.vim_command("x");
        assert_eq!(s.text, "hell");
        s.vim_command("u");
        assert_eq!(s.text, "hello");
    }

    #[test]
    fn prompt_input_visual_yank() {
        let mut s = PromptInputState::new();
        s.vim_enabled = true;
        s.vim_mode = VimMode::Normal;
        s.text = "hello world".to_string();
        s.cursor = 0;
        s.vim_command("v");
        assert_eq!(s.vim_mode, VimMode::Visual);
        // Move to end of word
        s.vim_command("e");
        s.vim_command("y"); // yank selection
        assert_eq!(s.yank_buf, "hello");
        assert_eq!(s.vim_mode, VimMode::Normal);
    }

    // ---- Named registers ------------------------------------------------

    #[test]
    fn register_yank_and_paste() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "hello world".to_string();
        s.cursor = 0;
        // `"ay` — yank line to register 'a'
        s.vim_command("\"");
        s.vim_command("a");
        s.vim_command("y");
        assert_eq!(s.vim_registers.get(&'a').map(|s| s.as_str()), Some("hello world"));
        // `"ap` — paste from register 'a' after cursor
        s.cursor = 0;
        s.vim_command("\"");
        s.vim_command("a");
        s.vim_command("p");
        assert!(s.text.contains("hello world"));
    }

    #[test]
    fn register_yank_method() {
        let mut s = PromptInputState::new();
        s.yank_to_register('b', "some text");
        assert_eq!(s.paste_from_register('b'), Some("some text".to_string()));
        assert_eq!(s.paste_from_register('z'), None);
    }

    #[test]
    fn register_delete_to_named() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "hello\nworld".to_string();
        s.cursor = 0;
        // `"ad` — delete line to register 'a'
        s.vim_command("\"");
        s.vim_command("a");
        s.vim_command("d");
        assert_eq!(s.vim_registers.get(&'a').map(|s| s.as_str()), Some("hello\n"));
        assert_eq!(s.text, "world");
    }

    // ---- Marks ----------------------------------------------------------

    #[test]
    fn mark_set_and_jump() {
        let mut s = PromptInputState::new();
        s.text = "hello world".to_string();
        s.cursor = 6; // at 'w'
        s.set_mark('a');
        s.cursor = 0;
        s.jump_to_mark('a');
        assert_eq!(s.cursor, 6);
    }

    #[test]
    fn mark_jump_nonexistent_is_noop() {
        let mut s = PromptInputState::new();
        s.text = "hello".to_string();
        s.cursor = 3;
        s.jump_to_mark('z'); // no mark 'z' set
        assert_eq!(s.cursor, 3);
    }

    #[test]
    fn mark_via_vim_command() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "hello world".to_string();
        s.cursor = 6;
        // `ma` — set mark 'a'
        s.vim_command("m");
        s.vim_command("a");
        assert!(s.vim_marks.contains_key(&'a'));
        // Move cursor and jump back with `'a`
        s.cursor = 0;
        s.vim_command("'");
        s.vim_command("a");
        assert_eq!(s.cursor, 6);
    }

    #[test]
    fn mark_clamped_when_text_shortened() {
        let mut s = PromptInputState::new();
        s.text = "hello world".to_string();
        s.cursor = 10;
        s.set_mark('x');
        // Shorten the text
        s.text = "hi".to_string();
        s.cursor = 0;
        s.jump_to_mark('x');
        // Should clamp to text length
        assert!(s.cursor <= s.text.len());
        assert!(s.text.is_char_boundary(s.cursor));
    }

    // ---- Macro recording ------------------------------------------------

    #[test]
    fn macro_record_and_replay() {
        let mut s = PromptInputState::new();
        // Start recording into register 'q'
        s.start_macro_recording('q');
        assert_eq!(s.vim_macro_recording, Some('q'));
        // Simulate accumulating keys
        s.vim_macro_content.get_mut(&'q').unwrap().push("w".to_string());
        s.vim_macro_content.get_mut(&'q').unwrap().push("e".to_string());
        // Stop recording
        let reg = s.stop_macro_recording();
        assert_eq!(reg, Some('q'));
        assert_eq!(s.vim_macro_recording, None);
        // Replay
        let keys = s.replay_macro('q');
        assert_eq!(keys, vec!["w".to_string(), "e".to_string()]);
    }

    #[test]
    fn macro_replay_empty_register() {
        let s = PromptInputState::new();
        let keys = s.replay_macro('z');
        assert!(keys.is_empty());
    }

    #[test]
    fn macro_via_vim_command() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "abc".to_string();
        s.cursor = 0;
        // `qq` — start recording into 'q'
        s.vim_command("q");
        assert!(matches!(s.vim_pending, VimPendingState::MacroRecord));
        s.vim_command("q"); // register name = 'q'
        assert_eq!(s.vim_macro_recording, Some('q'));
        // Record some keys: move right twice
        s.vim_command("l");
        s.vim_command("l");
        // Stop recording with `q`
        s.vim_command("q");
        assert_eq!(s.vim_macro_recording, None);
        // The recorded content should have 'l', 'l'
        let keys = s.replay_macro('q');
        assert_eq!(keys, vec!["l".to_string(), "l".to_string()]);
    }

    #[test]
    fn macro_replay_via_at() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "abcdef".to_string();
        s.cursor = 0;
        // Manually record a macro: move 2 chars right
        s.vim_macro_content.insert('q', vec!["l".to_string(), "l".to_string()]);
        // `@q` — replay macro 'q'
        s.vim_command("@");
        assert!(matches!(s.vim_pending, VimPendingState::MacroReplay));
        s.vim_command("q");
        // cursor should have moved right by 2
        assert_eq!(s.cursor, 2);
    }

    // ---- Dot-repeat -----------------------------------------------------

    #[test]
    fn dot_repeat_delete_char() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "hello".to_string();
        s.cursor = 0;
        // Delete char at cursor with `x`
        s.vim_command("x");
        assert_eq!(s.text, "ello");
        // Dot-repeat should delete again
        s.vim_command(".");
        assert_eq!(s.text, "llo");
    }

    #[test]
    fn dot_repeat_replace_char() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "hello".to_string();
        s.cursor = 0;
        // Replace 'h' with 'H' using `r`
        s.vim_command("r");
        s.vim_command("H");
        assert_eq!(s.text, "Hello");
        // Move and dot-repeat: should replace 'e' with 'H'
        s.vim_command("l");
        s.vim_command(".");
        assert_eq!(s.text, "HHllo");
    }

    #[test]
    fn dot_repeat_noop_when_no_action() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "hello".to_string();
        s.cursor = 0;
        // `.` with no prior modifying action should be a no-op
        s.vim_command(".");
        assert_eq!(s.text, "hello");
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn dot_repeat_after_visual_delete() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "hello world".to_string();
        s.cursor = 0;
        // Enter visual, select 'hel', then delete
        s.vim_command("v");
        s.vim_command("l");
        s.vim_command("l");
        s.vim_command("d");
        assert_eq!(s.text, "lo world");
        // Dot-repeat should delete chars again
        s.vim_command(".");
        // The text should be shorter
        assert!(s.text.len() < "lo world".len());
    }

    // ---- Visual line mode (V) -------------------------------------------

    #[test]
    fn visual_line_mode_enter() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "line one\nline two".to_string();
        s.cursor = 0;
        s.vim_command("V");
        assert_eq!(s.vim_mode, VimMode::VisualLine);
        assert!(s.visual_anchor.is_some());
    }

    #[test]
    fn visual_line_yank() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "line one\nline two".to_string();
        s.cursor = 0;
        s.vim_command("V");
        s.vim_command("y");
        assert_eq!(s.vim_mode, VimMode::Normal);
        assert_eq!(s.yank_buf, "line one\n");
    }

    #[test]
    fn visual_line_delete() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "line one\nline two".to_string();
        s.cursor = 0;
        s.vim_command("V");
        s.vim_command("d");
        assert_eq!(s.vim_mode, VimMode::Normal);
        assert_eq!(s.text, "line two");
    }

    #[test]
    fn visual_line_escape_returns_normal() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "hello".to_string();
        s.vim_command("V");
        assert_eq!(s.vim_mode, VimMode::VisualLine);
        s.vim_command("Escape");
        assert_eq!(s.vim_mode, VimMode::Normal);
    }

    // ---- Command-line mode (:) ------------------------------------------

    #[test]
    fn command_line_mode_enter() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.vim_command(":");
        assert_eq!(s.vim_mode, VimMode::Command);
        assert!(s.vim_command_buf.is_empty());
    }

    #[test]
    fn command_line_accumulates_chars() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.vim_command(":");
        s.vim_command("q");
        assert_eq!(s.vim_command_buf, "q");
        s.vim_command("!");
        assert_eq!(s.vim_command_buf, "q!");
    }

    #[test]
    fn command_line_backspace_pops() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.vim_command(":");
        s.vim_command("q");
        s.vim_command("w");
        s.vim_command("Backspace");
        assert_eq!(s.vim_command_buf, "q");
    }

    #[test]
    fn command_line_empty_backspace_cancels() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.vim_command(":");
        s.vim_command("Backspace");
        assert_eq!(s.vim_mode, VimMode::Normal);
    }

    #[test]
    fn command_q_sets_quit_flag() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.vim_command(":");
        s.vim_command("q");
        s.vim_command("Enter");
        assert!(s.vim_quit_requested);
        assert_eq!(s.vim_mode, VimMode::Normal);
    }

    #[test]
    fn command_noh_clears_search() {
        let mut s = PromptInputState::new();
        s.vim_search_last = Some("foo".to_string());
        s.vim_mode = VimMode::Normal;
        s.vim_command(":");
        for c in "noh".chars() {
            s.vim_command(&c.to_string());
        }
        s.vim_command("Enter");
        assert!(s.vim_search_last.is_none());
    }

    #[test]
    fn command_escape_cancels() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.vim_command(":");
        s.vim_command("q");
        s.vim_command("Escape");
        assert_eq!(s.vim_mode, VimMode::Normal);
    }

    // ---- In-prompt search (/) -------------------------------------------

    #[test]
    fn search_mode_enter() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.vim_command("/");
        assert_eq!(s.vim_mode, VimMode::Search);
        assert!(s.vim_search_buf.is_empty());
    }

    #[test]
    fn search_finds_match_and_moves_cursor() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "hello world hello".to_string();
        s.cursor = 0;
        s.vim_command("/");
        for c in "world".chars() {
            s.vim_command(&c.to_string());
        }
        s.vim_command("Enter");
        assert_eq!(s.vim_mode, VimMode::Normal);
        assert_eq!(s.cursor, 6); // "world" starts at byte 6
        assert_eq!(s.vim_search_last.as_deref(), Some("world"));
    }

    #[test]
    fn search_n_finds_next() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "aa bb aa".to_string();
        s.cursor = 0;
        s.vim_command("/");
        s.vim_command("a");
        s.vim_command("a");
        s.vim_command("Enter");
        assert_eq!(s.cursor, 0); // first 'aa'
        s.vim_command("n");
        assert_eq!(s.cursor, 6); // second 'aa'
    }

    #[test]
    fn search_N_finds_prev() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.text = "aa bb aa".to_string();
        s.cursor = 7; // at second 'aa'
        s.vim_search_last = Some("aa".to_string());
        s.vim_command("N");
        assert_eq!(s.cursor, 0); // wraps to first 'aa'
    }

    #[test]
    fn search_escape_cancels() {
        let mut s = PromptInputState::new();
        s.vim_mode = VimMode::Normal;
        s.vim_command("/");
        s.vim_command("f");
        s.vim_command("Escape");
        assert_eq!(s.vim_mode, VimMode::Normal);
    }

    // ---- VimMode labels -------------------------------------------------

    #[test]
    fn vim_mode_new_labels() {
        assert_eq!(VimMode::VisualLine.label(), "VISUAL LINE");
        assert_eq!(VimMode::Command.label(), "COMMAND");
        assert_eq!(VimMode::Search.label(), "SEARCH");
    }
}
