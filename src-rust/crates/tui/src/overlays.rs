// overlays.rs — All full-screen and floating overlays:
//   - HelpOverlay (? / F1 / /help)
//   - HistorySearchOverlay (Ctrl+R)
//   - MessageSelectorOverlay (/rewind step 1)
//   - RewindFlowOverlay (/rewind full multi-step flow)

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub const CLAURST_ACCENT: Color = Color::Rgb(233, 30, 99);
pub const CLAURST_PANEL_BG: Color = Color::Rgb(20, 20, 28);
pub const CLAURST_PANEL_BORDER: Color = Color::Rgb(72, 72, 80);
pub const CLAURST_TEXT: Color = Color::Rgb(235, 235, 240);
pub const CLAURST_MUTED: Color = Color::Rgb(110, 110, 118);
pub const CLAURST_OVERLAY_BG: Color = Color::Rgb(10, 10, 14);

// ---------------------------------------------------------------------------
// Geometry helper (shared)
// ---------------------------------------------------------------------------

/// Compute a centred `Rect` of the given `width` × `height` inside `area`.
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

// ---------------------------------------------------------------------------
// Reusable overlay helpers (shared by all dialog renderers)
// ---------------------------------------------------------------------------

/// Darken the entire screen with a semi-transparent overlay.
/// Call this BEFORE rendering any dialog content.
pub fn render_dark_overlay(frame: &mut Frame, area: Rect) {
    render_dark_overlay_buf(frame.buffer_mut(), area);
}

pub fn render_dark_overlay_buf(buf: &mut Buffer, area: Rect) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(CLAURST_OVERLAY_BG);
                cell.set_fg(CLAURST_MUTED);
            }
        }
    }
}

/// Fill a rectangle with the standard dialog background color (no border).
pub fn render_dialog_bg(frame: &mut Frame, area: Rect) {
    render_dialog_bg_buf(frame.buffer_mut(), area);
}

pub fn render_dialog_bg_buf(buf: &mut Buffer, area: Rect) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(' ');
                cell.set_bg(CLAURST_PANEL_BG);
                cell.set_fg(CLAURST_TEXT);
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ModalLayout {
    pub dialog_area: Rect,
    pub inner_area: Rect,
    pub header_area: Rect,
    pub body_area: Rect,
    pub footer_area: Rect,
}

fn compute_modal_layout(
    area: Rect,
    width: u16,
    height: u16,
    header_height: u16,
    footer_height: u16,
) -> ModalLayout {
    let dialog_width = width.min(area.width.saturating_sub(4)).max(8);
    let dialog_height = height.min(area.height.saturating_sub(4)).max(6);
    let dialog_area = centered_rect(dialog_width, dialog_height, area);
    let inner_area = Rect {
        x: dialog_area.x + 1,
        y: dialog_area.y + 1,
        width: dialog_area.width.saturating_sub(2),
        height: dialog_area.height.saturating_sub(2),
    };
    let header_h = header_height.min(inner_area.height);
    let footer_h = footer_height.min(inner_area.height.saturating_sub(header_h));
    let body_area = Rect {
        x: inner_area.x,
        y: inner_area.y.saturating_add(header_h),
        width: inner_area.width,
        height: inner_area.height.saturating_sub(header_h + footer_h),
    };
    ModalLayout {
        dialog_area,
        inner_area,
        header_area: Rect {
            x: inner_area.x,
            y: inner_area.y,
            width: inner_area.width,
            height: header_h,
        },
        body_area,
        footer_area: Rect {
            x: inner_area.x,
            y: inner_area.y + inner_area.height.saturating_sub(footer_h),
            width: inner_area.width,
            height: footer_h,
        },
    }
}

pub fn begin_modal_frame(
    frame: &mut Frame,
    area: Rect,
    width: u16,
    height: u16,
    header_height: u16,
    footer_height: u16,
) -> ModalLayout {
    let layout = compute_modal_layout(area, width, height, header_height, footer_height);
    render_dark_overlay(frame, area);
    frame.render_widget(Clear, layout.dialog_area);
    render_dialog_bg(frame, layout.dialog_area);
    layout
}

pub fn begin_modal_buf(
    buf: &mut Buffer,
    area: Rect,
    width: u16,
    height: u16,
    header_height: u16,
    footer_height: u16,
) -> ModalLayout {
    let layout = compute_modal_layout(area, width, height, header_height, footer_height);
    render_dark_overlay_buf(buf, area);
    Clear.render(layout.dialog_area, buf);
    render_dialog_bg_buf(buf, layout.dialog_area);
    layout
}

pub fn modal_title_line(title: &str, right_hint: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {}", title),
            Style::default()
                .fg(CLAURST_TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", right_hint),
            Style::default().fg(CLAURST_MUTED),
        ),
    ])
}

pub fn render_modal_title_frame(frame: &mut Frame, area: Rect, title: &str, right_hint: &str) {
    if area.height == 0 {
        return;
    }
    let title_width = UnicodeWidthStr::width(title);
    let hint_width = UnicodeWidthStr::width(right_hint);
    let padding = area
        .width
        .saturating_sub((title_width + hint_width + 3) as u16) as usize;
    let line = Line::from(vec![
        Span::styled(
            format!(" {}", title),
            Style::default()
                .fg(CLAURST_TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(padding), Style::default().fg(CLAURST_TEXT)),
        Span::styled(right_hint.to_string(), Style::default().fg(CLAURST_MUTED)),
    ]);
    frame.render_widget(
        Paragraph::new(line),
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
    );
}

pub fn render_modal_title_buf(buf: &mut Buffer, area: Rect, title: &str, right_hint: &str) {
    if area.height == 0 {
        return;
    }
    let title_width = UnicodeWidthStr::width(title);
    let hint_width = UnicodeWidthStr::width(right_hint);
    let padding = area
        .width
        .saturating_sub((title_width + hint_width + 3) as u16) as usize;
    let line = Line::from(vec![
        Span::styled(
            format!(" {}", title),
            Style::default()
                .fg(CLAURST_TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ".repeat(padding), Style::default().fg(CLAURST_TEXT)),
        Span::styled(right_hint.to_string(), Style::default().fg(CLAURST_MUTED)),
    ]);
    Paragraph::new(line).render(
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
        buf,
    );
}

pub fn modal_header_line_area(header_area: Rect, row: u16) -> Option<Rect> {
    if header_area.height <= row {
        return None;
    }
    Some(Rect {
        x: header_area.x,
        y: header_area.y + row,
        width: header_area.width,
        height: 1,
    })
}

pub fn modal_search_line(
    query: &str,
    placeholder: &str,
    placeholder_color: Color,
    query_color: Color,
) -> Line<'static> {
    if query.is_empty() {
        let mut chars = placeholder.chars();
        let first = chars.next().unwrap_or(' ');
        let rest: String = chars.collect();
        Line::from(vec![
            Span::styled(" ", Style::default().fg(placeholder_color)),
            Span::styled(
                first.to_string(),
                Style::default()
                    .fg(placeholder_color)
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Span::styled(rest, Style::default().fg(placeholder_color)),
        ])
    } else {
        Line::from(vec![Span::styled(
            format!(" {}", query),
            Style::default().fg(query_color),
        )])
    }
}

// ============================================================================
// HelpOverlay
// ============================================================================

/// State for the full-screen help overlay (? / F1 / /help).
#[derive(Debug, Default)]
pub struct HelpOverlay {
    pub visible: bool,
    pub scroll_offset: u16,
    /// Live search filter — only commands matching this substring are shown.
    pub filter: String,
    /// Dynamically populated entries from the command registry.
    pub commands: Vec<HelpEntry>,
}

/// A single command entry shown in the help overlay.
#[derive(Debug, Clone)]
pub struct HelpEntry {
    pub name: String,
    /// Comma-separated aliases, e.g. "h, ?"
    pub aliases: String,
    pub description: String,
    pub category: String,
}

impl HelpOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Populate (or replace) the command entries from the command registry.
    /// Entries are sorted by category then name.
    pub fn populate_from_commands(&mut self, entries: Vec<HelpEntry>) {
        self.commands = entries;
        // Sort stable by category, then name for consistent display.
        self.commands
            .sort_by(|a, b| a.category.cmp(&b.category).then(a.name.cmp(&b.name)));
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        if !self.visible {
            // Reset state when closing
            self.scroll_offset = 0;
            self.filter.clear();
        }
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.scroll_offset = 0;
        self.filter.clear();
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_down(&mut self, max: u16) {
        if self.scroll_offset + 1 < max {
            self.scroll_offset += 1;
        }
    }

    pub fn push_filter_char(&mut self, c: char) {
        self.filter.push(c);
        self.scroll_offset = 0;
    }

    pub fn pop_filter_char(&mut self) {
        self.filter.pop();
        self.scroll_offset = 0;
    }
}

/// Render the help overlay into the frame.
pub fn render_help_overlay(frame: &mut Frame, overlay: &HelpOverlay, area: Rect) {
    use claurst_core::constants::APP_VERSION;
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::widgets::Wrap;

    if !overlay.visible {
        return;
    }

    let layout = begin_modal_frame(frame, area, 100, 36, 3, 1);
    render_modal_title_frame(frame, layout.header_area, "Shortcuts & commands", "esc");
    let search_line = modal_search_line(
        &overlay.filter,
        "Search shortcuts or commands",
        CLAURST_MUTED,
        CLAURST_TEXT,
    );
    if let Some(search_area) = modal_header_line_area(layout.header_area, 2) {
        frame.render_widget(Paragraph::new(search_line), search_area);
    }

    let content_area = layout.body_area;
    if content_area.height == 0 {
        return;
    }

    let col_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(42),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(content_area);

    // ─── Left column: keyboard shortcuts by category ───────────────────────
    let mut left_lines: Vec<Line<'static>> = Vec::new();

    left_lines.push(Line::from(Span::styled(
        " Keyboard Shortcuts",
        Style::default()
            .fg(CLAURST_ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
    left_lines.push(Line::from(""));

    // Navigation category
    left_lines.push(Line::from(Span::styled(
        " Navigation",
        Style::default()
            .fg(CLAURST_ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
    for (key, desc) in &[
        ("PageUp / PgDn", "Scroll messages"),
        ("j / k", "Scroll one line"),
        ("Home / End", "Top / bottom"),
    ] {
        left_lines.push(kb_line(key, desc));
    }
    left_lines.push(Line::from(""));

    // Input category
    left_lines.push(Line::from(Span::styled(
        " Input",
        Style::default()
            .fg(CLAURST_ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
    for (key, desc) in &[
        ("Enter", "Submit message"),
        ("Up / Down", "Input history"),
        ("Ctrl+R", "Search history"),
        ("Esc", "Cancel / close"),
    ] {
        left_lines.push(kb_line(key, desc));
    }
    left_lines.push(Line::from(""));

    // App category
    left_lines.push(Line::from(Span::styled(
        " App",
        Style::default()
            .fg(CLAURST_ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
    for (key, desc) in &[
        ("F1 / ?", "Toggle help"),
        ("Ctrl+A", "Model picker"),
        ("Ctrl+K", "Command palette"),
        ("Ctrl+C", "Cancel / quit"),
        ("Ctrl+D", "Quit (empty input)"),
        ("Ctrl+L", "Clear screen"),
    ] {
        left_lines.push(kb_line(key, desc));
    }

    frame.render_widget(
        Paragraph::new(left_lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(CLAURST_PANEL_BG)),
        col_chunks[0],
    );

    // ─── Center divider ────────────────────────────────────────────────────
    let divider_lines: Vec<Line<'static>> = (0..content_area.height)
        .map(|_| Line::from(Span::styled("\u{2502}", Style::default().fg(CLAURST_MUTED))))
        .collect();
    frame.render_widget(Paragraph::new(divider_lines), col_chunks[1]);

    // ─── Right column: slash commands by category ──────────────────────────
    let filter_lc = overlay.filter.to_lowercase();
    let filtered: Vec<&HelpEntry> = overlay
        .commands
        .iter()
        .filter(|e| {
            filter_lc.is_empty()
                || e.name.to_lowercase().contains(filter_lc.as_str())
                || e.aliases.to_lowercase().contains(filter_lc.as_str())
                || e.description.to_lowercase().contains(filter_lc.as_str())
        })
        .collect();

    let mut right_lines: Vec<Line<'static>> = Vec::new();

    right_lines.push(Line::from(Span::styled(
        " Slash Commands",
        Style::default()
            .fg(CLAURST_ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
    right_lines.push(Line::from(""));

    let mut current_cat = "";
    for entry in &filtered {
        if entry.category.as_str() != current_cat {
            current_cat = entry.category.as_str();
            if right_lines.len() > 2 {
                right_lines.push(Line::from(""));
            }
            right_lines.push(Line::from(Span::styled(
                format!(" {}", entry.category),
                Style::default()
                    .fg(CLAURST_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        let aliases_text = if entry.aliases.is_empty() {
            String::new()
        } else {
            format!(" ({})", entry.aliases)
        };
        right_lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("/{:<14}", entry.name),
                Style::default()
                    .fg(CLAURST_TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(aliases_text, Style::default().fg(CLAURST_MUTED)),
            Span::raw("  "),
            Span::styled(
                entry.description.clone(),
                Style::default().fg(CLAURST_MUTED),
            ),
        ]));
    }

    if filtered.is_empty() {
        right_lines.push(Line::from(Span::styled(
            " No matching commands",
            Style::default().fg(CLAURST_MUTED),
        )));
    }

    let right_total = right_lines.len() as u16;
    let right_visible = col_chunks[2].height;
    let max_scroll = right_total.saturating_sub(right_visible);
    let scroll = overlay.scroll_offset.min(max_scroll);

    frame.render_widget(
        Paragraph::new(right_lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .style(Style::default().bg(CLAURST_PANEL_BG)),
        col_chunks[2],
    );

    let version_line = Line::from(vec![Span::styled(
        format!(
            " v{}  ·  type to filter  ·  ↑↓ scroll commands  ·  esc close",
            APP_VERSION
        ),
        Style::default()
            .fg(CLAURST_MUTED)
            .add_modifier(Modifier::ITALIC),
    )]);
    frame.render_widget(Paragraph::new(version_line), layout.footer_area);
}

// ============================================================================
// HistorySearchOverlay
// ============================================================================

// ---------------------------------------------------------------------------
// HistoryEntry — wrapper with optional timestamp
// ---------------------------------------------------------------------------

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// A single history entry with an optional Unix timestamp and pinned state.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub text: String,
    /// Unix timestamp (seconds since epoch) when this entry was recorded.
    /// `None` for legacy entries without timestamps.
    pub timestamp: Option<u64>,
    /// Whether this entry has been pinned by the user.  Pinned entries always
    /// appear at the top of the history overlay list and are persisted to
    /// `~/.claurst/history_pins.json`.
    pub pinned: bool,
}

impl HistoryEntry {
    /// Create a new entry stamped with the current time.
    pub fn new(text: String) -> Self {
        Self {
            text,
            timestamp: Some(current_unix_secs()),
            pinned: false,
        }
    }

    /// Create a legacy entry without a timestamp.
    pub fn legacy(text: String) -> Self {
        Self {
            text,
            timestamp: None,
            pinned: false,
        }
    }

    /// Human-readable relative time: "just now", "2m ago", "3h ago", "2d ago", etc.
    pub fn relative_time(&self) -> String {
        let ts = match self.timestamp {
            None => return String::new(),
            Some(t) => t,
        };
        let now = current_unix_secs();
        let delta = now.saturating_sub(ts);
        if delta < 60 {
            "just now".to_string()
        } else if delta < 3600 {
            format!("{}m ago", delta / 60)
        } else if delta < 86400 {
            format!("{}h ago", delta / 3600)
        } else {
            format!("{}d ago", delta / 86400)
        }
    }
}

// ---------------------------------------------------------------------------
// Pinned-entry persistence  (~/.claurst/history_pins.json)
// ---------------------------------------------------------------------------

fn pins_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".claurst")
        .join("history_pins.json")
}

/// Load the set of pinned entry texts from `~/.claurst/history_pins.json`.
/// Returns an empty set if the file does not exist or cannot be parsed.
pub fn load_pinned_texts() -> std::collections::HashSet<String> {
    let path = pins_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return std::collections::HashSet::new(),
    };
    serde_json::from_str::<std::collections::HashSet<String>>(&content).unwrap_or_default()
}

/// Persist `pinned_texts` to `~/.claurst/history_pins.json`.
/// Failures are silently ignored (best-effort).
pub fn save_pinned_texts(pinned_texts: &std::collections::HashSet<String>) {
    let path = pins_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(pinned_texts) {
        let _ = std::fs::write(&path, json);
    }
}

// ---------------------------------------------------------------------------
// Fuzzy / subsequence matching
// ---------------------------------------------------------------------------

/// Compute a match score for `query` against `target`.
///
/// Fast path: if `target` contains `query` as a substring the score is
/// `1.0 + position_bonus` so it always beats a pure subsequence match.
///
/// Subsequence path: each character of `query` must appear in `target` in
/// order. The score is `consecutive_run_bonus + position_bonus` where
///   - `consecutive_run_bonus = longest_consecutive_run as f32 / query.len() as f32`
///   - `position_bonus       = 1.0 / (1.0 + first_match_position as f32)`
///
/// Returns `None` when `query` is neither a substring nor a subsequence of
/// `target`.
///
/// The returned `Vec<usize>` contains the byte indices in `target` that were
/// matched (useful for highlight rendering).
pub fn subsequence_score(query: &str, target: &str) -> Option<(f32, Vec<usize>)> {
    if query.is_empty() {
        return Some((0.0, Vec::new()));
    }

    let q_lc = query.to_lowercase();
    let t_lc = target.to_lowercase();

    // --- Fast path: substring match (always wins over subsequence) ----------
    if let Some(pos) = t_lc.find(q_lc.as_str()) {
        let position_bonus = 1.0 / (1.0 + pos as f32);
        let score = 1.0 + position_bonus;
        // Matched positions are the contiguous byte range [pos, pos+q_lc.len())
        let positions: Vec<usize> = (pos..pos + q_lc.len()).collect();
        return Some((score, positions));
    }

    // --- Subsequence path ---------------------------------------------------
    let q_chars: Vec<char> = q_lc.chars().collect();
    let t_chars: Vec<char> = t_lc.chars().collect();

    let mut q_pos = 0usize;
    // Map: char index in t_chars -> byte offset in original target
    let t_byte_offsets: Vec<usize> = {
        let mut off = 0usize;
        t_chars
            .iter()
            .map(|c| {
                let o = off;
                off += c.len_utf8();
                o
            })
            .collect()
    };

    let mut matched_char_indices: Vec<usize> = Vec::with_capacity(q_chars.len());

    for (t_i, &tc) in t_chars.iter().enumerate() {
        if q_pos < q_chars.len() && tc == q_chars[q_pos] {
            matched_char_indices.push(t_i);
            q_pos += 1;
        }
    }

    if q_pos < q_chars.len() {
        // Not all query chars found in order
        return None;
    }

    // Compute longest consecutive run among matched char indices
    let mut max_run = 1usize;
    let mut cur_run = 1usize;
    for w in matched_char_indices.windows(2) {
        if w[1] == w[0] + 1 {
            cur_run += 1;
            if cur_run > max_run {
                max_run = cur_run;
            }
        } else {
            cur_run = 1;
        }
    }

    let q_len = q_chars.len() as f32;
    let consecutive_run_bonus = max_run as f32 / q_len;
    let first_match_pos = matched_char_indices[0];
    let position_bonus = 1.0 / (1.0 + first_match_pos as f32);
    let score = consecutive_run_bonus + position_bonus;

    let byte_positions: Vec<usize> = matched_char_indices
        .iter()
        .map(|&ci| t_byte_offsets[ci])
        .collect();

    Some((score, byte_positions))
}

// ---------------------------------------------------------------------------
// MatchEntry — scored match with highlight positions
// ---------------------------------------------------------------------------

/// One scored match result produced by `update_matches`.
#[derive(Debug, Clone)]
pub struct MatchEntry {
    /// Index of this entry in the `snapshot` held by `HistorySearchOverlay`.
    pub snapshot_idx: usize,
    pub score: f32,
    /// Byte positions in `entry.text` that were matched (for highlighting).
    pub highlight_positions: Vec<usize>,
}

// ---------------------------------------------------------------------------
// HistorySearchOverlay
// ---------------------------------------------------------------------------

/// State for the Ctrl+R history search floating panel.
#[derive(Debug, Default)]
pub struct HistorySearchOverlay {
    pub visible: bool,
    pub query: String,
    /// Scored, sorted matches.  `matches[i].snapshot_idx` is the index into
    /// `snapshot`.  `matches` is sorted best-score-first.
    pub matches: Vec<MatchEntry>,
    pub selected_idx: usize,
    /// Snapshot of the history taken at `open()` time, stored as
    /// `HistoryEntry` so timestamps are available.
    pub snapshot: Vec<HistoryEntry>,
}

/// Convenience accessor: the plain list of `snapshot_idx` values from
/// `matches`, in order.  Kept for callers that only need indices.
impl HistorySearchOverlay {
    pub fn match_indices(&self) -> Vec<usize> {
        self.matches.iter().map(|m| m.snapshot_idx).collect()
    }
}

impl HistorySearchOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open with a `&[String]` slice (legacy callers).  All entries are
    /// treated as legacy (no timestamp).
    pub fn open(history: &[String]) -> Self {
        let entries: Vec<HistoryEntry> = history
            .iter()
            .map(|s| HistoryEntry::legacy(s.clone()))
            .collect();
        Self::open_with_entries(entries)
    }

    /// Open with a pre-built `Vec<HistoryEntry>` (timestamp-aware callers).
    ///
    /// Pinned state is loaded from `~/.claurst/history_pins.json` and applied
    /// to any matching entries.
    pub fn open_with_entries(entries: Vec<HistoryEntry>) -> Self {
        let pinned_texts = load_pinned_texts();
        let entries = entries
            .into_iter()
            .map(|mut e| {
                if pinned_texts.contains(&e.text) {
                    e.pinned = true;
                }
                e
            })
            .collect();
        let mut s = Self {
            visible: true,
            query: String::new(),
            matches: Vec::new(),
            selected_idx: 0,
            snapshot: entries,
        };
        s.recompute_matches();
        s
    }

    /// Toggle the pinned state of the currently selected entry.
    ///
    /// Persists the updated pin set to `~/.claurst/history_pins.json` and
    /// recomputes the match list so the entry moves to/from the pinned section.
    pub fn toggle_pin(&mut self) {
        let Some(m) = self.matches.get(self.selected_idx) else {
            return;
        };
        let snap_idx = m.snapshot_idx;
        let Some(entry) = self.snapshot.get_mut(snap_idx) else {
            return;
        };
        entry.pinned = !entry.pinned;

        // Rebuild the persisted pin set from the full snapshot.
        let pinned_texts: std::collections::HashSet<String> = self
            .snapshot
            .iter()
            .filter(|e| e.pinned)
            .map(|e| e.text.clone())
            .collect();
        save_pinned_texts(&pinned_texts);

        // Recompute without moving selected_idx so the cursor stays stable.
        self.recompute_matches();
    }

    // ------------------------------------------------------------------
    // Internal scoring
    // ------------------------------------------------------------------

    fn recompute_matches(&mut self) {
        let q = self.query.to_lowercase();
        let mut scored: Vec<MatchEntry> = self
            .snapshot
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| {
                if q.is_empty() {
                    Some(MatchEntry {
                        snapshot_idx: i,
                        score: 0.0,
                        highlight_positions: Vec::new(),
                    })
                } else {
                    subsequence_score(&q, &entry.text).map(|(score, positions)| MatchEntry {
                        snapshot_idx: i,
                        score,
                        highlight_positions: positions,
                    })
                }
            })
            .collect();

        // Sort: pinned entries always first, then by score descending.
        // Stable sort preserves insertion order for ties within each group.
        scored.sort_by(|a, b| {
            let a_pinned = self
                .snapshot
                .get(a.snapshot_idx)
                .map_or(false, |e| e.pinned);
            let b_pinned = self
                .snapshot
                .get(b.snapshot_idx)
                .map_or(false, |e| e.pinned);
            match (b_pinned, a_pinned) {
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => b
                    .score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal),
            }
        });

        self.matches = scored;
        // Clamp selection
        if !self.matches.is_empty() && self.selected_idx >= self.matches.len() {
            self.selected_idx = self.matches.len() - 1;
        }
    }

    // ------------------------------------------------------------------
    // Public API — backward-compatible with &[String] callers
    // ------------------------------------------------------------------

    /// Recompute matches from the given `history` slice.
    ///
    /// This updates the internal snapshot and recomputes.  Callers that pass
    /// `&app.prompt_input.history` every time will continue to work unchanged.
    pub fn update_matches(&mut self, history: &[String]) {
        // Rebuild snapshot preserving existing timestamps where possible.
        // Simple strategy: replace snapshot with legacy entries from `history`.
        // (A more sophisticated approach would merge by text, but keeping it
        // simple avoids complexity and matches the current call-site pattern.)
        self.snapshot = history
            .iter()
            .map(|s| HistoryEntry::legacy(s.clone()))
            .collect();
        self.recompute_matches();
    }

    pub fn push_char(&mut self, c: char, history: &[String]) {
        self.query.push(c);
        self.selected_idx = 0;
        self.update_matches(history);
    }

    pub fn pop_char(&mut self, history: &[String]) {
        self.query.pop();
        self.selected_idx = 0;
        self.update_matches(history);
    }

    pub fn select_prev(&mut self) {
        let count = self.matches.len();
        if count == 0 {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = count - 1;
        } else {
            self.selected_idx -= 1;
        }
    }

    pub fn select_next(&mut self) {
        let count = self.matches.len();
        if count == 0 {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % count;
    }

    /// Return the currently selected history entry text, if any.
    ///
    /// The `history` parameter is accepted for backward compatibility but the
    /// overlay uses its internal snapshot.  If `history` is non-empty it is
    /// used as a fallback when the snapshot is empty.
    pub fn current_entry<'a>(&self, history: &'a [String]) -> Option<&'a str> {
        let snap_idx = self.matches.get(self.selected_idx)?.snapshot_idx;
        // Try the history slice first (keeps existing call-sites working).
        history.get(snap_idx).map(String::as_str)
    }

    /// Like `current_entry` but returns from the internal snapshot.
    pub fn current_entry_owned(&self) -> Option<&str> {
        let snap_idx = self.matches.get(self.selected_idx)?.snapshot_idx;
        self.snapshot.get(snap_idx).map(|e| e.text.as_str())
    }

    pub fn close(&mut self) {
        self.visible = false;
    }
}

/// Render the history search floating panel.
pub fn render_history_search_overlay(
    frame: &mut Frame,
    overlay: &HistorySearchOverlay,
    history: &[String],
    area: Rect,
) {
    if !overlay.visible {
        return;
    }

    const VISIBLE_MATCHES: usize = 8;
    let dialog_width = 72u16.min(area.width.saturating_sub(4));
    let match_count = overlay.matches.len().max(1);
    let rows = VISIBLE_MATCHES.min(match_count) as u16;
    // +2 for blank separator + hint footer line, +2 for block borders
    let dialog_height = (6 + rows).min(area.height.saturating_sub(4));
    let dialog_area = centered_rect(dialog_width, dialog_height, area);

    frame.render_widget(Clear, dialog_area);

    let mut lines: Vec<Line> = Vec::new();

    // --- Search query line ---------------------------------------------------
    let result_count_str = format!("{} results", overlay.matches.len());
    lines.push(Line::from(vec![
        Span::raw("  Search: "),
        Span::styled(
            overlay.query.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{2588}", Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(
            result_count_str,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
    ]));
    lines.push(Line::from(""));

    if overlay.matches.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  (no matches)",
            Style::default().fg(Color::DarkGray),
        )]));
    } else {
        let start = overlay
            .selected_idx
            .saturating_sub(VISIBLE_MATCHES / 2)
            .min(overlay.matches.len().saturating_sub(VISIBLE_MATCHES));
        let end = (start + VISIBLE_MATCHES).min(overlay.matches.len());

        for (display_i, match_entry) in overlay.matches[start..end].iter().enumerate() {
            let real_i = start + display_i;
            let is_selected = real_i == overlay.selected_idx;

            // Resolve snapshot entry (for text, timestamp, pinned state).
            let snap_entry: Option<&HistoryEntry> = overlay.snapshot.get(match_entry.snapshot_idx);

            // Resolve entry text: prefer snapshot, fall back to passed-in history.
            let entry_text: &str = snap_entry
                .map(|e| e.text.as_str())
                .or_else(|| history.get(match_entry.snapshot_idx).map(String::as_str))
                .unwrap_or("");

            let is_pinned = snap_entry.map_or(false, |e| e.pinned);

            // Relative timestamp (right-aligned suffix)
            let time_suffix: String = snap_entry
                .map(|e| {
                    let t = e.relative_time();
                    if t.is_empty() {
                        t
                    } else {
                        format!(" · {}", t)
                    }
                })
                .unwrap_or_default();

            // Pin star shown to the left of pinned entries: "★ " (2 chars wide)
            // Available width for the entry text
            let pin_prefix_width: usize = if is_pinned { 2 } else { 0 };
            let prefix_width: usize = 4 + pin_prefix_width; // "    " or "  ► " + optional "★ "
            let time_width = UnicodeWidthStr::width(time_suffix.as_str());
            let max_text_chars =
                (dialog_width as usize).saturating_sub(prefix_width + time_width + 2);

            let (prefix, base_style) = if is_selected {
                (
                    "  \u{25BA} ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("    ", Style::default().fg(Color::White))
            };

            // Build highlighted spans for the entry text
            let text_spans = build_highlighted_spans(
                entry_text,
                &match_entry.highlight_positions,
                max_text_chars,
                base_style,
                is_selected,
            );

            let mut row_spans: Vec<Span> = vec![Span::raw(prefix)];

            // Pin star badge (shown for all pinned entries)
            if is_pinned {
                row_spans.push(Span::styled(
                    "\u{2605} ", // ★
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }

            row_spans.extend(text_spans);
            if !time_suffix.is_empty() {
                row_spans.push(Span::styled(
                    time_suffix,
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ));
            }

            lines.push(Line::from(row_spans));
        }
    }

    // Footer hint bar (below the match list)
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "  \u{2191}\u{2193} navigate  \u{00b7}  Enter select  \u{00b7}  p pin/unpin  \u{00b7}  Esc cancel",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        ),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" History Search ")
        .border_style(Style::default().fg(Color::Cyan));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, dialog_area);
}

/// Build a list of `Span`s for `text`, highlighting the bytes at
/// `highlight_positions` in yellow. Text is truncated to `max_chars`.
fn build_highlighted_spans<'a>(
    text: &str,
    highlight_positions: &[usize],
    max_chars: usize,
    base_style: Style,
    _is_selected: bool,
) -> Vec<Span<'a>> {
    // Collect char-level info (byte offset, char)
    let chars: Vec<(usize, char)> = text.char_indices().collect();

    // Convert highlight byte-positions to a set of byte offsets for O(1) lookup
    let hl_set: std::collections::HashSet<usize> = highlight_positions.iter().copied().collect();

    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut current_text = String::new();
    let mut current_highlighted = false;
    let mut char_count = 0usize;
    let mut truncated = false;

    for (byte_off, ch) in &chars {
        if char_count >= max_chars {
            truncated = true;
            break;
        }
        let is_hl = hl_set.contains(byte_off);
        if is_hl != current_highlighted && !current_text.is_empty() {
            let style = if current_highlighted {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                base_style
            };
            spans.push(Span::styled(current_text.clone(), style));
            current_text.clear();
        }
        current_highlighted = is_hl;
        current_text.push(*ch);
        char_count += 1;
    }
    if !current_text.is_empty() {
        let style = if current_highlighted {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            base_style
        };
        spans.push(Span::styled(current_text, style));
    }
    if truncated {
        spans.push(Span::styled(
            "…".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans
}

// ============================================================================
// MessageSelectorOverlay
// ============================================================================

/// A single entry shown in the message selector list.
#[derive(Debug, Clone)]
pub struct SelectorMessage {
    /// Original index in the conversation.
    pub idx: usize,
    pub role: String,
    /// First ~80 chars of content.
    pub preview: String,
    pub has_tool_use: bool,
}

/// State for the message selector overlay used by /rewind step 1.
#[derive(Debug, Default)]
pub struct MessageSelectorOverlay {
    pub visible: bool,
    pub messages: Vec<SelectorMessage>,
    pub selected_idx: usize,
    pub scroll_offset: usize,
}

impl MessageSelectorOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(messages: Vec<SelectorMessage>) -> Self {
        // Start with selection at the end (most recent)
        let selected = messages.len().saturating_sub(1);
        Self {
            visible: true,
            messages,
            selected_idx: selected,
            scroll_offset: selected.saturating_sub(5),
        }
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn select_prev(&mut self) {
        const VISIBLE_ROWS: usize = 12;
        let count = self.messages.len();
        if count == 0 {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = count - 1;
            self.scroll_offset = count.saturating_sub(VISIBLE_ROWS);
        } else {
            self.selected_idx -= 1;
            if self.selected_idx < self.scroll_offset {
                self.scroll_offset = self.selected_idx;
            }
        }
    }

    pub fn select_next(&mut self) {
        const VISIBLE_ROWS: usize = 12;
        let count = self.messages.len();
        if count == 0 {
            return;
        }
        if self.selected_idx + 1 >= count {
            self.selected_idx = 0;
            self.scroll_offset = 0;
        } else {
            self.selected_idx += 1;
            if self.selected_idx >= self.scroll_offset + VISIBLE_ROWS {
                self.scroll_offset = self.selected_idx - VISIBLE_ROWS + 1;
            }
        }
    }

    pub fn current_message(&self) -> Option<&SelectorMessage> {
        self.messages.get(self.selected_idx)
    }
}

/// Render the message selector overlay.
pub fn render_message_selector(frame: &mut Frame, overlay: &MessageSelectorOverlay, area: Rect) {
    if !overlay.visible {
        return;
    }

    const VISIBLE_ROWS: usize = 12;
    let dialog_width = 70u16.min(area.width.saturating_sub(4));
    let rows = VISIBLE_ROWS.min(overlay.messages.len().max(1)) as u16;
    let dialog_height = (rows + 4).min(area.height.saturating_sub(4));
    let dialog_area = centered_rect(dialog_width, dialog_height, area);

    frame.render_widget(Clear, dialog_area);

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![Span::styled(
        "  Select a message to rewind to:",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(""));

    if overlay.messages.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  (no messages)",
            Style::default().fg(Color::DarkGray),
        )]));
    } else {
        let start = overlay.scroll_offset;
        let end = (start + VISIBLE_ROWS).min(overlay.messages.len());

        for (display_i, msg) in overlay.messages[start..end].iter().enumerate() {
            let real_i = start + display_i;
            let is_selected = real_i == overlay.selected_idx;

            let role_color = if msg.role == "user" {
                Color::Cyan
            } else {
                Color::Green
            };

            let tool_tag = if msg.has_tool_use { " [tool]" } else { "" };

            let preview_max = dialog_width as usize - 20;
            let preview = if UnicodeWidthStr::width(msg.preview.as_str()) > preview_max {
                format!("{}…", &msg.preview[..preview_max.saturating_sub(1)])
            } else {
                msg.preview.clone()
            };

            let prefix = if is_selected { "  \u{25BA} " } else { "    " };
            let idx_style = if is_selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            lines.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled(format!("{:>3}. ", msg.idx), idx_style),
                Span::styled(
                    format!("{:<10}", msg.role),
                    Style::default()
                        .fg(role_color)
                        .add_modifier(if is_selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled(
                    preview,
                    if is_selected {
                        Style::default().fg(Color::White)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::styled(tool_tag.to_string(), Style::default().fg(Color::Yellow)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        "  ↑↓ navigate  ·  Enter to select  ·  Esc to cancel",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )]));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Rewind — Select Message ")
        .border_style(Style::default().fg(Color::Yellow));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, dialog_area);
}

// ============================================================================
// RewindFlowOverlay  (multi-step: select → confirm → done)
// ============================================================================

/// The current step in the rewind flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewindStep {
    /// Step 1: user is browsing the message list.
    Selecting,
    /// Step 2: user has chosen a message and must confirm.
    Confirming { message_idx: usize },
}

/// Full multi-step overlay for the /rewind command.
#[derive(Debug)]
pub struct RewindFlowOverlay {
    pub visible: bool,
    pub step: RewindStep,
    pub selector: MessageSelectorOverlay,
}

impl Default for RewindFlowOverlay {
    fn default() -> Self {
        Self {
            visible: false,
            step: RewindStep::Selecting,
            selector: MessageSelectorOverlay::new(),
        }
    }
}

impl RewindFlowOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the overlay with the given conversation messages.
    pub fn open(&mut self, messages: Vec<SelectorMessage>) {
        self.selector = MessageSelectorOverlay::open(messages);
        self.step = RewindStep::Selecting;
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.selector.close();
        self.step = RewindStep::Selecting;
    }

    /// Confirm the current selection; advances to the `Confirming` step.
    /// Returns the selected message index if in the Selecting step.
    pub fn confirm_selection(&mut self) -> Option<usize> {
        if self.step == RewindStep::Selecting {
            if let Some(msg) = self.selector.current_message() {
                let idx = msg.idx;
                self.step = RewindStep::Confirming { message_idx: idx };
                return Some(idx);
            }
        }
        None
    }

    /// The user pressed 'y' in the Confirming step.
    /// Returns the final message index to rewind to.
    pub fn accept_confirm(&mut self) -> Option<usize> {
        if let RewindStep::Confirming { message_idx } = self.step {
            self.close();
            return Some(message_idx);
        }
        None
    }

    /// The user pressed 'n' or Esc in the Confirming step — go back to selector.
    pub fn reject_confirm(&mut self) {
        if matches!(self.step, RewindStep::Confirming { .. }) {
            self.step = RewindStep::Selecting;
        }
    }
}

/// Render the full rewind flow overlay.
pub fn render_rewind_flow(frame: &mut Frame, overlay: &RewindFlowOverlay, area: Rect) {
    if !overlay.visible {
        return;
    }

    match &overlay.step {
        RewindStep::Selecting => {
            render_message_selector(frame, &overlay.selector, area);
        }
        RewindStep::Confirming { message_idx } => {
            render_rewind_confirm(frame, *message_idx, area);
        }
    }
}

fn render_rewind_confirm(frame: &mut Frame, message_idx: usize, area: Rect) {
    let dialog_width = 50u16.min(area.width.saturating_sub(4));
    let dialog_height = 7u16.min(area.height.saturating_sub(4));
    let dialog_area = centered_rect(dialog_width, dialog_height, area);

    frame.render_widget(Clear, dialog_area);

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("  Rewind to message "),
            Span::styled(
                format!("#{}", message_idx),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  [y] ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("Yes, rewind"),
            Span::raw("    "),
            Span::styled(
                "[n] ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw("Cancel"),
        ]),
        Line::from(""),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Confirm Rewind ")
        .border_style(Style::default().fg(Color::Yellow));

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, dialog_area);
}

// ---------------------------------------------------------------------------
// Shared helper
// ---------------------------------------------------------------------------

fn kb_line<'a>(key: &str, desc: &str) -> Line<'a> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<20}", key),
            Style::default()
                .fg(CLAURST_TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(desc.to_string(), Style::default().fg(CLAURST_MUTED)),
    ])
}

// ---------------------------------------------------------------------------
// Global Search Dialog (T2-7)
// ---------------------------------------------------------------------------

/// State for the global ripgrep search dialog.
#[derive(Debug, Clone, Default)]
pub struct GlobalSearchState {
    pub open: bool,
    pub query: String,
    pub results: Vec<SearchResult>,
    pub selected: usize,
    pub total_matches: usize,
    pub searching: bool,
}

/// A single search result from ripgrep.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub text: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

impl GlobalSearchState {
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.results.clear();
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn select_prev(&mut self) {
        let count = self.results.len();
        if count == 0 {
            return;
        }
        if self.selected == 0 {
            self.selected = count - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn select_next(&mut self) {
        let count = self.results.len();
        if count == 0 {
            return;
        }
        self.selected = (self.selected + 1) % count;
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.selected = 0;
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    /// Run ripgrep synchronously (should be called from tokio::task::spawn_blocking).
    pub fn run_search(&mut self, project_root: &std::path::Path) {
        if self.query.is_empty() {
            self.results.clear();
            return;
        }
        self.searching = true;
        let output = std::process::Command::new("rg")
            .args([
                "--json",
                "--max-count",
                "10",
                "--max-filesize",
                "1M",
                &self.query,
                ".",
            ])
            .current_dir(project_root)
            .output();

        self.searching = false;
        self.results.clear();
        self.total_matches = 0;

        if let Ok(out) = output {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                    match val["type"].as_str() {
                        Some("match") => {
                            let data = &val["data"];
                            let file = data["path"]["text"].as_str().unwrap_or("").to_string();
                            let line_no = data["line_number"].as_u64().unwrap_or(0) as u32;
                            let text = data["lines"]["text"]
                                .as_str()
                                .unwrap_or("")
                                .trim_end_matches('\n')
                                .to_string();
                            let col = data["submatches"][0]["start"].as_u64().unwrap_or(0) as u32;
                            self.results.push(SearchResult {
                                file,
                                line: line_no,
                                col,
                                text,
                                context_before: Vec::new(),
                                context_after: Vec::new(),
                            });
                            self.total_matches += 1;
                            if self.results.len() >= 500 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Return the selected result as a `file:line` string for prompt injection.
    pub fn selected_ref(&self) -> Option<String> {
        self.results
            .get(self.selected)
            .map(|r| format!("{}:{}", r.file, r.line))
    }
}

/// Render the global search dialog overlay.
pub fn render_global_search(
    state: &GlobalSearchState,
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
) {
    use ratatui::{
        layout::Rect,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph, Widget},
    };
    use std::path::Path;

    if !state.open {
        return;
    }

    let w = (area.width * 4 / 5).max(40).min(area.width);
    let h = (area.height * 3 / 4).max(10).min(area.height);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 4;
    let dialog = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    Clear.render(dialog, buf);
    Block::default()
        .title(" Search [Esc: close, Enter: insert, \u{2191}\u{2193}: navigate] ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Cyan))
        .render(dialog, buf);

    let inner = Rect {
        x: dialog.x + 1,
        y: dialog.y + 1,
        width: dialog.width.saturating_sub(2),
        height: dialog.height.saturating_sub(2),
    };

    // Query input bar (first row)
    let query_line = Line::from(vec![
        Span::styled("/ ", Style::default().fg(Color::Cyan)),
        Span::styled(state.query.clone(), Style::default().fg(Color::White)),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]);
    Paragraph::new(query_line).render(
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
        buf,
    );

    // Separator
    let sep = Line::from(Span::styled(
        "\u{2500}".repeat(inner.width as usize),
        Style::default().fg(Color::DarkGray),
    ));
    Paragraph::new(sep).render(
        Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: 1,
        },
        buf,
    );

    let results_area = Rect {
        x: inner.x,
        y: inner.y + 2,
        width: inner.width,
        height: inner.height.saturating_sub(3),
    };

    // Build grouped display rows: (is_header, result_idx_or_none, file_label, match_count, result_ref)
    // Group results by file
    #[derive(Clone)]
    enum DisplayRow {
        Header { label: String, count: usize },
        Result { result_idx: usize },
    }

    let mut rows: Vec<DisplayRow> = Vec::new();
    if !state.results.is_empty() {
        let mut current_file = "";
        let mut group_count = 0usize;
        let mut group_start = 0usize;

        for (idx, result) in state.results.iter().enumerate() {
            if result.file.as_str() != current_file {
                if !current_file.is_empty() {
                    // Patch the header we already pushed with the real count
                    if let Some(DisplayRow::Header { count, .. }) = rows.get_mut(group_start) {
                        *count = group_count;
                    }
                }
                current_file = result.file.as_str();
                group_count = 0;
                group_start = rows.len();
                let label = Path::new(&result.file)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&result.file)
                    .to_string();
                rows.push(DisplayRow::Header { label, count: 0 });
            }
            group_count += 1;
            rows.push(DisplayRow::Result { result_idx: idx });
        }
        // Patch last group
        if let Some(DisplayRow::Header { count, .. }) = rows.get_mut(group_start) {
            *count = group_count;
        }
    }

    let max_visible = results_area.height as usize;
    // Scroll so the selected result is visible — find which display row it's in
    let selected_display_row = rows
        .iter()
        .position(|r| {
            if let DisplayRow::Result { result_idx } = r {
                *result_idx == state.selected
            } else {
                false
            }
        })
        .unwrap_or(0);
    let start = selected_display_row.saturating_sub(max_visible / 2);

    for (i, row) in rows[start..].iter().enumerate() {
        if i >= max_visible {
            break;
        }
        let row_y = results_area.y + i as u16;

        match row {
            DisplayRow::Header { label, count } => {
                // File group header: ─── filename (N) ──────────
                let count_str = format!(" ({}) ", count);
                let label_part = format!(" {} ", label);
                let dashes_right = (results_area.width as usize)
                    .saturating_sub(4 + label_part.len() + count_str.len());
                let header_line = Line::from(vec![Span::styled(
                    format!(
                        "\u{2500}\u{2500}\u{2500}{}{}{}",
                        label_part,
                        count_str,
                        "\u{2500}".repeat(dashes_right)
                    ),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )]);
                Paragraph::new(header_line).render(
                    Rect {
                        x: results_area.x,
                        y: row_y,
                        width: results_area.width,
                        height: 1,
                    },
                    buf,
                );
            }
            DisplayRow::Result { result_idx } => {
                let result = &state.results[*result_idx];
                let selected = *result_idx == state.selected;
                let prefix = if selected { "> " } else { "  " };
                let style = if selected {
                    Style::default()
                        .add_modifier(Modifier::BOLD)
                        .fg(Color::White)
                } else {
                    Style::default().fg(Color::Gray)
                };

                // Highlight query match in text
                let text_trimmed = result.text.trim();
                let query_lc = state.query.to_lowercase();
                let text_spans: Vec<Span<'static>> = if !query_lc.is_empty() {
                    let text_lc = text_trimmed.to_lowercase();
                    if let Some(pos) = text_lc.find(query_lc.as_str()) {
                        let before: String = text_trimmed
                            .chars()
                            .take(text_trimmed[..pos].chars().count())
                            .collect();
                        let matched: String = text_trimmed[pos..pos + query_lc.len()].to_string();
                        let after: String = text_trimmed[pos + query_lc.len()..]
                            .chars()
                            .take(30)
                            .collect();
                        vec![
                            Span::styled(before, style),
                            Span::styled(
                                matched,
                                style.bg(Color::Rgb(60, 50, 0)).fg(Color::Yellow),
                            ),
                            Span::styled(after, style),
                        ]
                    } else {
                        let t: String = text_trimmed.chars().take(50).collect();
                        vec![Span::styled(t, style)]
                    }
                } else {
                    let t: String = text_trimmed.chars().take(50).collect();
                    vec![Span::styled(t, style)]
                };

                let mut spans = vec![
                    Span::styled(prefix.to_string(), style),
                    Span::styled(format!("{:>4}  ", result.line), style.fg(Color::DarkGray)),
                ];
                spans.extend(text_spans);

                Paragraph::new(Line::from(spans)).render(
                    Rect {
                        x: results_area.x,
                        y: row_y,
                        width: results_area.width,
                        height: 1,
                    },
                    buf,
                );
            }
        }
    }

    // Status bar
    let status = if state.searching {
        "Searching\u{2026}".to_string()
    } else if state.results.is_empty() && !state.query.is_empty() {
        "No matches".to_string()
    } else if state.total_matches > 0 {
        format!(
            "{} matches in {} files",
            state.total_matches,
            state
                .results
                .iter()
                .map(|r| &r.file)
                .collect::<std::collections::HashSet<_>>()
                .len()
        )
    } else {
        "Type to search".to_string()
    };
    let status_y = inner.y + inner.height.saturating_sub(1);
    Paragraph::new(Line::from(vec![Span::styled(
        status,
        Style::default().fg(Color::DarkGray),
    )]))
    .render(
        Rect {
            x: inner.x,
            y: status_y,
            width: inner.width,
            height: 1,
        },
        buf,
    );
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- HelpOverlay ---------------------------------------------------

    #[test]
    fn help_overlay_toggle() {
        let mut h = HelpOverlay::new();
        assert!(!h.visible);
        h.toggle();
        assert!(h.visible);
        h.toggle();
        assert!(!h.visible);
    }

    #[test]
    fn help_overlay_close_resets_state() {
        let mut h = HelpOverlay::new();
        h.visible = true;
        h.scroll_offset = 5;
        h.filter = "foo".to_string();
        h.close();
        assert!(!h.visible);
        assert_eq!(h.scroll_offset, 0);
        assert!(h.filter.is_empty());
    }

    #[test]
    fn help_overlay_filter() {
        let mut h = HelpOverlay::new();
        h.push_filter_char('h');
        h.push_filter_char('e');
        assert_eq!(h.filter, "he");
        h.pop_filter_char();
        assert_eq!(h.filter, "h");
    }

    #[test]
    fn modal_search_line_separates_leading_space_from_cursor() {
        let line = modal_search_line("", "Search", CLAURST_MUTED, CLAURST_TEXT);
        assert_eq!(line.spans.len(), 3);
        assert_eq!(line.spans[0].content.as_ref(), " ");
        assert_eq!(line.spans[1].content.as_ref(), "S");
        assert_eq!(line.spans[2].content.as_ref(), "earch");
    }

    // --- HistorySearchOverlay -----------------------------------------

    #[test]
    fn history_search_update_matches() {
        // All three entries contain 'g', so all three match.
        let history = vec![
            "git commit".to_string(),
            "cargo build".to_string(),
            "git push".to_string(),
        ];
        let mut hs = HistorySearchOverlay::open(&history);
        hs.push_char('g', &history);
        assert_eq!(hs.matches.len(), 3);

        // "gi": "cargo build" has 'g' at index 3 and 'i' in "build",
        // so it IS a subsequence match -- all three still match.
        hs.push_char('i', &history);
        assert_eq!(hs.matches.len(), 3);

        // Narrowing further to "git": "cargo build" has no 't' after g+i, so
        // only the two git entries match.
        hs.push_char('t', &history);
        assert_eq!(hs.matches.len(), 2);
        let idxs: Vec<usize> = hs.matches.iter().map(|m| m.snapshot_idx).collect();
        assert!(idxs.contains(&0));
        assert!(idxs.contains(&2));
    }

    #[test]
    fn history_search_navigation() {
        let history = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut hs = HistorySearchOverlay::open(&history);
        assert_eq!(hs.selected_idx, 0);
        hs.select_prev();
        assert_eq!(hs.selected_idx, 2);
        hs.select_next();
        assert_eq!(hs.selected_idx, 0);
        hs.select_prev();
        assert_eq!(hs.selected_idx, 2);
    }

    #[test]
    fn history_search_current_entry() {
        let history = vec!["first".to_string(), "second".to_string()];
        let hs = HistorySearchOverlay::open(&history);
        // With no query all entries match; index 0 is first.
        assert_eq!(hs.current_entry(&history), Some("first"));
    }

    // --- subsequence_score tests --------------------------------------

    #[test]
    fn subseq_score_none_for_non_subsequence() {
        // "xyz" cannot be a subsequence of "abcde"
        assert!(subsequence_score("xyz", "abcde").is_none());
        // letters out of order
        assert!(subsequence_score("ba", "abc").is_none());
    }

    #[test]
    fn subseq_score_some_for_exact_subsequence() {
        // 'g','i','t' in order inside "git push"
        assert!(subsequence_score("git", "git push").is_some());
        // non-consecutive subsequence: 'g','t' in "get it together"
        assert!(subsequence_score("gt", "get it together").is_some());
    }

    #[test]
    fn subseq_score_substring_beats_subsequence() {
        // "git" appears as a substring in "git push" and as a subsequence in
        // "go into town".  The substring match should score higher.
        let (score_sub, _) = subsequence_score("git", "git push").unwrap();
        let (score_seq, _) = subsequence_score("git", "go into town").unwrap();
        assert!(
            score_sub > score_seq,
            "substring score {score_sub} should beat subsequence score {score_seq}"
        );
    }

    #[test]
    fn subseq_score_returns_correct_positions_for_substring() {
        // "git" at position 0 in "git commit" → positions 0,1,2
        let (_, positions) = subsequence_score("git", "git commit").unwrap();
        assert_eq!(positions, vec![0, 1, 2]);
    }

    #[test]
    fn subseq_score_sorts_correctly_in_overlay() {
        // "git commit" and "get items together" both match query "git".
        // "git commit" is a substring match → higher score → appears first.
        let history = vec!["get items together".to_string(), "git commit".to_string()];
        let mut hs = HistorySearchOverlay::open(&history);
        hs.push_char('g', &history);
        hs.push_char('i', &history);
        hs.push_char('t', &history);
        // First match should be "git commit" (snapshot_idx 1, higher score)
        assert_eq!(hs.matches[0].snapshot_idx, 1);
    }

    // --- HistoryEntry timestamp tests ---------------------------------

    #[test]
    fn history_entry_relative_time_just_now() {
        let entry = HistoryEntry::new("hello".to_string());
        assert_eq!(entry.relative_time(), "just now");
    }

    #[test]
    fn history_entry_relative_time_minutes() {
        let five_mins_ago = current_unix_secs().saturating_sub(300);
        let entry = HistoryEntry {
            text: "cmd".to_string(),
            timestamp: Some(five_mins_ago),
            pinned: false,
        };
        assert_eq!(entry.relative_time(), "5m ago");
    }

    #[test]
    fn history_entry_relative_time_hours() {
        let two_hours_ago = current_unix_secs().saturating_sub(7200);
        let entry = HistoryEntry {
            text: "cmd".to_string(),
            timestamp: Some(two_hours_ago),
            pinned: false,
        };
        assert_eq!(entry.relative_time(), "2h ago");
    }

    #[test]
    fn history_entry_relative_time_days() {
        let three_days_ago = current_unix_secs().saturating_sub(3 * 86400);
        let entry = HistoryEntry {
            text: "cmd".to_string(),
            timestamp: Some(three_days_ago),
            pinned: false,
        };
        assert_eq!(entry.relative_time(), "3d ago");
    }

    #[test]
    fn history_entry_legacy_has_no_timestamp() {
        let entry = HistoryEntry::legacy("old command".to_string());
        assert!(entry.timestamp.is_none());
        assert_eq!(entry.relative_time(), "");
    }

    #[test]
    fn history_search_with_timestamps_stores_snapshot() {
        let entries = vec![
            HistoryEntry::new("cargo test".to_string()),
            HistoryEntry::legacy("old cmd".to_string()),
        ];
        let hs = HistorySearchOverlay::open_with_entries(entries);
        assert_eq!(hs.snapshot.len(), 2);
        assert!(hs.snapshot[0].timestamp.is_some());
        assert!(hs.snapshot[1].timestamp.is_none());
        // Relative time for legacy entry is empty
        assert_eq!(hs.snapshot[1].relative_time(), "");
        // Relative time for new entry is "just now"
        assert_eq!(hs.snapshot[0].relative_time(), "just now");
    }

    // --- MessageSelectorOverlay ---------------------------------------

    #[test]
    fn message_selector_open_selects_last() {
        let msgs = vec![
            SelectorMessage {
                idx: 0,
                role: "user".to_string(),
                preview: "hi".to_string(),
                has_tool_use: false,
            },
            SelectorMessage {
                idx: 1,
                role: "assistant".to_string(),
                preview: "hello".to_string(),
                has_tool_use: false,
            },
        ];
        let sel = MessageSelectorOverlay::open(msgs);
        assert_eq!(sel.selected_idx, 1);
    }

    #[test]
    fn message_selector_navigate() {
        let msgs = vec![
            SelectorMessage {
                idx: 0,
                role: "user".to_string(),
                preview: "a".to_string(),
                has_tool_use: false,
            },
            SelectorMessage {
                idx: 1,
                role: "assistant".to_string(),
                preview: "b".to_string(),
                has_tool_use: false,
            },
            SelectorMessage {
                idx: 2,
                role: "user".to_string(),
                preview: "c".to_string(),
                has_tool_use: false,
            },
        ];
        let mut sel = MessageSelectorOverlay::open(msgs);
        // starts at last
        assert_eq!(sel.selected_idx, 2);
        sel.select_prev();
        assert_eq!(sel.selected_idx, 1);
        sel.select_next();
        assert_eq!(sel.selected_idx, 2);
        sel.select_next();
        assert_eq!(sel.selected_idx, 0);
    }

    // --- RewindFlowOverlay -------------------------------------------

    #[test]
    fn rewind_flow_confirm_advances_step() {
        let msgs = vec![SelectorMessage {
            idx: 0,
            role: "user".to_string(),
            preview: "hi".to_string(),
            has_tool_use: false,
        }];
        let mut flow = RewindFlowOverlay::new();
        flow.open(msgs);
        let idx = flow.confirm_selection().unwrap();
        assert_eq!(idx, 0);
        assert!(matches!(
            flow.step,
            RewindStep::Confirming { message_idx: 0 }
        ));
    }

    #[test]
    fn rewind_flow_accept_closes() {
        let msgs = vec![SelectorMessage {
            idx: 3,
            role: "user".to_string(),
            preview: "test".to_string(),
            has_tool_use: false,
        }];
        let mut flow = RewindFlowOverlay::new();
        flow.open(msgs);
        flow.confirm_selection();
        let result = flow.accept_confirm().unwrap();
        assert_eq!(result, 3);
        assert!(!flow.visible);
    }

    #[test]
    fn rewind_flow_reject_returns_to_selector() {
        let msgs = vec![SelectorMessage {
            idx: 0,
            role: "user".to_string(),
            preview: "x".to_string(),
            has_tool_use: false,
        }];
        let mut flow = RewindFlowOverlay::new();
        flow.open(msgs);
        flow.confirm_selection();
        assert!(matches!(flow.step, RewindStep::Confirming { .. }));
        flow.reject_confirm();
        assert_eq!(flow.step, RewindStep::Selecting);
        assert!(flow.visible);
    }
}
