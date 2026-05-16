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
    format!("{} tokens · {}", format_tokens(tokens), format_cost_usd(cost_cents))
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
        // For timestamps older than 2 days, return a calendar date ("Mar 15").
        // Convert the timestamp to a date via UNIX epoch arithmetic.
        // Days since epoch → approximate month/day without a date library.
        let ts_secs = ts_ms / 1000;
        let days_since_epoch = ts_secs / 86400;
        // Gregorian calendar computation (handles leap years correctly).
        let (month, day) = days_to_month_day(days_since_epoch);
        let month_names = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun",
            "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        format!("{} {}", month_names[(month as usize).saturating_sub(1).min(11)], day)
    }
}

/// Convert a count of days since the UNIX epoch (1970-01-01) to a
/// `(month, day)` pair using the proleptic Gregorian calendar.
fn days_to_month_day(days: u64) -> (u32, u32) {
    // Algorithm: civil calendar from Howard Hinnant's date algorithms.
    let z = days as i64 + 719468; // shift epoch to 0000-03-01
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // year of era [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // month of year [0, 11] (March=0)
    let day = doy - (153 * mp + 2) / 5 + 1; // day [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (month as u32, day as u32)
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

    #[test]
    fn days_to_month_day_known_dates() {
        // 2024-03-15 → days since epoch = (2024-1970)*365 + leap_days + 74
        // Verify a known date: 2000-01-01 is day 10957 since epoch.
        assert_eq!(days_to_month_day(10957), (1, 1));
        // 2000-03-01 is day 11017 since epoch.
        assert_eq!(days_to_month_day(11017), (3, 1));
        // 1970-01-01 is day 0.
        assert_eq!(days_to_month_day(0), (1, 1));
    }

    #[test]
    fn format_relative_time_old_timestamp_returns_calendar_date() {
        // A timestamp 30 days ago should return a "Mon DD" string, not "X days ago".
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let thirty_days_ago = now_ms.saturating_sub(30 * 86400 * 1000);
        let result = format_relative_time(thirty_days_ago);
        // Must NOT contain "days ago".
        assert!(
            !result.contains("days ago"),
            "expected calendar date, got: {result}"
        );
        // Must look like "Mmm DD" (e.g. "Mar 5" or "Feb 15").
        let month_names = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun",
            "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let has_month = month_names.iter().any(|m| result.starts_with(m));
        assert!(has_month, "expected month prefix in: {result}");
    }
}
