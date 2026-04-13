//! Formatting utilities for cost, duration, and token counts.
//! Mirrors src/utils/formatters.ts and related TS helpers.

/// Format a cost in USD cents as a human-readable string.
/// 0 → "$0.00", 150 → "$1.50", 0.5 → "$0.01"
pub fn format_cost_usd(cents: f64) -> String {
    if cents < 0.01 {
        "<$0.01".to_string()
    } else {
        format!("${:.2}", cents / 100.0)
    }
}

/// Format a duration in milliseconds as a human-readable string.
/// < 1000ms → "Xms", < 60s → "Xs", < 60m → "Xm Ys", else "Xh Ym"
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else if ms < 3_600_000 {
        let minutes = ms / 60_000;
        let seconds = (ms % 60_000) / 1_000;
        format!("{}m {}s", minutes, seconds)
    } else {
        let hours = ms / 3_600_000;
        let minutes = (ms % 3_600_000) / 60_000;
        format!("{}h {}m", hours, minutes)
    }
}

/// Format a token count compactly: 1234 → "1.2K", 1234567 → "1.2M"
pub fn format_tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 10_000 {
        format!("{:.0}K", count as f64 / 1_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// Format a token/cost summary line for the status bar.
/// Example: "3.2K tokens · $0.04"
pub fn format_usage_summary(tokens: u64, cost_cents: f64) -> String {
    format!(
        "{} tokens · {}",
        format_tokens(tokens),
        format_cost_usd(cost_cents)
    )
}

/// Format a relative time string (for session listings).
/// "just now", "2 minutes ago", "3 hours ago", "yesterday", "Mar 15"
pub fn format_relative_time(ts_ms: u64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let diff_ms = now_ms.saturating_sub(ts_ms);
    let diff_secs = diff_ms / 1000;

    if diff_secs < 60 {
        "just now".to_string()
    } else if diff_secs < 3600 {
        let m = diff_secs / 60;
        format!("{} minute{} ago", m, if m == 1 { "" } else { "s" })
    } else if diff_secs < 86400 {
        let h = diff_secs / 3600;
        format!("{} hour{} ago", h, if h == 1 { "" } else { "s" })
    } else if diff_secs < 172800 {
        "yesterday".to_string()
    } else {
        let days = diff_secs / 86400;
        format!("{} days ago", days)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_cost() {
        assert_eq!(format_cost_usd(0.0), "<$0.01");
        assert_eq!(format_cost_usd(150.0), "$1.50");
        assert_eq!(format_cost_usd(2.0), "$0.02");
    }

    #[test]
    fn format_duration() {
        assert_eq!(format_duration_ms(500), "500ms");
        assert_eq!(format_duration_ms(5000), "5.0s");
        assert_eq!(format_duration_ms(90_000), "1m 30s");
    }

    #[test]
    fn format_tokens_cases() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1500), "1.5K");
        assert_eq!(format_tokens(50_000), "50K");
    }
}
