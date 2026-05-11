//! Model picker overlay (/model command).
//! Mirrors src/components/ModelPicker.tsx — including effort levels and
//! fast-mode notice.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::overlays::{centered_rect, modal_search_line, CLAURST_PANEL_BG};

// ---------------------------------------------------------------------------
// Effort level
// ---------------------------------------------------------------------------

/// Mirrors the TS `EffortLevel` enum and `effortLevelToSymbol()` helper.
///
/// Effort controls the extended-thinking `budget_tokens` parameter sent to the
/// API. Only models that support extended thinking honour this; for others it
/// is silently ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    Low,
    Normal,
    High,
    Max,
}

impl EffortLevel {
    /// Unicode quarter-circle symbol used in the TS UI.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Low    => "\u{25cb}", // ○  empty circle
            Self::Normal => "\u{25d0}", // ◐  half
            Self::High   => "\u{25d5}", // ◕  three-quarter
            Self::Max    => "\u{25cf}", // ●  full
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Low    => "low",
            Self::Normal => "normal",
            Self::High   => "high",
            Self::Max    => "max",
        }
    }

    /// Returns the budget_tokens value to pass to the API, or `None` for the
    /// default (no extended thinking).
    pub fn budget_tokens(self) -> Option<u32> {
        match self {
            Self::Low    => Some(1_024),
            Self::Normal => None,
            Self::High   => Some(16_000),
            Self::Max    => Some(32_000),
        }
    }

    /// Cycle to next level; skips `Max` when the selected model does not
    /// support it.
    pub fn next(self, supports_max: bool) -> Self {
        match self {
            Self::Low    => Self::Normal,
            Self::Normal => Self::High,
            Self::High   => if supports_max { Self::Max } else { Self::Low },
            Self::Max    => Self::Low,
        }
    }

    /// Cycle to previous level.
    pub fn prev(self, supports_max: bool) -> Self {
        match self {
            Self::Low    => if supports_max { Self::Max } else { Self::High },
            Self::Normal => Self::Low,
            Self::High   => Self::Normal,
            Self::Max    => Self::High,
        }
    }
}

impl Default for EffortLevel {
    fn default() -> Self { Self::Normal }
}

// ---------------------------------------------------------------------------
// Model capability helpers
// ---------------------------------------------------------------------------

/// Returns `true` for models that support extended thinking / effort levels.
pub fn model_supports_effort(id: &str) -> bool {
    id.starts_with("claude-3-7")
        || id.starts_with("claude-opus-4")
        || id.starts_with("claude-sonnet-4")
}

/// Returns `true` for models that support the maximum effort tier.
pub fn model_supports_max_effort(id: &str) -> bool {
    id.starts_with("claude-opus-4")
}

/// Returns a short description string based on the model family inferred from
/// the model ID.  Used when converting API model entries to `ModelEntry`.
pub fn model_family_description(id: &str) -> String {
    let lower = id.to_lowercase();
    if lower.contains("opus") {
        "Most capable — best for complex reasoning and analysis".to_string()
    } else if lower.contains("sonnet") {
        "Balanced performance and speed — great for coding tasks".to_string()
    } else if lower.contains("haiku") {
        "Fast and efficient — ideal for quick completions".to_string()
    } else {
        "AI model".to_string()
    }
}

// ---------------------------------------------------------------------------
// Provider grouping helpers
// ---------------------------------------------------------------------------

/// Format context window tokens for display in the model picker.
pub fn format_context_window(context_window: u32) -> String {
    if context_window >= 1_000_000 {
        if context_window % 1_000_000 == 0 {
            format!("{}M context", context_window / 1_000_000)
        } else {
            format!("{:.1}M context", context_window as f64 / 1_000_000.0)
        }
    } else {
        format!("{}K context", context_window / 1000)
    }
}

/// Format a model display line with optional context window and cost info.
///
/// Example: `"gpt-4o  128K ctx  $5.00/M"`
pub fn format_model_line(model_str: &str, context_window: Option<u32>, cost_per_1m: Option<f64>) -> String {
    let mut parts = vec![model_str.to_string()];
    if let Some(ctx) = context_window {
        parts.push(format_context_window(ctx).replace(" context", " ctx"));
    }
    if let Some(cost) = cost_per_1m {
        if cost == 0.0 {
            parts.push("free".to_string());
        } else {
            parts.push(format!("${:.2}/M", cost));
        }
    }
    parts.join("  ")
}

/// A group of models belonging to the same provider, for structured display.
pub struct ProviderSection {
    pub provider_name: String,
    pub models: Vec<String>, // model ID strings in "provider/model" format
}

impl ModelPickerState {
    /// Build grouped model sections from a flat list of model strings.
    ///
    /// Models with a `"provider/model"` slash format are grouped by their
    /// provider prefix.  Bare model names are heuristically assigned to a
    /// provider based on the model name pattern.
    pub fn build_provider_sections(models: &[String]) -> Vec<ProviderSection> {
        use std::collections::HashMap;
        let mut by_provider: HashMap<String, Vec<String>> = HashMap::new();

        for m in models {
            let provider = if let Some((p, _)) = m.split_once('/') {
                p.to_string()
            } else {
                // Bare model name — detect provider from model name
                if m.contains("claude") {
                    "anthropic".to_string()
                } else if m.starts_with("gpt") || m.starts_with("o3") || m.starts_with("o4") {
                    "openai".to_string()
                } else if m.contains("gemini") {
                    "google".to_string()
                } else if m.contains("minimax") {
                    "minimax".to_string()
                } else {
                    "other".to_string()
                }
            };
            by_provider.entry(provider).or_default().push(m.clone());
        }

        // Define display order
        let order = ["anthropic", "openai", "google", "ollama", "other"];
        let mut sections = Vec::new();
        for provider in order {
            if let Some(models) = by_provider.remove(provider) {
                sections.push(ProviderSection {
                    provider_name: match provider {
                        "anthropic" => "ANTHROPIC".to_string(),
                        "openai" => "OPENAI".to_string(),
                        "google" => "GOOGLE".to_string(),
                        "ollama" => "OLLAMA (local)".to_string(),
                        _ => provider.to_uppercase(),
                    },
                    models,
                });
            }
        }
        // Add any remaining providers not in the order list
        for (provider, models) in by_provider {
            sections.push(ProviderSection {
                provider_name: provider.to_uppercase(),
                models,
            });
        }
        sections
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single model entry shown in the picker.
#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub id: String,
    pub display_name: String,
    pub description: String,
    /// Whether this is the currently active model.
    pub is_current: bool,
}

// ---------------------------------------------------------------------------
// Provider-aware model lists
// ---------------------------------------------------------------------------

/// Helper to build a `ModelEntry` with `is_current = false`.
fn model_entry(id: &str, name: &str, desc: &str) -> ModelEntry {
    ModelEntry {
        id: id.to_string(),
        display_name: name.to_string(),
        description: desc.to_string(),
        is_current: false,
    }
}

/// Get models for a provider from the model registry (models.dev data).
///
/// Builds picker entries from the bundled / network-refreshed registry.
/// The registry is always populated (the embedded models.dev snapshot
/// contains ~118 providers / ~4500 models), so the only time the result
/// is empty is when the caller passed a truly unknown provider id — in
/// which case we synthesize a single `"default"` placeholder so the
/// picker isn't blank.
pub fn models_for_provider_from_registry(
    provider_id: &str,
    registry: &claurst_api::ModelRegistry,
) -> Vec<ModelEntry> {
    // "free" is the composite Zen → OpenRouter provider; the upstream
    // models.dev catalog has nothing under this id, so serve a curated list
    // directly.  `free/auto` is the default routing entry; the rest pin a
    // specific upstream model for users who care.
    if provider_id == "free" {
        return free_provider_models();
    }

    let mut entries = registry.list_visible_by_provider(provider_id);

    // Fall back to all entries (including alpha/deprecated) if the visible
    // filter wiped the list — better to show something than nothing.
    if entries.is_empty() {
        entries = registry.list_by_provider(provider_id);
    }

    if entries.is_empty() {
        // Truly unknown provider — keep the picker non-empty so /model still
        // works against e.g. self-hosted endpoints.
        return vec![model_entry(
            "default",
            "Default model",
            "no catalog entry for this provider",
        )];
    }

    // Sort: most recently released first, then alphabetical by id.
    entries.sort_by(|a, b| {
        let rd_a = a.release_date.as_deref().unwrap_or("");
        let rd_b = b.release_date.as_deref().unwrap_or("");
        rd_b.cmp(rd_a).then_with(|| (*a.info.id).cmp(&*b.info.id))
    });

    entries
        .iter()
        .map(|e| {
            let cost_str = match (e.cost_input, e.cost_output) {
                (Some(ci), Some(co)) => format!(
                    "{} | ${:.2}/${:.2} per M",
                    format_context_window(e.info.context_window),
                    ci,
                    co
                ),
                _ => format_context_window(e.info.context_window),
            };
            ModelEntry {
                id: e.info.id.to_string(),
                display_name: e.info.name.clone(),
                description: cost_str,
                is_current: false,
            }
        })
        .collect()
}

/// Return the provider-prefixed default model name for a given provider,
/// consulting the registry first and falling back to a `provider/default`
/// placeholder for unknown providers.
///
/// **Anthropic exception** — anthropic models are emitted bare (no
/// `anthropic/` prefix) for backward-compatibility with config files that
/// pre-date the multi-provider era.
///
/// **Free exception** — the composite Zen → OpenRouter provider ships with
/// a synthetic `free/auto` default that the wrapper translates per upstream.
pub fn default_model_for_provider(
    provider_id: &str,
    registry: &claurst_api::ModelRegistry,
) -> String {
    if provider_id == "free" {
        return "free/auto".to_string();
    }
    if let Some(best) = registry.best_model_for_provider(provider_id) {
        if provider_id == "anthropic" {
            best
        } else {
            format!("{}/{}", provider_id, best)
        }
    } else {
        format!("{}/default", provider_id)
    }
}

/// Curated free-mode model list used by `models_for_provider_from_registry`.
fn free_provider_models() -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            id: "free/auto".to_string(),
            display_name: "Auto (Zen \u{2192} OpenRouter)".to_string(),
            description: "200K ctx | $0.00/$0.00 per M".to_string(),
            is_current: false,
        },
        ModelEntry {
            id: "zen/minimax-m2.5-free".to_string(),
            display_name: "MiniMax M2.5 (Free, via Zen)".to_string(),
            description: "200K ctx | $0.00 per M".to_string(),
            is_current: false,
        },
        ModelEntry {
            id: "zen/big-pickle".to_string(),
            display_name: "Big Pickle (Free, via Zen)".to_string(),
            description: "128K ctx | $0.00 per M".to_string(),
            is_current: false,
        },
        ModelEntry {
            id: "zen/ring-2.6-1t-free".to_string(),
            display_name: "Ring 2.6 1T (Free, via Zen)".to_string(),
            description: "128K ctx | $0.00 per M".to_string(),
            is_current: false,
        },
        ModelEntry {
            id: "zen/nemotron-3-super-free".to_string(),
            display_name: "Nemotron 3 Super (Free, via Zen)".to_string(),
            description: "128K ctx | $0.00 per M".to_string(),
            is_current: false,
        },
        ModelEntry {
            id: "openrouter/free".to_string(),
            display_name: "OpenRouter Free Router".to_string(),
            description: "200K ctx | random free model · $0.00 per M".to_string(),
            is_current: false,
        },
    ]
}

/// State for the /model picker overlay.
pub struct ModelPickerState {
    pub visible: bool,
    pub selected_idx: usize,
    pub models: Vec<ModelEntry>,
    pub title: String,
    /// Live filter typed by the user.
    pub filter: String,
    /// Current effort level for models that support extended thinking.
    pub effort_level: EffortLevel,
    /// Whether fast mode is currently active.
    pub fast_mode: bool,
    /// The currently locked fast-mode model, if fast mode is active.
    pub fast_mode_model: Option<String>,
    /// `true` once the dynamic model list has been loaded from the API.
    pub models_loaded: bool,
    /// `true` while the background fetch is in flight.
    pub loading_models: bool,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl ModelPickerState {
    /// Create a new picker with the default model list (not yet visible).
    pub fn new() -> Self {
        Self {
            visible: false,
            selected_idx: 0,
            models: Self::default_models(),
            title: "Select model".to_string(),
            filter: String::new(),
            effort_level: EffortLevel::Normal,
            fast_mode: false,
            fast_mode_model: None,
            models_loaded: false,
            loading_models: false,
        }
    }

    /// Open the overlay.
    ///
    /// `current_model` is highlighted as active; `current_effort` and
    /// `fast_mode` are carried over from app state so the user sees the live
    /// values.
    pub fn open(&mut self, current_model: &str) {
        self.open_with_state(current_model, EffortLevel::Normal, false);
    }

    /// Open the overlay with full state context.
    pub fn open_with_state(&mut self, current_model: &str, effort: EffortLevel, fast_mode: bool) {
        self.open_with_title("Select model", current_model, effort, fast_mode);
    }

    pub fn open_with_title(
        &mut self,
        title: impl Into<String>,
        current_model: &str,
        effort: EffortLevel,
        fast_mode: bool,
    ) {
        for m in &mut self.models {
            m.is_current = m.id == current_model;
        }
        self.selected_idx = self
            .models
            .iter()
            .position(|m| m.is_current)
            .unwrap_or(0);
        self.title = title.into();
        self.filter.clear();
        self.effort_level = effort;
        self.fast_mode = fast_mode;
        self.fast_mode_model = fast_mode.then_some(current_model.to_string());
        self.visible = true;
    }

    /// Close the overlay without selecting.
    pub fn close(&mut self) {
        self.visible = false;
        self.filter.clear();
    }

    pub fn is_selected_fast_mode_model(&self, model_id: &str) -> bool {
        self.fast_mode_model.as_deref() == Some(model_id)
    }

    /// Move selection up one row (wraps to last if at top).
    pub fn select_prev(&mut self) {
        let count = self.filtered_models().len();
        if count == 0 { return; }
        if self.selected_idx == 0 {
            self.selected_idx = count - 1;
        } else {
            self.selected_idx -= 1;
        }
    }

    /// Move selection down one row (wraps to first if at bottom).
    pub fn select_next(&mut self) {
        let count = self.filtered_models().len();
        if count == 0 { return; }
        self.selected_idx = (self.selected_idx + 1) % count;
    }

    pub fn select_first(&mut self) {
        self.selected_idx = 0;
    }

    pub fn select_last(&mut self) {
        let count = self.filtered_models().len();
        self.selected_idx = count.saturating_sub(1);
    }

    /// Cycle effort level forward (→ key).
    pub fn effort_next(&mut self) {
        let filtered = self.filtered_models();
        let id = filtered.get(self.selected_idx).map(|m| m.id.as_str()).unwrap_or("");
        let supports_max = model_supports_max_effort(id);
        self.effort_level = self.effort_level.next(supports_max);
    }

    /// Cycle effort level backward (← key).
    pub fn effort_prev(&mut self) {
        let filtered = self.filtered_models();
        let id = filtered.get(self.selected_idx).map(|m| m.id.as_str()).unwrap_or("");
        let supports_max = model_supports_max_effort(id);
        self.effort_level = self.effort_level.prev(supports_max);
    }

    /// Returns the effective effort for the currently highlighted model:
    /// `None` if the model does not support extended thinking.
    pub fn effective_effort(&self) -> Option<EffortLevel> {
        let filtered = self.filtered_models();
        let id = filtered.get(self.selected_idx).map(|m| m.id.as_str()).unwrap_or("");
        if model_supports_effort(id) {
            Some(self.effort_level)
        } else {
            None
        }
    }

    /// Confirm the current selection.
    ///
    /// Returns `(model_id, effort)` where `effort` is `None` for models that
    /// do not support extended thinking.  Closes the picker.
    ///
    /// Returns the selected model; the caller is responsible for persisting it
    /// in the correct provider-aware format.
    pub fn confirm(&mut self) -> Option<(String, Option<EffortLevel>)> {
        let filtered = self.filtered_models();
        let custom = self.filter.trim();
        if filtered.is_empty() {
            if custom.is_empty() {
                return None;
            }
            let id = custom.to_string();
            self.close();
            return Some((id, None));
        }
        let entry = filtered.get(self.selected_idx)?;
        let id = entry.id.clone();
        let effort = if model_supports_effort(&id) { Some(self.effort_level) } else { None };
        // If user chose a model other than the fast-mode model while fast mode is
        // active, the caller should turn off fast mode (mirrors TS behaviour).
        self.close();
        Some((id, effort))
    }

    /// Append a character to the filter string and reset the selection.
    pub fn push_filter_char(&mut self, c: char) {
        self.filter.push(c);
        self.selected_idx = 0;
    }

    /// Remove the last character from the filter string.
    pub fn pop_filter_char(&mut self) {
        self.filter.pop();
        self.selected_idx = 0;
    }

    /// Return models that match the current filter (case-insensitive).
    pub fn filtered_models(&self) -> Vec<&ModelEntry> {
        if self.filter.is_empty() {
            return self.models.iter().collect();
        }
        let needle = self.filter.to_lowercase();
        self.models
            .iter()
            .filter(|m| {
                m.id.to_lowercase().contains(needle.as_str())
                    || m.display_name.to_lowercase().contains(needle.as_str())
                    || m.description.to_lowercase().contains(needle.as_str())
            })
            .collect()
    }

    /// Replace the model list with dynamically loaded entries.
    ///
    /// Called by the app event loop when the background fetch completes.
    /// Resets `loading_models` and sets `models_loaded`.
    pub fn set_models(&mut self, entries: Vec<ModelEntry>) {
        self.models = entries;
        self.loading_models = false;
        self.models_loaded = true;
        // Keep selected_idx in bounds.
        let count = self.filtered_models().len();
        if count > 0 && self.selected_idx >= count {
            self.selected_idx = count - 1;
        }
    }

    /// Fetch the list of available models from the Anthropic API and convert
    /// them to `ModelEntry` values.
    ///
    /// On success, models are sorted newest-first (by `created_at` descending).
    /// On any error, returns `default_models()` as a fallback so the picker is
    /// never left empty.
    pub async fn fetch_models(client: &claurst_api::AnthropicClient) -> Vec<ModelEntry> {
        match client.fetch_available_models().await {
            Ok(available) => {
                if available.is_empty() {
                    return Self::default_models();
                }

                let mut entries: Vec<(i64, ModelEntry)> = available
                    .into_iter()
                    .map(|m| {
                        let display = m
                            .display_name
                            .clone()
                            .unwrap_or_else(|| m.id.clone());
                        let description = model_family_description(&m.id);
                        let ts = m.created_at.unwrap_or(0);
                        (ts, ModelEntry {
                            id: m.id,
                            display_name: display,
                            description,
                            is_current: false,
                        })
                    })
                    .collect();

                // Sort newest-first.
                entries.sort_by(|a, b| b.0.cmp(&a.0));
                entries.into_iter().map(|(_, e)| e).collect()
            }
            Err(_) => Self::default_models(),
        }
    }

    /// Hardcoded list of Claude models available as of 2025.
    pub fn default_models() -> Vec<ModelEntry> {
        vec![
            ModelEntry {
                id: "claude-opus-4-6".to_string(),
                display_name: "Claude Opus 4.6".to_string(),
                description: "Most capable model — best for complex reasoning and analysis".to_string(),
                is_current: false,
            },
            ModelEntry {
                id: "claude-sonnet-4-6".to_string(),
                display_name: "Claude Sonnet 4.6".to_string(),
                description: "Balanced performance and speed — great for coding tasks".to_string(),
                is_current: false,
            },
            ModelEntry {
                id: "claude-haiku-4-5-20251001".to_string(),
                display_name: "Claude Haiku 4.5 (2025-10-01)".to_string(),
                description: "Fast and efficient — ideal for quick completions".to_string(),
                is_current: false,
            },
            ModelEntry {
                id: "claude-opus-4-5".to_string(),
                display_name: "Claude Opus 4.5".to_string(),
                description: "Previous Opus generation — powerful multimodal reasoning".to_string(),
                is_current: false,
            },
            ModelEntry {
                id: "claude-sonnet-4-5".to_string(),
                display_name: "Claude Sonnet 4.5".to_string(),
                description: "Previous Sonnet generation — solid coding and writing".to_string(),
                is_current: false,
            },
            ModelEntry {
                id: "claude-haiku-4-5".to_string(),
                display_name: "Claude Haiku 4.5".to_string(),
                description: "Previous Haiku generation — lightweight and responsive".to_string(),
                is_current: false,
            },
            ModelEntry {
                id: "claude-3-7-sonnet-20250219".to_string(),
                display_name: "Claude 3.7 Sonnet (2025-02-19)".to_string(),
                description: "Sonnet 3.7 with enhanced instruction following".to_string(),
                is_current: false,
            },
            ModelEntry {
                id: "claude-3-5-sonnet-20241022".to_string(),
                display_name: "Claude 3.5 Sonnet (2024-10-22)".to_string(),
                description: "Highly capable 3.5 Sonnet — reliable and well-tested".to_string(),
                is_current: false,
            },
            ModelEntry {
                id: "claude-3-5-haiku-20241022".to_string(),
                display_name: "Claude 3.5 Haiku (2024-10-22)".to_string(),
                description: "Fast 3.5 Haiku — great for high-throughput pipelines".to_string(),
                is_current: false,
            },
        ]
    }
}

impl Default for ModelPickerState {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the model picker overlay directly into `buf`.
///
/// Draws a centred modal (≈70 wide × ≈22 tall) with:
/// - Fast-mode notice when fast mode is active
/// - A filter line when the user is typing
/// - A scrollable list of models with effort indicator for supporting models
/// - Selection highlight on the focused row
/// - Bottom hint bar with ←/→ keys for effort adjustment
pub fn render_model_picker(state: &ModelPickerState, area: Rect, buf: &mut Buffer) {
    if !state.visible {
        return;
    }

    use ratatui::prelude::Stylize;
    use ratatui::widgets::Widget;

    let _pink = Color::Rgb(233, 30, 99);
    let dim = Color::Rgb(90, 90, 90);
    let dialog_bg = CLAURST_PANEL_BG;
    let highlight_bg = Color::Rgb(233, 30, 99);
    let highlight_fg = Color::White;

    // ── Dark overlay ──
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(Color::Rgb(10, 10, 14));
                cell.set_fg(Color::Rgb(40, 40, 45));
            }
        }
    }

    // ── Dialog size ──
    let width = 65u16.min(area.width.saturating_sub(6));
    let max_height = (area.height as f32 * 0.75) as u16;
    let filtered = state.filtered_models();
    let content_h = (filtered.len() as u16 + 6).min(max_height).max(8);
    let dialog_area = centered_rect(width, content_h, area);

    // ── Fill dialog bg (no border) ──
    for y in dialog_area.y..dialog_area.y + dialog_area.height {
        for x in dialog_area.x..dialog_area.x + dialog_area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(' ');
                cell.set_bg(dialog_bg);
                cell.set_fg(Color::White);
            }
        }
    }

    let inner = Rect {
        x: dialog_area.x + 1,
        y: dialog_area.y + 1,
        width: dialog_area.width.saturating_sub(2),
        height: dialog_area.height.saturating_sub(2),
    };

    let footer_height = 1u16.min(inner.height);
    let header_height = 3u16.min(inner.height.saturating_sub(footer_height));
    let header_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: header_height,
    };
    let body_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(header_height),
        width: inner.width,
        height: inner.height.saturating_sub(header_height + footer_height),
    };
    let footer_area = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(footer_height),
        width: inner.width,
        height: footer_height,
    };

    // ── Fixed header ──
    let mut header_lines: Vec<Line> = Vec::new();

    // Title row: "Select model" left, "esc" right
    let title_pad = inner.width.saturating_sub(state.title.len() as u16 + 5) as usize;
    header_lines.push(Line::from(vec![
        Span::styled(format!(" {}", state.title), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:>w$}", "esc ", w = title_pad), Style::default().fg(dim)),
    ]));

    // Search field
    header_lines.push(Line::from(""));
    header_lines.push(modal_search_line(&state.filter, "Search", dim, Color::White));

    let header_para = Paragraph::new(header_lines).bg(dialog_bg);
    header_para.render(header_area, buf);

    if body_area.height == 0 {
        return;
    }

    // ── Model items ──
    let mut lines: Vec<Line> = Vec::new();
    let mut selected_line_idx: u16 = 0;

    if state.fast_mode {
        lines.push(Line::from(vec![Span::styled(
            format!(
                " \u{26a1} Fast mode ON ({})",
                state.fast_mode_model.as_deref().unwrap_or("current model")
            ),
            Style::default().fg(Color::Yellow),
        )]));
    }

    if state.loading_models {
        lines.push(Line::from(vec![Span::styled(
            " Loading models\u{2026}",
            Style::default().fg(dim),
        )]));
    }

    if !lines.is_empty() {
        lines.push(Line::from(""));
    }

    if filtered.is_empty() {
        lines.push(Line::from(vec![Span::styled(" No results found", Style::default().fg(dim))]));
        if !state.filter.trim().is_empty() {
            lines.push(Line::from(vec![Span::styled(
                " Press Enter to use custom model",
                Style::default().fg(Color::Rgb(200, 200, 200)),
            )]));
        }
    } else {
        for (i, model) in filtered.iter().enumerate() {
            let is_selected = i == state.selected_idx;
            let supports_effort = model_supports_effort(&model.id);

            if is_selected {
                selected_line_idx = lines.len() as u16;
            }

            let (fg, bg) = if is_selected {
                (highlight_fg, highlight_bg)
            } else {
                (Color::White, dialog_bg)
            };

            let mut spans: Vec<Span<'static>> = Vec::new();

            // Current model indicator
            if model.is_current {
                spans.push(Span::styled(" \u{25cf} ", Style::default().fg(Color::Green).bg(bg)));
            } else {
                spans.push(Span::styled("   ", Style::default().bg(bg)));
            }

            spans.push(Span::styled(model.display_name.clone(), Style::default().fg(fg).bg(bg)));

            // Effort indicator
            if supports_effort && is_selected {
                spans.push(Span::styled(
                    format!("  {} {}", state.effort_level.symbol(), state.effort_level.label()),
                    Style::default().fg(Color::Rgb(200, 255, 200)).bg(bg),
                ));
            }

            // Description
            if !model.description.is_empty() {
                let desc_fg = if is_selected { Color::Rgb(200, 200, 200) } else { dim };
                spans.push(Span::styled(
                    format!("  {}", model.description),
                    Style::default().fg(desc_fg).bg(bg),
                ));
            }

            // Pad for full-width highlight
            if is_selected {
                let text_len: usize = spans.iter().map(|s| s.content.len()).sum();
                let pad = inner.width.saturating_sub(text_len as u16) as usize;
                if pad > 0 {
                    spans.push(Span::styled(" ".repeat(pad), Style::default().bg(highlight_bg)));
                }
            }

            lines.push(Line::from(spans));
        }
    }

    // ── Scroll ──
    let total_lines = lines.len() as u16;
    let visible = body_area.height;
    let scroll_y = if total_lines <= visible {
        0u16
    } else if selected_line_idx + 3 >= visible {
        (selected_line_idx + 3).saturating_sub(visible)
    } else {
        0
    };

    let para = Paragraph::new(lines).bg(dialog_bg).scroll((scroll_y, 0));

    para.render(body_area, buf);

    let mut footer_spans = vec![
        Span::styled(" enter", Style::default().fg(dim)),
        Span::styled(" select", Style::default().fg(dim)),
    ];
    if let Some(model) = filtered.get(state.selected_idx) {
        if model_supports_effort(&model.id) {
            footer_spans.push(Span::raw("  "));
            footer_spans.push(Span::styled("\u{2190}/\u{2192}", Style::default().fg(dim)));
            footer_spans.push(Span::styled(" effort", Style::default().fg(dim)));
        }
    }
    footer_spans.push(Span::raw("  "));
    footer_spans.push(Span::styled(" /connect", Style::default().fg(Color::Rgb(233, 30, 99))));
    footer_spans.push(Span::styled(" providers", Style::default().fg(dim)));
    Paragraph::new(Line::from(footer_spans)).bg(dialog_bg).render(footer_area, buf);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_picker_with_current(current: &str) -> ModelPickerState {
        let mut p = ModelPickerState::new();
        p.open(current);
        p
    }

    // 1. Default model list is non-empty and contains expected IDs.
    #[test]
    fn default_models_are_populated() {
        let models = ModelPickerState::default_models();
        assert!(!models.is_empty(), "default model list must not be empty");
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"claude-sonnet-4-6"));
        assert!(ids.contains(&"claude-opus-4-6"));
        assert!(ids.contains(&"claude-3-5-haiku-20241022"));
    }

    // 2. open() marks exactly one model as current.
    #[test]
    fn open_marks_current_model() {
        let mut p = ModelPickerState::new();
        p.open("claude-sonnet-4-6");
        let current_count = p.models.iter().filter(|m| m.is_current).count();
        assert_eq!(current_count, 1);
        assert!(p.models.iter().find(|m| m.id == "claude-sonnet-4-6").unwrap().is_current);
    }

    #[test]
    fn open_with_title_updates_dialog_title() {
        let mut p = ModelPickerState::new();
        p.open_with_title("Anthropic", "claude-sonnet-4-6", EffortLevel::Normal, false);
        assert_eq!(p.title, "Anthropic");
    }

    #[test]
    fn open_with_fast_mode_tracks_locked_model() {
        let mut p = ModelPickerState::new();
        p.open_with_state("gpt-4o-mini", EffortLevel::Normal, true);
        assert_eq!(p.fast_mode_model.as_deref(), Some("gpt-4o-mini"));
        assert!(p.is_selected_fast_mode_model("gpt-4o-mini"));
        assert!(!p.is_selected_fast_mode_model("gpt-4o"));
    }

    // 3. open() with an unknown model ID marks none as current and sets idx=0.
    #[test]
    fn open_unknown_model_selects_first() {
        let mut p = ModelPickerState::new();
        p.open("unknown-model");
        assert_eq!(p.selected_idx, 0);
        assert!(p.models.iter().all(|m| !m.is_current));
    }

    // 4. select_next() wraps around to 0 after the last entry.
    #[test]
    fn select_next_wraps() {
        let mut p = make_picker_with_current("claude-opus-4-6");
        let total = p.filtered_models().len();
        p.selected_idx = total - 1;
        p.select_next();
        assert_eq!(p.selected_idx, 0);
    }

    // 5. select_prev() wraps around to last after idx 0.
    #[test]
    fn select_prev_wraps() {
        let mut p = make_picker_with_current("claude-opus-4-6");
        p.selected_idx = 0;
        p.select_prev();
        let total = p.filtered_models().len();
        assert_eq!(p.selected_idx, total - 1);
    }

    // 6. filter reduces visible entries.
    #[test]
    fn filter_reduces_results() {
        let mut p = make_picker_with_current("claude-opus-4-6");
        for c in "sonnet".chars() { p.push_filter_char(c); }
        let all = p.models.len();
        let filtered = p.filtered_models();
        assert!(filtered.len() < all, "filter should reduce the result count");
        assert!(!filtered.is_empty(), "at least one sonnet model must match");
        for m in &filtered {
            let haystack = format!("{} {} {}", m.id, m.display_name, m.description).to_lowercase();
            assert!(haystack.contains("sonnet"), "model '{}' does not match filter", m.id);
        }
    }

    // 7. pop_filter_char removes last char.
    #[test]
    fn pop_filter_char_removes_last() {
        let mut p = make_picker_with_current("claude-opus-4-6");
        p.push_filter_char('h'); p.push_filter_char('a'); p.push_filter_char('i');
        assert_eq!(p.filter, "hai");
        p.pop_filter_char();
        assert_eq!(p.filter, "ha");
    }

    // 8. confirm() returns selected model ID and closes the picker.
    #[test]
    fn confirm_returns_id_and_closes() {
        let mut p = make_picker_with_current("claude-opus-4-6");
        p.selected_idx = 0;
        let first_id = p.filtered_models()[0].id.clone();
        let result = p.confirm();
        assert_eq!(result.map(|(id, _)| id), Some(first_id));
        assert!(!p.visible, "picker should be closed after confirm");
    }

    // 9. confirm() on empty filter list uses custom model when filter is set.
    #[test]
    fn confirm_empty_filter_returns_none() {
        let mut p = make_picker_with_current("claude-opus-4-6");
        p.filter = "zzznomatch999".to_string();
        p.selected_idx = 0;
        let result = p.confirm();
        assert_eq!(result.map(|(id, _)| id), Some("zzznomatch999".to_string()));
    }

    // 10. close() clears filter and hides overlay.
    #[test]
    fn close_clears_state() {
        let mut p = make_picker_with_current("claude-opus-4-6");
        p.push_filter_char('x');
        p.close();
        assert!(!p.visible);
        assert!(p.filter.is_empty());
    }

    // 11. effort cycling works for effort-supporting models.
    #[test]
    fn effort_cycles_correctly() {
        let mut p = make_picker_with_current("claude-sonnet-4-6");
        // sonnet-4-6 supports effort but not max
        assert_eq!(p.effort_level, EffortLevel::Normal);
        p.effort_next();
        assert_eq!(p.effort_level, EffortLevel::High);
        p.effort_next();
        // no max for sonnet → wraps to Low
        assert_eq!(p.effort_level, EffortLevel::Low);
    }

    // 12. Opus supports max effort.
    #[test]
    fn opus_supports_max_effort() {
        assert!(model_supports_max_effort("claude-opus-4-6"));
        assert!(!model_supports_max_effort("claude-sonnet-4-6"));
        assert!(!model_supports_max_effort("claude-haiku-4-5"));
    }

    // 13. Non-effort models return None from effective_effort.
    #[test]
    fn haiku_has_no_effort() {
        let mut p = make_picker_with_current("claude-haiku-4-5");
        p.selected_idx = p.models.iter().position(|m| m.id == "claude-haiku-4-5").unwrap();
        assert!(!model_supports_effort("claude-haiku-4-5"));
        let effort = p.confirm();
        assert!(effort.is_some_and(|(_, e)| e.is_none()));
    }

    // 14. render_model_picker does not panic for a default-area call.
    #[test]
    fn render_does_not_panic() {
        let mut p = ModelPickerState::new();
        p.open("claude-sonnet-4-6");
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        render_model_picker(&p, area, &mut buf);
    }

    // 15. render does nothing when not visible.
    #[test]
    fn render_noop_when_hidden() {
        let p = ModelPickerState::new();
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        render_model_picker(&p, area, &mut buf);
        for cell in buf.content() {
            assert_eq!(cell.symbol(), " ", "buffer should be empty when picker is hidden");
        }
    }

    // 16. models_for_provider_from_registry returns the bundled snapshot's
    //     entries for each well-known provider.  Specific model IDs aren't
    //     asserted here because the snapshot is regenerated periodically;
    //     instead we check the family / provider-namespace shape.
    #[test]
    fn models_for_provider_anthropic() {
        let registry = claurst_api::ModelRegistry::new();
        let models = models_for_provider_from_registry("anthropic", &registry);
        assert!(!models.is_empty(), "anthropic must yield models");
        assert!(
            models.iter().any(|m| m.id.starts_with("claude")),
            "anthropic should expose at least one claude-* model"
        );
    }

    #[test]
    fn models_for_provider_openai() {
        let registry = claurst_api::ModelRegistry::new();
        let models = models_for_provider_from_registry("openai", &registry);
        assert!(!models.is_empty());
        // Must NOT contain Claude models
        assert!(!models.iter().any(|m| m.id.contains("claude")));
        // Should contain at least one gpt-* or o-series id
        assert!(
            models.iter().any(|m| m.id.starts_with("gpt-") || m.id.starts_with("o3") || m.id.starts_with("o4")),
            "openai should expose at least one gpt/o-series model"
        );
    }

    #[test]
    fn models_for_provider_unknown_returns_default() {
        let registry = claurst_api::ModelRegistry::new();
        let models = models_for_provider_from_registry("some-unknown-provider", &registry);
        assert!(!models.is_empty());
        assert_eq!(models[0].id, "default");
    }

    // 17. default_model_for_provider returns prefixed models for non-anthropic.
    #[test]
    fn default_model_for_provider_openai() {
        let registry = claurst_api::ModelRegistry::new();
        let m = default_model_for_provider("openai", &registry);
        assert!(m.starts_with("openai/"), "openai default must be prefixed: {m}");
    }

    #[test]
    fn default_model_for_provider_anthropic_bare() {
        // Anthropic models are bare (no prefix) for backwards compat.
        let registry = claurst_api::ModelRegistry::new();
        let m = default_model_for_provider("anthropic", &registry);
        assert!(!m.contains('/'), "anthropic default must be bare: {m}");
        assert!(m.starts_with("claude"), "anthropic default must be a claude variant: {m}");
    }

    #[test]
    fn default_model_for_provider_unknown_falls_back() {
        let registry = claurst_api::ModelRegistry::new();
        assert_eq!(
            default_model_for_provider("some-self-hosted-thing", &registry),
            "some-self-hosted-thing/default"
        );
    }

    // 18. set_models replaces the model list.
    #[test]
    fn set_models_replaces_list() {
        let registry = claurst_api::ModelRegistry::new();
        let mut p = ModelPickerState::new();
        let openai_models = models_for_provider_from_registry("openai", &registry);
        p.set_models(openai_models);
        let ids: Vec<&str> = p.models.iter().map(|m| m.id.as_str()).collect();
        assert!(!ids.iter().any(|id| id.contains("claude")));
    }
}
