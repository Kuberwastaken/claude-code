//! Stats dialog — mirrors src/components/Stats.tsx
//!
//! Four-tab overlay: Overview | Daily Tokens | Cost Heatmap | Models
//! Data source: ~/.claurst/stats.jsonl (append-only per-turn usage log)

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::overlays::{
    begin_modal_buf, modal_header_line_area, render_modal_title_buf, CLAURST_ACCENT, CLAURST_MUTED,
    CLAURST_PANEL_BG,
};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A single entry in ~/.claurst/stats.jsonl
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatsEntry {
    pub timestamp_ms: u64,
    pub session_id: Option<String>,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// Cost in USD cents (f64)
    pub cost_cents: f64,
    pub project: Option<String>,
}

/// Aggregated stats for display.
#[derive(Debug, Clone, Default)]
pub struct AggregatedStats {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_cents: f64,
    pub by_model: HashMap<String, ModelStats>,
    /// (date_str "YYYY-MM-DD", tokens) pairs sorted by date
    pub daily_tokens: Vec<(String, u64)>,
    /// (date_str "YYYY-MM-DD", cost_cents) for heatmap
    pub daily_costs: HashMap<String, f64>,
    pub peak_day: Option<String>,
    pub peak_day_tokens: u64,
}

/// Per-model usage stats (used in AggregatedStats.by_model).
#[derive(Debug, Clone, Default)]
pub struct ModelStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_cents: f64,
    pub turns: u64,
}

/// Per-model breakdown entry for the Models tab (cost in USD, not cents).
#[derive(Debug, Clone, Default)]
pub struct ModelBreakdown {
    pub model_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

// ---------------------------------------------------------------------------
// Data loading
// ---------------------------------------------------------------------------

/// Load and aggregate stats from ~/.claurst/stats.jsonl
pub fn load_stats() -> AggregatedStats {
    let path = dirs::home_dir()
        .map(|h| h.join(".claurst").join("stats.jsonl"))
        .unwrap_or_default();

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return AggregatedStats::default(),
    };

    let mut agg = AggregatedStats::default();
    let mut daily: HashMap<String, u64> = HashMap::new();

    for line in content.lines() {
        let Ok(entry) = serde_json::from_str::<StatsEntry>(line) else {
            continue;
        };

        let total_tokens = entry.input_tokens + entry.output_tokens;
        agg.total_input_tokens += entry.input_tokens;
        agg.total_output_tokens += entry.output_tokens;
        agg.total_cost_cents += entry.cost_cents;

        let model_entry = agg.by_model.entry(entry.model.clone()).or_default();
        model_entry.input_tokens += entry.input_tokens;
        model_entry.output_tokens += entry.output_tokens;
        model_entry.cost_cents += entry.cost_cents;
        model_entry.turns += 1;

        // Date from timestamp
        let date = timestamp_to_date(entry.timestamp_ms);
        *daily.entry(date.clone()).or_insert(0) += total_tokens;
        *agg.daily_costs.entry(date).or_insert(0.0) += entry.cost_cents;
    }

    // Build sorted daily_tokens
    let mut daily_sorted: Vec<(String, u64)> = daily.into_iter().collect();
    daily_sorted.sort_by(|a, b| a.0.cmp(&b.0));
    agg.peak_day = daily_sorted.iter().max_by_key(|d| d.1).map(|d| d.0.clone());
    agg.peak_day_tokens = daily_sorted.iter().map(|d| d.1).max().unwrap_or(0);
    agg.daily_tokens = daily_sorted;

    agg
}

fn timestamp_to_date(ts_ms: u64) -> String {
    // Simple ISO date from Unix timestamp in ms
    let secs = ts_ms / 1000;
    let days_since_epoch = secs / 86400;
    // Rough Gregorian calendar calculation
    let year = 1970 + (days_since_epoch * 4 + 2) / 1461;
    let day_of_year = days_since_epoch - (year - 1970) * 365 - (year - 1970 - 1) / 4;
    let (month, day) = day_of_year_to_month_day(day_of_year as u32, is_leap_year(year as u32));
    format!("{:04}-{:02}-{:02}", year, month, day)
}

fn is_leap_year(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn day_of_year_to_month_day(doy: u32, leap: bool) -> (u32, u32) {
    let months = if leap {
        [31u32, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31u32, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut remaining = doy;
    for (i, &m) in months.iter().enumerate() {
        if remaining < m {
            return (i as u32 + 1, remaining + 1);
        }
        remaining -= m;
    }
    (12, 31)
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsTab {
    Overview,
    DailyTokens,
    CostHeatmap,
    Models,
}

#[derive(Debug, Clone)]
pub struct StatsDialogState {
    pub open: bool,
    pub tab: StatsTab,
    pub range_days: u32, // 7, 30, or 0 = all
    pub data: Option<AggregatedStats>,
    pub scroll: u16,
    /// Per-model breakdown for the Models tab (cost in USD).
    pub model_breakdown: Vec<ModelBreakdown>,
    /// How many consecutive days the user has had activity (ending today).
    pub current_streak_days: u32,
    /// The longest streak ever recorded.
    pub longest_streak_days: u32,
}

impl StatsDialogState {
    pub fn new() -> Self {
        Self {
            open: false,
            tab: StatsTab::Overview,
            range_days: 30,
            data: None,
            scroll: 0,
            model_breakdown: Vec::new(),
            current_streak_days: 0,
            longest_streak_days: 0,
        }
    }

    pub fn open(&mut self) {
        let stats = load_stats();
        self.model_breakdown = build_model_breakdown(&stats);
        let (current, longest) = compute_streaks(&stats);
        self.current_streak_days = current;
        self.longest_streak_days = longest;
        self.data = Some(stats);
        self.open = true;
        self.scroll = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn next_tab(&mut self) {
        self.tab = match self.tab {
            StatsTab::Overview => StatsTab::DailyTokens,
            StatsTab::DailyTokens => StatsTab::CostHeatmap,
            StatsTab::CostHeatmap => StatsTab::Models,
            StatsTab::Models => StatsTab::Overview,
        };
        self.scroll = 0;
    }

    pub fn prev_tab(&mut self) {
        self.tab = match self.tab {
            StatsTab::Overview => StatsTab::Models,
            StatsTab::DailyTokens => StatsTab::Overview,
            StatsTab::CostHeatmap => StatsTab::DailyTokens,
            StatsTab::Models => StatsTab::CostHeatmap,
        };
        self.scroll = 0;
    }

    pub fn cycle_range(&mut self) {
        self.range_days = match self.range_days {
            7 => 30,
            30 => 0,
            _ => 7,
        };
    }

    /// Record usage for a model, accumulating into `model_breakdown`.
    /// `cost` is in USD (not cents).
    pub fn add_model_usage(&mut self, model_id: &str, input: u64, output: u64, cost: f64) {
        if let Some(entry) = self
            .model_breakdown
            .iter_mut()
            .find(|e| e.model_id == model_id)
        {
            entry.input_tokens += input;
            entry.output_tokens += output;
            entry.cost_usd += cost;
        } else {
            self.model_breakdown.push(ModelBreakdown {
                model_id: model_id.to_string(),
                input_tokens: input,
                output_tokens: output,
                cost_usd: cost,
            });
        }
    }
}

impl Default for StatsDialogState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers: build model breakdown and compute streaks
// ---------------------------------------------------------------------------

fn build_model_breakdown(stats: &AggregatedStats) -> Vec<ModelBreakdown> {
    let mut breakdown: Vec<ModelBreakdown> = stats
        .by_model
        .iter()
        .map(|(model_id, ms)| ModelBreakdown {
            model_id: model_id.clone(),
            input_tokens: ms.input_tokens,
            output_tokens: ms.output_tokens,
            cost_usd: ms.cost_cents / 100.0,
        })
        .collect();
    breakdown.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    breakdown
}

/// Compute (current_streak, longest_streak) in days from the aggregated stats.
/// A streak is a consecutive run of calendar days with any activity, ending on
/// the most-recent active day.
fn compute_streaks(stats: &AggregatedStats) -> (u32, u32) {
    if stats.daily_tokens.is_empty() {
        return (0, 0);
    }

    // Collect sorted unique active dates
    let mut dates: Vec<&str> = stats.daily_tokens.iter().map(|(d, _)| d.as_str()).collect();
    dates.dedup();

    let mut longest: u32 = 1;
    let mut current_run: u32 = 1;

    for window in dates.windows(2) {
        if consecutive_dates(window[0], window[1]) {
            current_run += 1;
            if current_run > longest {
                longest = current_run;
            }
        } else {
            current_run = 1;
        }
    }

    // The "current" streak is the run ending on the last active date.
    // Recompute from the end.
    let mut current_streak: u32 = 1;
    for window in dates.windows(2).rev() {
        if consecutive_dates(window[0], window[1]) {
            current_streak += 1;
        } else {
            break;
        }
    }

    (current_streak, longest)
}

/// Returns true when `next` is exactly one calendar day after `prev`.
/// Both strings must be "YYYY-MM-DD".
fn consecutive_dates(prev: &str, next: &str) -> bool {
    let prev_days = date_to_days_since_epoch(prev);
    let next_days = date_to_days_since_epoch(next);
    match (prev_days, next_days) {
        (Some(p), Some(n)) => n == p + 1,
        _ => false,
    }
}

fn date_to_days_since_epoch(date: &str) -> Option<u64> {
    // Expect "YYYY-MM-DD"
    if date.len() != 10 {
        return None;
    }
    let year: u64 = date[0..4].parse().ok()?;
    let month: u64 = date[5..7].parse().ok()?;
    let day: u64 = date[8..10].parse().ok()?;
    // Days from 1970-01-01 (approximate, good enough for streak detection)
    let y = year - 1970;
    let leap_days = if y > 0 {
        (y - 1) / 4 - (y - 1) / 100 + (y - 1) / 400 + 1
    } else {
        0
    };
    let days_in_years = y * 365 + leap_days;
    let leap = is_leap_year(year as u32);
    let months = if leap {
        [0u64, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335]
    } else {
        [0u64, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334]
    };
    let month_days = months.get((month as usize).saturating_sub(1))?;
    Some(days_in_years + month_days + day - 1)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the stats dialog overlay.
pub fn render_stats_dialog(state: &StatsDialogState, area: Rect, buf: &mut Buffer) {
    if !state.open {
        return;
    }

    let layout = begin_modal_buf(buf, area, 92, 30, 2, 1);
    render_modal_title_buf(buf, layout.header_area, "Cost & stats", "esc");

    let tab_line = Line::from(vec![
        tab_span("Overview", state.tab == StatsTab::Overview),
        Span::styled("  ·  ", Style::default().fg(CLAURST_MUTED)),
        tab_span("Daily Tokens", state.tab == StatsTab::DailyTokens),
        Span::styled("  ·  ", Style::default().fg(CLAURST_MUTED)),
        tab_span("Cost Heatmap", state.tab == StatsTab::CostHeatmap),
        Span::styled("  ·  ", Style::default().fg(CLAURST_MUTED)),
        tab_span("Models", state.tab == StatsTab::Models),
    ]);
    if let Some(tab_area) = modal_header_line_area(layout.header_area, 1) {
        Paragraph::new(tab_line).render(tab_area, buf);
    }

    let content_area = layout.body_area;

    let Some(data) = &state.data else {
        Paragraph::new("Loading\u{2026}")
            .style(Style::default().fg(CLAURST_MUTED).bg(CLAURST_PANEL_BG))
            .render(content_area, buf);
        return;
    };

    match state.tab {
        StatsTab::Overview => render_overview(data, state, content_area, buf),
        StatsTab::DailyTokens => render_daily_tokens(data, state.range_days, content_area, buf),
        StatsTab::CostHeatmap => render_cost_heatmap(data, content_area, buf),
        StatsTab::Models => render_models(state, content_area, buf),
    }
    Paragraph::new(Line::from(vec![Span::styled(
        " tab/←/→ switch tabs  ·  r cycle range  ·  ↑↓ scroll",
        Style::default()
            .fg(CLAURST_MUTED)
            .add_modifier(Modifier::ITALIC),
    )]))
    .render(layout.footer_area, buf);
}

fn tab_span(label: &str, active: bool) -> Span<'static> {
    if active {
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(CLAURST_ACCENT)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
    } else {
        Span::styled(label.to_string(), Style::default().fg(CLAURST_MUTED))
    }
}

// ---------------------------------------------------------------------------
// Overview tab
// ---------------------------------------------------------------------------

fn render_overview(data: &AggregatedStats, state: &StatsDialogState, area: Rect, buf: &mut Buffer) {
    let total_tokens = data.total_input_tokens + data.total_output_tokens;
    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("Total tokens: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format_tokens(total_tokens),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Input:    ", Style::default().fg(Color::DarkGray)),
        Span::raw(format_tokens(data.total_input_tokens)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Output:   ", Style::default().fg(Color::DarkGray)),
        Span::raw(format_tokens(data.total_output_tokens)),
    ]));
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("Total cost: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("${:.2}", data.total_cost_cents / 100.0),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Streak display
    lines.push(Line::default());
    {
        let current = state.current_streak_days;
        let longest = state.longest_streak_days;
        let streak_value = Span::styled(
            format!("● {} day{}", current, if current == 1 { "" } else { "s" }),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
        let streak_longest = Span::styled(
            format!(
                "  (longest: {} day{})",
                longest,
                if longest == 1 { "" } else { "s" }
            ),
            Style::default().fg(Color::DarkGray),
        );
        lines.push(Line::from(vec![
            Span::styled("Streak: ", Style::default().fg(Color::DarkGray)),
            streak_value,
            streak_longest,
        ]));
    }

    if let Some(peak) = &data.peak_day {
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("Peak day: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} ({} tokens)", peak, format_tokens(data.peak_day_tokens)),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    }

    if !data.by_model.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(vec![Span::styled(
            "By model:",
            Style::default().fg(Color::DarkGray),
        )]));
        let mut models: Vec<_> = data.by_model.iter().collect();
        models.sort_by(|a, b| {
            b.1.cost_cents
                .partial_cmp(&a.1.cost_cents)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for (model, stats) in models.iter().take(5) {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:40} ", model), Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!(
                        "{} turns  {}",
                        stats.turns,
                        format_tokens(stats.input_tokens + stats.output_tokens)
                    ),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("  ${:.2}", stats.cost_cents / 100.0),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    Paragraph::new(lines).render(area, buf);
}

// ---------------------------------------------------------------------------
// Daily Tokens tab
// ---------------------------------------------------------------------------

fn render_daily_tokens(data: &AggregatedStats, range_days: u32, area: Rect, buf: &mut Buffer) {
    // Filter to range
    let filtered: Vec<_> = if range_days == 0 {
        data.daily_tokens.iter().collect()
    } else {
        data.daily_tokens
            .iter()
            .rev()
            .take(range_days as usize)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };

    if filtered.is_empty() {
        Paragraph::new("No data yet.")
            .style(Style::default().fg(Color::DarkGray))
            .render(area, buf);
        return;
    }

    let range_label = match range_days {
        7 => "7 days",
        30 => "30 days",
        _ => "all time",
    };
    let label_line = Line::from(vec![Span::styled(
        format!("Range: {} [r: cycle]", range_label),
        Style::default().fg(Color::DarkGray),
    )]);
    Paragraph::new(label_line).render(
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
        buf,
    );

    let chart_area = Rect {
        x: area.x,
        y: area.y + 2,
        width: area.width,
        height: area.height.saturating_sub(2),
    };

    // Build bar chart data
    let max_val = filtered.iter().map(|d| d.1).max().unwrap_or(1).max(1);
    let bar_data: Vec<(&str, u64)> = filtered
        .iter()
        .map(|d| {
            let label: &str = if d.0.len() >= 5 {
                &d.0[5..]
            } else {
                d.0.as_str()
            };
            (label, d.1 * (chart_area.height as u64 - 1) / max_val)
        })
        .collect();

    // Render ASCII bar chart manually (ratatui BarChart needs 'static strs)
    for (i, (label, height)) in bar_data.iter().enumerate() {
        let x = chart_area.x + i as u16 * 6;
        if x + 5 >= chart_area.x + chart_area.width {
            break;
        }
        let bar_height = (*height as u16).min(chart_area.height.saturating_sub(1));
        for row in 0..bar_height {
            let y = chart_area.y + chart_area.height - 1 - row;
            let cell = buf.cell_mut((x + 1, y));
            if let Some(c) = cell {
                c.set_symbol("\u{2588}");
                c.set_style(Style::default().fg(Color::Cyan));
            }
            let cell2 = buf.cell_mut((x + 2, y));
            if let Some(c) = cell2 {
                c.set_symbol("\u{2588}");
                c.set_style(Style::default().fg(Color::Cyan));
            }
        }
        // Label
        let y = chart_area.y + chart_area.height - 1;
        let label_short: String = label.chars().take(4).collect();
        for (j, ch) in label_short.chars().enumerate() {
            let cell = buf.cell_mut((x + j as u16, y));
            if let Some(c) = cell {
                c.set_symbol(&ch.to_string());
                c.set_style(Style::default().fg(Color::DarkGray));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cost Heatmap tab (GitHub-style)
// ---------------------------------------------------------------------------

fn render_cost_heatmap(data: &AggregatedStats, area: Rect, buf: &mut Buffer) {
    if data.daily_costs.is_empty() {
        Paragraph::new("No cost data yet.")
            .style(Style::default().fg(Color::DarkGray))
            .render(area, buf);
        return;
    }

    let max_cost = data
        .daily_costs
        .values()
        .cloned()
        .fold(0.0_f64, f64::max)
        .max(0.01);

    // Header legend
    Paragraph::new(Line::from(vec![
        Span::styled(
            "Cost Heatmap (last 12 weeks)   no activity ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("\u{25a0}", Style::default().fg(Color::Rgb(30, 30, 30))),
        Span::styled(" low ", Style::default().fg(Color::DarkGray)),
        Span::styled("\u{25a0}", Style::default().fg(Color::Rgb(0, 100, 0))),
        Span::styled(" med ", Style::default().fg(Color::DarkGray)),
        Span::styled("\u{25a0}", Style::default().fg(Color::Rgb(0, 200, 0))),
        Span::styled(" high ", Style::default().fg(Color::DarkGray)),
        Span::styled("\u{25a0}", Style::default().fg(Color::Rgb(0, 255, 0))),
    ]))
    .render(
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
        buf,
    );

    // Weekday labels column (Mon..Sun order)
    let weekday_labels = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let heatmap_area = Rect {
        x: area.x + 4, // leave 4 cols for "Mon" etc.
        y: area.y + 2,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(3),
    };

    for (i, label) in weekday_labels.iter().enumerate() {
        let y = heatmap_area.y + i as u16;
        if y >= heatmap_area.y + heatmap_area.height {
            break;
        }
        Paragraph::new(Line::from(vec![Span::styled(
            label.to_string(),
            Style::default().fg(Color::DarkGray),
        )]))
        .render(
            Rect {
                x: area.x,
                y,
                width: 3,
                height: 1,
            },
            buf,
        );
    }

    // 12 weeks x 7 days grid — sorted ascending, display newest on right
    let sorted_dates: Vec<_> = {
        let mut v: Vec<_> = data.daily_costs.iter().collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v
    };

    // We group into chunks of 7 calendar days (by index, as in the original)
    // and place week columns right-to-left from the most-recent week.
    let chunks: Vec<_> = sorted_dates.chunks(7).collect();
    let total_chunks = chunks.len();
    let start_chunk = if total_chunks > 12 {
        total_chunks - 12
    } else {
        0
    };

    for (display_col, chunk) in chunks[start_chunk..].iter().enumerate() {
        let x = heatmap_area.x + display_col as u16 * 2;
        if x >= heatmap_area.x + heatmap_area.width {
            break;
        }
        for (day_idx, (_, cost)) in chunk.iter().enumerate() {
            let y = heatmap_area.y + day_idx as u16;
            if y >= heatmap_area.y + heatmap_area.height {
                break;
            }
            let intensity = (*cost / max_cost).min(1.0);
            let color = heatmap_color(intensity);
            let cell = buf.cell_mut((x, y));
            if let Some(c) = cell {
                c.set_symbol("\u{25a0}");
                c.set_style(Style::default().fg(color));
            }
        }
    }
}

/// Map a 0..=1 intensity to a green-shade color matching the GitHub heatmap spec.
fn heatmap_color(intensity: f64) -> Color {
    if intensity < 0.01 {
        Color::Rgb(30, 30, 30)
    } else if intensity < 0.25 {
        Color::Rgb(0, 100, 0)
    } else if intensity < 0.50 {
        Color::Rgb(0, 150, 0)
    } else if intensity < 0.75 {
        Color::Rgb(0, 200, 0)
    } else {
        Color::Rgb(0, 255, 0)
    }
}

// ---------------------------------------------------------------------------
// Models tab
// ---------------------------------------------------------------------------

fn render_models(state: &StatsDialogState, area: Rect, buf: &mut Buffer) {
    if state.model_breakdown.is_empty() {
        Paragraph::new("No model usage data yet.")
            .style(Style::default().fg(Color::DarkGray))
            .render(area, buf);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    // Table header
    lines.push(Line::from(vec![Span::styled(
        format!(
            "{:<42} {:>12} {:>13} {:>10}",
            "Model", "Input", "Output", "Cost"
        ),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )]));
    // Separator
    lines.push(Line::from(vec![Span::styled(
        "\u{2500}".repeat(area.width.saturating_sub(2) as usize),
        Style::default().fg(Color::DarkGray),
    )]));

    let mut total_input: u64 = 0;
    let mut total_output: u64 = 0;
    let mut total_cost: f64 = 0.0;

    for entry in &state.model_breakdown {
        total_input += entry.input_tokens;
        total_output += entry.output_tokens;
        total_cost += entry.cost_usd;

        // Truncate long model IDs
        let model_display = if entry.model_id.len() > 42 {
            format!("{}...", &entry.model_id[..39])
        } else {
            entry.model_id.clone()
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<42} ", model_display),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(
                format!("{:>12} ", format_tokens(entry.input_tokens)),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!("{:>13} ", format_tokens(entry.output_tokens)),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!("{:>9}", format!("${:.4}", entry.cost_usd)),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    }

    // Grand total separator + row
    lines.push(Line::from(vec![Span::styled(
        "\u{2500}".repeat(area.width.saturating_sub(2) as usize),
        Style::default().fg(Color::DarkGray),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<42} ", "TOTAL"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>12} ", format_tokens(total_input)),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>13} ", format_tokens(total_output)),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>9}", format!("${:.4}", total_cost)),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    Paragraph::new(lines).render(area, buf);
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.0}K", n as f64 / 1_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers -----------------------------------------------------------

    fn make_state_with_models(entries: &[(&str, u64, u64, f64)]) -> StatsDialogState {
        let mut state = StatsDialogState::new();
        for (model, input, output, cost) in entries {
            state.add_model_usage(model, *input, *output, *cost);
        }
        state
    }

    fn make_agg_with_dates(dates: &[&str]) -> AggregatedStats {
        let mut agg = AggregatedStats::default();
        for date in dates {
            agg.daily_tokens.push((date.to_string(), 100));
        }
        agg
    }

    // ---- model breakdown: add_model_usage ----------------------------------

    #[test]
    fn test_add_model_usage_new_model() {
        let mut state = StatsDialogState::new();
        state.add_model_usage("claude-3-opus", 1000, 500, 0.05);

        assert_eq!(state.model_breakdown.len(), 1);
        let e = &state.model_breakdown[0];
        assert_eq!(e.model_id, "claude-3-opus");
        assert_eq!(e.input_tokens, 1000);
        assert_eq!(e.output_tokens, 500);
        assert!((e.cost_usd - 0.05).abs() < 1e-9);
    }

    #[test]
    fn test_add_model_usage_accumulates_same_model() {
        let mut state = StatsDialogState::new();
        state.add_model_usage("claude-3-opus", 1000, 500, 0.05);
        state.add_model_usage("claude-3-opus", 2000, 800, 0.10);

        assert_eq!(state.model_breakdown.len(), 1);
        let e = &state.model_breakdown[0];
        assert_eq!(e.input_tokens, 3000);
        assert_eq!(e.output_tokens, 1300);
        assert!((e.cost_usd - 0.15).abs() < 1e-9);
    }

    #[test]
    fn test_add_model_usage_multiple_models() {
        let state = make_state_with_models(&[
            ("claude-3-opus", 1000, 500, 0.05),
            ("claude-3-haiku", 500, 200, 0.01),
            ("claude-3-sonnet", 800, 400, 0.03),
        ]);

        assert_eq!(state.model_breakdown.len(), 3);
        let ids: Vec<&str> = state
            .model_breakdown
            .iter()
            .map(|e| e.model_id.as_str())
            .collect();
        assert!(ids.contains(&"claude-3-opus"));
        assert!(ids.contains(&"claude-3-haiku"));
        assert!(ids.contains(&"claude-3-sonnet"));
    }

    #[test]
    fn test_model_breakdown_totals() {
        let state = make_state_with_models(&[
            ("model-a", 1_000_000, 200_000, 1.00),
            ("model-b", 500_000, 100_000, 0.50),
        ]);
        let total_input: u64 = state.model_breakdown.iter().map(|e| e.input_tokens).sum();
        let total_output: u64 = state.model_breakdown.iter().map(|e| e.output_tokens).sum();
        let total_cost: f64 = state.model_breakdown.iter().map(|e| e.cost_usd).sum();
        assert_eq!(total_input, 1_500_000);
        assert_eq!(total_output, 300_000);
        assert!((total_cost - 1.50).abs() < 1e-9);
    }

    // ---- streak tracking ---------------------------------------------------

    #[test]
    fn test_streak_consecutive_days() {
        let agg = make_agg_with_dates(&["2025-01-01", "2025-01-02", "2025-01-03"]);
        let (current, longest) = compute_streaks(&agg);
        assert_eq!(current, 3);
        assert_eq!(longest, 3);
    }

    #[test]
    fn test_streak_gap_resets_current() {
        // Two separate runs: 3 days then a gap, then 2 days.
        let agg = make_agg_with_dates(&[
            "2025-01-01",
            "2025-01-02",
            "2025-01-03",
            "2025-01-10",
            "2025-01-11",
        ]);
        let (current, longest) = compute_streaks(&agg);
        assert_eq!(current, 2);
        assert_eq!(longest, 3);
    }

    #[test]
    fn test_streak_single_day() {
        let agg = make_agg_with_dates(&["2025-03-15"]);
        let (current, longest) = compute_streaks(&agg);
        assert_eq!(current, 1);
        assert_eq!(longest, 1);
    }

    #[test]
    fn test_streak_empty() {
        let agg = AggregatedStats::default();
        let (current, longest) = compute_streaks(&agg);
        assert_eq!(current, 0);
        assert_eq!(longest, 0);
    }

    #[test]
    fn test_streak_longer_tail_wins_longest() {
        // Five days, then a gap, then one day.
        let agg = make_agg_with_dates(&[
            "2025-02-01",
            "2025-02-02",
            "2025-02-03",
            "2025-02-04",
            "2025-02-05",
            "2025-02-20",
        ]);
        let (current, longest) = compute_streaks(&agg);
        assert_eq!(current, 1);
        assert_eq!(longest, 5);
    }

    #[test]
    fn test_consecutive_dates_helper() {
        assert!(consecutive_dates("2025-01-31", "2025-02-01"));
        assert!(consecutive_dates("2024-02-28", "2024-02-29")); // 2024 is a leap year
        assert!(!consecutive_dates("2025-01-01", "2025-01-03"));
        assert!(!consecutive_dates("2025-01-05", "2025-01-04")); // reversed
    }

    // ---- heatmap color -----------------------------------------------------

    #[test]
    fn test_heatmap_color_zero() {
        assert_eq!(heatmap_color(0.0), Color::Rgb(30, 30, 30));
    }

    #[test]
    fn test_heatmap_color_max() {
        assert_eq!(heatmap_color(1.0), Color::Rgb(0, 255, 0));
    }

    #[test]
    fn test_heatmap_color_mid() {
        // 0.60 -> high bracket
        assert_eq!(heatmap_color(0.60), Color::Rgb(0, 200, 0));
    }

    // ---- build_model_breakdown sorting -------------------------------------

    #[test]
    fn test_build_model_breakdown_sorted_by_cost_desc() {
        let mut agg = AggregatedStats::default();
        agg.by_model.insert(
            "cheap".to_string(),
            ModelStats {
                input_tokens: 100,
                output_tokens: 50,
                cost_cents: 10.0,
                turns: 1,
            },
        );
        agg.by_model.insert(
            "expensive".to_string(),
            ModelStats {
                input_tokens: 200,
                output_tokens: 100,
                cost_cents: 500.0,
                turns: 2,
            },
        );
        agg.by_model.insert(
            "mid".to_string(),
            ModelStats {
                input_tokens: 150,
                output_tokens: 75,
                cost_cents: 100.0,
                turns: 1,
            },
        );

        let breakdown = build_model_breakdown(&agg);
        assert_eq!(breakdown[0].model_id, "expensive");
        assert_eq!(breakdown[1].model_id, "mid");
        assert_eq!(breakdown[2].model_id, "cheap");
    }
}
