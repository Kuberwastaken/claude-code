//! Reusable live execution timeline model for the TUI.
//!
//! This module intentionally contains no rendering code. It provides a small,
//! deterministic state container that other TUI layers can feed from agent,
//! tool, or turn events.

/// Status of a timeline row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineStatus {
    Running,
    Done,
    Error,
    Cancelled,
}

/// Kinds of rows shown in the execution timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineKind {
    ToolCall,
    TurnSummary,
    Status,
}

/// One visible timeline row.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineRow {
    pub id: String,
    pub title: String,
    pub kind: TimelineKind,
    pub status: TimelineStatus,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub token_delta_input: Option<u64>,
    pub token_delta_output: Option<u64>,
    pub cost_delta_usd: Option<f64>,
    pub detail_preview: String,
    pub expandable_details: String,
}

impl TimelineRow {
    /// Duration in milliseconds when both timestamps are known.
    #[must_use]
    pub fn duration_ms(&self) -> Option<u64> {
        self.finished_at_ms
            .map(|finished_at_ms| finished_at_ms.saturating_sub(self.started_at_ms))
    }

    /// Returns true once the row has reached a terminal status.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            TimelineStatus::Done | TimelineStatus::Error | TimelineStatus::Cancelled
        )
    }

    /// Returns true when the row has additional details worth expanding.
    #[must_use]
    pub fn has_expandable_details(&self) -> bool {
        !self.expandable_details.is_empty()
    }
}

/// Default cap used for bounded retention.
pub const DEFAULT_MAX_ROWS: usize = 200;

/// Timeline state and helpers for live UI integration.
#[derive(Debug, Clone)]
pub struct Timeline {
    pub rows: Vec<TimelineRow>,
    pub selected_idx: usize,
    pub max_rows: usize,
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ROWS)
    }
}

impl Timeline {
    /// Create a new timeline with the provided retention cap.
    #[must_use]
    pub fn new(max_rows: usize) -> Self {
        Self {
            rows: Vec::new(),
            selected_idx: 0,
            max_rows: max_rows.max(1),
        }
    }

    /// Number of retained rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the timeline is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Returns the currently selected row, if any.
    #[must_use]
    pub fn selected_row(&self) -> Option<&TimelineRow> {
        self.rows.get(self.selected_idx)
    }

    /// Returns the currently selected row mutably, if any.
    #[must_use]
    pub fn selected_row_mut(&mut self) -> Option<&mut TimelineRow> {
        self.rows.get_mut(self.selected_idx)
    }

    /// Returns the last retained row, if any.
    #[must_use]
    pub fn last_row(&self) -> Option<&TimelineRow> {
        self.rows.last()
    }

    /// Clamp the selected index to a valid row, if any rows exist.
    pub fn clamp_selected_idx(&mut self) {
        self.selected_idx = Self::clamp_index(self.selected_idx, self.rows.len());
    }

    /// Set the selected row, clamping to the last row when needed.
    pub fn set_selected_idx(&mut self, idx: usize) {
        self.selected_idx = Self::clamp_index(idx, self.rows.len());
    }

    /// Update the retention cap and prune immediately if required.
    ///
    /// Returns the number of rows removed from the front.
    pub fn set_max_rows(&mut self, max_rows: usize) -> usize {
        self.max_rows = max_rows.max(1);
        self.prune_to_limit()
    }

    /// Selection-safe helper for front-pruning a bounded row list.
    ///
    /// `removed_from_front` is the number of oldest rows dropped from the front.
    /// `remaining_len` is the length after pruning.
    #[must_use]
    pub fn selected_index_after_prune(
        selected_idx: usize,
        removed_from_front: usize,
        remaining_len: usize,
    ) -> usize {
        if remaining_len == 0 {
            return 0;
        }

        if selected_idx < removed_from_front {
            0
        } else {
            (selected_idx - removed_from_front).min(remaining_len - 1)
        }
    }

    /// Add a new running tool row.
    pub fn add_running_tool(
        &mut self,
        id: impl Into<String>,
        title: impl Into<String>,
        started_at_ms: u64,
        detail_preview: impl Into<String>,
        expandable_details: impl Into<String>,
    ) -> usize {
        self.push_row(TimelineRow {
            id: id.into(),
            title: title.into(),
            kind: TimelineKind::ToolCall,
            status: TimelineStatus::Running,
            started_at_ms,
            finished_at_ms: None,
            token_delta_input: None,
            token_delta_output: None,
            cost_delta_usd: None,
            detail_preview: detail_preview.into(),
            expandable_details: expandable_details.into(),
        })
    }

    /// Mark an existing tool row as finished.
    ///
    /// Returns the updated row index when the id is found.
    pub fn finish_tool(
        &mut self,
        id: &str,
        finished_at_ms: u64,
        status: TimelineStatus,
        detail_preview: impl Into<String>,
        expandable_details: impl Into<String>,
        token_delta_input: Option<u64>,
        token_delta_output: Option<u64>,
        cost_delta_usd: Option<f64>,
    ) -> Option<usize> {
        let idx = self.rows.iter().rposition(|row| row.id == id)?;
        let row = self.rows.get_mut(idx)?;
        row.kind = TimelineKind::ToolCall;
        row.status = status;
        row.finished_at_ms = Some(finished_at_ms);
        row.token_delta_input = token_delta_input;
        row.token_delta_output = token_delta_output;
        row.cost_delta_usd = cost_delta_usd;
        row.detail_preview = detail_preview.into();
        row.expandable_details = expandable_details.into();
        self.clamp_selected_idx();
        Some(idx)
    }

    /// Add a completed turn summary row.
    pub fn add_turn_summary(
        &mut self,
        id: impl Into<String>,
        title: impl Into<String>,
        started_at_ms: u64,
        finished_at_ms: u64,
        detail_preview: impl Into<String>,
        expandable_details: impl Into<String>,
        token_delta_input: Option<u64>,
        token_delta_output: Option<u64>,
        cost_delta_usd: Option<f64>,
    ) -> usize {
        self.push_row(TimelineRow {
            id: id.into(),
            title: title.into(),
            kind: TimelineKind::TurnSummary,
            status: TimelineStatus::Done,
            started_at_ms,
            finished_at_ms: Some(finished_at_ms),
            token_delta_input,
            token_delta_output,
            cost_delta_usd,
            detail_preview: detail_preview.into(),
            expandable_details: expandable_details.into(),
        })
    }

    /// Add a status note row.
    pub fn add_status_note(
        &mut self,
        id: impl Into<String>,
        title: impl Into<String>,
        started_at_ms: u64,
        status: TimelineStatus,
        detail_preview: impl Into<String>,
        expandable_details: impl Into<String>,
    ) -> usize {
        self.push_row(TimelineRow {
            id: id.into(),
            title: title.into(),
            kind: TimelineKind::Status,
            status,
            started_at_ms,
            finished_at_ms: Some(started_at_ms),
            token_delta_input: None,
            token_delta_output: None,
            cost_delta_usd: None,
            detail_preview: detail_preview.into(),
            expandable_details: expandable_details.into(),
        })
    }

    /// Push a row, then enforce the retention cap.
    pub fn push_row(&mut self, row: TimelineRow) -> usize {
        self.rows.push(row);
        let idx = self.rows.len() - 1;
        self.prune_to_limit();
        idx.min(self.rows.len().saturating_sub(1))
    }

    /// Drop the oldest rows until the retention cap is satisfied.
    ///
    /// Returns the number of rows removed from the front.
    pub fn prune_to_limit(&mut self) -> usize {
        if self.rows.len() <= self.max_rows {
            self.clamp_selected_idx();
            return 0;
        }

        let old_len = self.rows.len();
        let removed = old_len.saturating_sub(self.max_rows);
        self.rows.drain(..removed);
        self.selected_idx =
            Self::selected_index_after_prune(self.selected_idx, removed, self.rows.len());
        removed
    }

    fn clamp_index(idx: usize, len: usize) -> usize {
        if len == 0 {
            0
        } else {
            idx.min(len - 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_finish_tool_updates_row_in_place() {
        let mut timeline = Timeline::new(8);

        let idx = timeline.add_running_tool(
            "tool-1",
            "Read file",
            100,
            "reading README.md",
            "full tool input",
        );
        assert_eq!(idx, 0);

        let updated = timeline
            .finish_tool(
                "tool-1",
                175,
                TimelineStatus::Done,
                "read complete",
                "full tool output",
                Some(12),
                Some(7),
                Some(1.25),
            )
            .expect("tool row should exist");

        assert_eq!(updated, 0);
        let row = timeline.selected_row().unwrap();
        assert_eq!(row.id, "tool-1");
        assert_eq!(row.title, "Read file");
        assert_eq!(row.kind, TimelineKind::ToolCall);
        assert_eq!(row.status, TimelineStatus::Done);
        assert_eq!(row.started_at_ms, 100);
        assert_eq!(row.finished_at_ms, Some(175));
        assert_eq!(row.duration_ms(), Some(75));
        assert_eq!(row.token_delta_input, Some(12));
        assert_eq!(row.token_delta_output, Some(7));
        assert_eq!(row.cost_delta_usd, Some(1.25));
        assert_eq!(row.detail_preview, "read complete");
        assert_eq!(row.expandable_details, "full tool output");
        assert!(row.is_terminal());
        assert!(row.has_expandable_details());
    }

    #[test]
    fn add_turn_summary_captures_usage_and_duration() {
        let mut timeline = Timeline::new(8);

        let idx = timeline.add_turn_summary(
            "turn-3",
            "Turn 3",
            1_000,
            1_420,
            "assistant completed",
            "stop_reason=end_turn",
            Some(123),
            Some(77),
            Some(0.018),
        );

        assert_eq!(idx, 0);
        let row = timeline.last_row().expect("turn summary row should exist");
        assert_eq!(row.id, "turn-3");
        assert_eq!(row.kind, TimelineKind::TurnSummary);
        assert_eq!(row.status, TimelineStatus::Done);
        assert_eq!(row.duration_ms(), Some(420));
        assert_eq!(row.token_delta_input, Some(123));
        assert_eq!(row.token_delta_output, Some(77));
        assert_eq!(row.cost_delta_usd, Some(0.018));
    }

    #[test]
    fn prune_caps_rows_to_max_length() {
        let mut timeline = Timeline::new(3);

        timeline.add_status_note(
            "row-1",
            "note 1",
            1,
            TimelineStatus::Done,
            "first",
            "first details",
        );
        timeline.add_status_note(
            "row-2",
            "note 2",
            2,
            TimelineStatus::Done,
            "second",
            "second details",
        );
        timeline.add_status_note(
            "row-3",
            "note 3",
            3,
            TimelineStatus::Done,
            "third",
            "third details",
        );
        timeline.add_status_note(
            "row-4",
            "note 4",
            4,
            TimelineStatus::Done,
            "fourth",
            "fourth details",
        );

        assert_eq!(timeline.len(), 3);
        assert_eq!(timeline.rows[0].id, "row-2");
        assert_eq!(timeline.rows[1].id, "row-3");
        assert_eq!(timeline.rows[2].id, "row-4");
    }

    #[test]
    fn zero_cap_is_coerced_to_one_and_prunes_oldest_rows() {
        let mut timeline = Timeline::new(0);

        assert_eq!(timeline.max_rows, 1);

        timeline.add_status_note(
            "row-1",
            "note 1",
            1,
            TimelineStatus::Done,
            "first",
            "first details",
        );
        timeline.add_status_note(
            "row-2",
            "note 2",
            2,
            TimelineStatus::Done,
            "second",
            "second details",
        );

        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline.rows[0].id, "row-2");
    }

    #[test]
    fn selected_index_moves_with_front_prune() {
        let mut timeline = Timeline::new(3);

        timeline.add_running_tool("tool-1", "Tool 1", 10, "preview 1", "details 1");
        timeline.add_running_tool("tool-2", "Tool 2", 20, "preview 2", "details 2");
        timeline.add_running_tool("tool-3", "Tool 3", 30, "preview 3", "details 3");
        timeline.set_selected_idx(2);

        timeline.add_running_tool("tool-4", "Tool 4", 40, "preview 4", "details 4");

        assert_eq!(timeline.selected_idx, 1);
        assert_eq!(timeline.selected_row().unwrap().id, "tool-3");
        assert_eq!(
            timeline
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["tool-2", "tool-3", "tool-4"]
        );
    }

    #[test]
    fn selected_index_after_prune_stays_on_same_logical_row_when_retained() {
        let adjusted = Timeline::selected_index_after_prune(4, 2, 5);
        assert_eq!(adjusted, 2);
    }

    #[test]
    fn selected_index_after_prune_falls_back_to_first_when_selection_is_dropped() {
        let adjusted = Timeline::selected_index_after_prune(1, 3, 4);
        assert_eq!(adjusted, 0);
    }

    #[test]
    fn shrinking_max_rows_keeps_selection_stable() {
        let mut timeline = Timeline::new(5);

        timeline.add_status_note("row-1", "note 1", 1, TimelineStatus::Done, "1", "1");
        timeline.add_status_note("row-2", "note 2", 2, TimelineStatus::Done, "2", "2");
        timeline.add_status_note("row-3", "note 3", 3, TimelineStatus::Done, "3", "3");
        timeline.add_status_note("row-4", "note 4", 4, TimelineStatus::Done, "4", "4");
        timeline.add_status_note("row-5", "note 5", 5, TimelineStatus::Done, "5", "5");
        timeline.set_selected_idx(3);

        let removed = timeline.set_max_rows(3);

        assert_eq!(removed, 2);
        assert_eq!(timeline.selected_idx, 1);
        assert_eq!(
            timeline.selected_row().map(|row| row.id.as_str()),
            Some("row-4")
        );
    }
}
