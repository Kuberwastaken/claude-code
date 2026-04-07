//! Figure/icon constants matching src/constants/figures.ts

// Platform-aware: on Windows use ● (U+25CF), elsewhere ⏺ (U+23FA)
pub fn black_circle() -> &'static str {
    if cfg!(target_os = "windows") {
        "●"
    } else {
        "⏺"
    }
}
pub const BULLET_OPERATOR: &str = "∙"; // U+2219
pub const TEARDROP_ASTERISK: &str = "✻"; // U+273B - used for thinking/compact
pub const UP_ARROW: &str = "↑"; // U+2191
pub const DOWN_ARROW: &str = "↓"; // U+2193
pub const LIGHTNING_BOLT: &str = "↯"; // U+21AF - fast mode
pub const EFFORT_LOW: &str = "○"; // U+25CB
pub const EFFORT_MEDIUM: &str = "◐"; // U+25D0
pub const EFFORT_HIGH: &str = "●"; // U+25CF
pub const EFFORT_MAX: &str = "◉"; // U+25C9
pub const PLAY_ICON: &str = "▶"; // U+25B6
pub const PAUSE_ICON: &str = "⏸"; // U+23F8
pub const REFRESH_ARROW: &str = "↻"; // U+21BB
pub const FORK_GLYPH: &str = "⑂"; // U+2442
pub const DIAMOND_OPEN: &str = "◇"; // U+25C7 - running/pending review
pub const DIAMOND_FILLED: &str = "◆"; // U+25C6 - completed review
pub const REFERENCE_MARK: &str = "※"; // U+203B - away summary marker
pub const FLAG_ICON: &str = "⚑"; // U+2691
pub const BLOCKQUOTE_BAR: &str = "▎"; // U+258E - blockquote left bar
pub const HEAVY_HORIZONTAL: &str = "━"; // U+2501
pub const THEREFORE: &str = "∴"; // U+2234 - alternative thinking symbol
pub const NEW_MESSAGES_DOWN: &str = "↓";
pub const BRIDGE_READY: &str = "· ✔ ·";
pub const BRIDGE_FAILED: &str = "×";
