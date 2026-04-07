//! Settings migration framework
//! Runs on startup to upgrade settings.json from older versions.
//!
//! Migrations are derived from the TypeScript originals:
//!   - src/migrations/migrateFennecToOpus.ts
//!   - src/migrations/migrateLegacyOpusToCurrent.ts
//!   - src/migrations/migrateSonnet45ToSonnet46.ts
//!   - src/migrations/migrateAutoUpdatesToSettings.ts
//!   - (and several others without separate TS source files)
//!
//! Each migration is idempotent: it only touches fields it recognises and
//! only writes when it actually changes something.

use serde_json::Value;

/// A single migration function.
/// Returns `true` if the settings object was modified.
pub type MigrationFn = fn(&mut Value) -> bool;

/// All migrations in the order they must be applied.
pub const MIGRATIONS: &[(&str, MigrationFn)] = &[
    ("migrate_fennec_to_opus", migrate_fennec_to_opus),
    (
        "migrate_legacy_opus_to_current",
        migrate_legacy_opus_to_current,
    ),
    ("migrate_opus_to_opus_1m", migrate_opus_to_opus_1m),
    (
        "migrate_sonnet_1m_to_sonnet_45",
        migrate_sonnet_1m_to_sonnet_45,
    ),
    (
        "migrate_sonnet_45_to_sonnet_46",
        migrate_sonnet_45_to_sonnet_46,
    ),
    (
        "migrate_bypass_permissions_to_settings",
        migrate_bypass_permissions_to_settings,
    ),
    (
        "migrate_repl_bridge_to_remote_control",
        migrate_repl_bridge_to_remote_control,
    ),
    (
        "migrate_enable_all_mcp_servers",
        migrate_enable_all_mcp_servers,
    ),
    ("migrate_auto_updates", migrate_auto_updates),
    ("reset_auto_mode_opt_in", reset_auto_mode_opt_in),
    ("reset_pro_to_opus_default", reset_pro_to_opus_default),
];

/// Apply every pending migration to a settings `Value` (must be a JSON object).
/// Returns `true` when at least one migration changed the settings.
pub fn run_migrations(settings: &mut Value) -> bool {
    let mut changed = false;
    for (name, migration) in MIGRATIONS {
        if migration(settings) {
            tracing::info!("Applied settings migration: {}", name);
            changed = true;
        }
    }
    changed
}

// ---------------------------------------------------------------------------
// Model-name migrations
// ---------------------------------------------------------------------------

/// Fennec was an internal alias; map to the current Opus line.
/// Source: migrateFennecToOpus.ts
fn migrate_fennec_to_opus(settings: &mut Value) -> bool {
    // fennec-latest[1m] → opus[1m], fennec-latest → opus
    // fennec-fast-latest / opus-4-5-fast → opus[1m]  (fast-mode alias)
    let model = match settings.get("model").and_then(|v: &Value| v.as_str()) {
        Some(m) => m.to_string(),
        None => return false,
    };

    if model.starts_with("fennec-latest[1m]") {
        settings["model"] = Value::String("opus[1m]".to_string());
        return true;
    }
    if model.starts_with("fennec-latest") {
        settings["model"] = Value::String("opus".to_string());
        return true;
    }
    if model.starts_with("fennec-fast-latest") || model.starts_with("opus-4-5-fast") {
        settings["model"] = Value::String("opus[1m]".to_string());
        settings["fastMode"] = Value::Bool(true);
        return true;
    }
    false
}

/// Migrate explicit Opus 4.0/4.1 strings to the `opus` alias.
/// Source: migrateLegacyOpusToCurrent.ts
fn migrate_legacy_opus_to_current(settings: &mut Value) -> bool {
    const LEGACY_OPUS: &[&str] = &[
        "claude-opus-4-20250514",
        "claude-opus-4-1-20250805",
        "claude-opus-4-0",
        "claude-opus-4-1",
    ];
    let model = match settings.get("model").and_then(|v: &Value| v.as_str()) {
        Some(m) => m.to_string(),
        None => return false,
    };
    if LEGACY_OPUS.contains(&model.as_str()) {
        settings["model"] = Value::String("opus".to_string());
        return true;
    }
    false
}

/// Rename the old explicit `claude-opus-4-0` model string (pre-alias era).
fn migrate_opus_to_opus_1m(settings: &mut Value) -> bool {
    rename_model(settings, "claude-opus-4-0", "claude-opus-4-5-20251001")
}

/// Migrate the old Sonnet 1m string to the Sonnet 4.5 release ID.
fn migrate_sonnet_1m_to_sonnet_45(settings: &mut Value) -> bool {
    rename_model(
        settings,
        "claude-sonnet-4-0-1m",
        "claude-sonnet-4-5-20251015",
    )
}

/// Migrate Sonnet 4.5 explicit IDs to `sonnet` (which resolves to 4.6).
/// Source: migrateSonnet45ToSonnet46.ts
fn migrate_sonnet_45_to_sonnet_46(settings: &mut Value) -> bool {
    const SONNET_45_IDS: &[&str] = &[
        "claude-sonnet-4-5-20250929",
        "claude-sonnet-4-5-20250929[1m]",
        "sonnet-4-5-20250929",
        "sonnet-4-5-20250929[1m]",
        // Also handle the model strings used in the older Rust migrations table:
        "claude-sonnet-4-5-20251015",
        "claude-sonnet-4-5",
    ];

    let model = match settings.get("model").and_then(|v: &Value| v.as_str()) {
        Some(m) => m.to_string(),
        None => return false,
    };

    if SONNET_45_IDS.contains(&model.as_str()) {
        let has_1m = model.ends_with("[1m]");
        let new_model = if has_1m { "sonnet[1m]" } else { "sonnet" };
        settings["model"] = Value::String(new_model.to_string());
        return true;
    }
    false
}

/// Rename `from` to `to` in the `model`, `defaultModel`, and `mainLoopModel`
/// fields.  Returns `true` if any field was changed.
fn rename_model(settings: &mut Value, from: &str, to: &str) -> bool {
    let mut changed = false;
    for key in &["model", "defaultModel", "mainLoopModel"] {
        if let Some(val) = settings.get_mut(*key) {
            if val.as_str() == Some(from) {
                *val = Value::String(to.to_string());
                changed = true;
            }
        }
    }
    changed
}

// ---------------------------------------------------------------------------
// Config-structure migrations
// ---------------------------------------------------------------------------

/// Move `bypassPermissionsAccepted` boolean into `permissionMode`.
fn migrate_bypass_permissions_to_settings(settings: &mut Value) -> bool {
    let old = match settings.get("bypassPermissionsAccepted").cloned() {
        Some(v) => v,
        None => return false,
    };

    if settings.get("permissionMode").is_none() && old.as_bool().unwrap_or(false) {
        settings["permissionMode"] = Value::String("bypass".to_string());
    }

    if let Some(obj) = settings.as_object_mut() {
        obj.remove("bypassPermissionsAccepted");
    }
    true
}

/// Rename `replBridgeEnabled` → `remoteControlAtStartup`.
fn migrate_repl_bridge_to_remote_control(settings: &mut Value) -> bool {
    let old = match settings.get("replBridgeEnabled").cloned() {
        Some(v) => v,
        None => return false,
    };

    if settings.get("remoteControlAtStartup").is_none() {
        settings["remoteControlAtStartup"] = old;
    }

    if let Some(obj) = settings.as_object_mut() {
        obj.remove("replBridgeEnabled");
    }
    true
}

/// Rename `enableAllProjectMcpServers` → `mcpAutoApprove`.
fn migrate_enable_all_mcp_servers(settings: &mut Value) -> bool {
    let old = match settings.get("enableAllProjectMcpServers").cloned() {
        Some(v) => v,
        None => return false,
    };

    if settings.get("mcpAutoApprove").is_none() {
        settings["mcpAutoApprove"] = old;
    }

    if let Some(obj) = settings.as_object_mut() {
        obj.remove("enableAllProjectMcpServers");
    }
    true
}

/// Migrate `autoUpdatesEnabled` → `autoUpdates`.
/// Source: migrateAutoUpdatesToSettings.ts
/// The TS version also writes an env-var to settings.json; here we keep the
/// simpler structural rename and leave env-var injection to the caller.
fn migrate_auto_updates(settings: &mut Value) -> bool {
    let old = match settings.get("autoUpdatesEnabled").cloned() {
        Some(v) => v,
        None => return false,
    };

    if settings.get("autoUpdates").is_none() {
        settings["autoUpdates"] = old;
    }

    if let Some(obj) = settings.as_object_mut() {
        obj.remove("autoUpdatesEnabled");
    }
    true
}

/// Clear an old sentinel value for the auto-mode opt-in flag.
fn reset_auto_mode_opt_in(settings: &mut Value) -> bool {
    if let Some(val) = settings.get("autoModeOptIn") {
        if val.as_str() == Some("default_offer_2024") {
            settings["autoModeOptIn"] = Value::Null;
            return true;
        }
    }
    false
}

/// Reset users who were auto-defaulted to Opus back to Sonnet 4.6.
/// Only resets when `modelSetByUser` is not explicitly `true`.
fn reset_pro_to_opus_default(settings: &mut Value) -> bool {
    if let Some(val) = settings.get("model") {
        if val.as_str() == Some("claude-opus-4-5-20251001") {
            let set_by_user = settings
                .get("modelSetByUser")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !set_by_user {
                settings["model"] = Value::String("claude-sonnet-4-6".to_string());
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings(model: &str) -> Value {
        json!({ "model": model })
    }

    // ---- rename_model -------------------------------------------------------

    #[test]
    fn rename_model_changes_matching_field() {
        let mut s = settings("old-model");
        assert!(rename_model(&mut s, "old-model", "new-model"));
        assert_eq!(s["model"].as_str(), Some("new-model"));
    }

    #[test]
    fn rename_model_no_change_when_different() {
        let mut s = settings("something-else");
        assert!(!rename_model(&mut s, "old-model", "new-model"));
        assert_eq!(s["model"].as_str(), Some("something-else"));
    }

    #[test]
    fn rename_model_covers_all_keys() {
        let mut s = json!({
            "model": "claude-foo",
            "defaultModel": "claude-foo",
            "mainLoopModel": "claude-foo",
        });
        assert!(rename_model(&mut s, "claude-foo", "claude-bar"));
        assert_eq!(s["model"].as_str(), Some("claude-bar"));
        assert_eq!(s["defaultModel"].as_str(), Some("claude-bar"));
        assert_eq!(s["mainLoopModel"].as_str(), Some("claude-bar"));
    }

    // ---- migrate_fennec_to_opus ---------------------------------------------

    #[test]
    fn fennec_latest_1m_maps_to_opus_1m() {
        let mut s = settings("fennec-latest[1m]");
        assert!(migrate_fennec_to_opus(&mut s));
        assert_eq!(s["model"].as_str(), Some("opus[1m]"));
    }

    #[test]
    fn fennec_latest_maps_to_opus() {
        let mut s = settings("fennec-latest");
        assert!(migrate_fennec_to_opus(&mut s));
        assert_eq!(s["model"].as_str(), Some("opus"));
    }

    #[test]
    fn fennec_fast_maps_to_opus_1m_with_fast_mode() {
        let mut s = settings("fennec-fast-latest");
        assert!(migrate_fennec_to_opus(&mut s));
        assert_eq!(s["model"].as_str(), Some("opus[1m]"));
        assert_eq!(s["fastMode"].as_bool(), Some(true));
    }

    #[test]
    fn opus_4_5_fast_maps_to_opus_1m_with_fast_mode() {
        let mut s = settings("opus-4-5-fast");
        assert!(migrate_fennec_to_opus(&mut s));
        assert_eq!(s["model"].as_str(), Some("opus[1m]"));
        assert_eq!(s["fastMode"].as_bool(), Some(true));
    }

    #[test]
    fn fennec_no_match_returns_false() {
        let mut s = settings("claude-sonnet-4-6");
        assert!(!migrate_fennec_to_opus(&mut s));
    }

    // ---- migrate_legacy_opus_to_current ------------------------------------

    #[test]
    fn legacy_opus_4_0_maps_to_opus() {
        let mut s = settings("claude-opus-4-0");
        assert!(migrate_legacy_opus_to_current(&mut s));
        assert_eq!(s["model"].as_str(), Some("opus"));
    }

    #[test]
    fn legacy_opus_4_1_maps_to_opus() {
        let mut s = settings("claude-opus-4-1-20250805");
        assert!(migrate_legacy_opus_to_current(&mut s));
        assert_eq!(s["model"].as_str(), Some("opus"));
    }

    // ---- migrate_sonnet_45_to_sonnet_46 ------------------------------------

    #[test]
    fn sonnet_45_explicit_id_maps_to_sonnet() {
        let mut s = settings("claude-sonnet-4-5-20250929");
        assert!(migrate_sonnet_45_to_sonnet_46(&mut s));
        assert_eq!(s["model"].as_str(), Some("sonnet"));
    }

    #[test]
    fn sonnet_45_1m_maps_to_sonnet_1m() {
        let mut s = settings("claude-sonnet-4-5-20250929[1m]");
        assert!(migrate_sonnet_45_to_sonnet_46(&mut s));
        assert_eq!(s["model"].as_str(), Some("sonnet[1m]"));
    }

    #[test]
    fn sonnet_46_is_untouched() {
        let mut s = settings("claude-sonnet-4-6");
        assert!(!migrate_sonnet_45_to_sonnet_46(&mut s));
    }

    // ---- struct migrations -------------------------------------------------

    #[test]
    fn bypass_permissions_migrates_and_removes_old_key() {
        let mut s = json!({ "bypassPermissionsAccepted": true });
        assert!(migrate_bypass_permissions_to_settings(&mut s));
        assert!(s.get("bypassPermissionsAccepted").is_none());
        assert_eq!(s["permissionMode"].as_str(), Some("bypass"));
    }

    #[test]
    fn bypass_permissions_false_does_not_set_mode() {
        let mut s = json!({ "bypassPermissionsAccepted": false });
        assert!(migrate_bypass_permissions_to_settings(&mut s));
        assert!(s.get("permissionMode").is_none());
    }

    #[test]
    fn repl_bridge_renames_field() {
        let mut s = json!({ "replBridgeEnabled": true });
        assert!(migrate_repl_bridge_to_remote_control(&mut s));
        assert!(s.get("replBridgeEnabled").is_none());
        assert_eq!(s["remoteControlAtStartup"].as_bool(), Some(true));
    }

    #[test]
    fn enable_all_mcp_renames_field() {
        let mut s = json!({ "enableAllProjectMcpServers": true });
        assert!(migrate_enable_all_mcp_servers(&mut s));
        assert!(s.get("enableAllProjectMcpServers").is_none());
        assert_eq!(s["mcpAutoApprove"].as_bool(), Some(true));
    }

    #[test]
    fn auto_updates_renames_field() {
        let mut s = json!({ "autoUpdatesEnabled": false });
        assert!(migrate_auto_updates(&mut s));
        assert!(s.get("autoUpdatesEnabled").is_none());
        assert_eq!(s["autoUpdates"].as_bool(), Some(false));
    }

    #[test]
    fn reset_auto_mode_clears_sentinel() {
        let mut s = json!({ "autoModeOptIn": "default_offer_2024" });
        assert!(reset_auto_mode_opt_in(&mut s));
        assert!(s["autoModeOptIn"].is_null());
    }

    #[test]
    fn reset_auto_mode_leaves_other_values() {
        let mut s = json!({ "autoModeOptIn": "user_opted_in" });
        assert!(!reset_auto_mode_opt_in(&mut s));
        assert_eq!(s["autoModeOptIn"].as_str(), Some("user_opted_in"));
    }

    #[test]
    fn reset_pro_opus_default_resets_when_not_user_set() {
        let mut s = json!({ "model": "claude-opus-4-5-20251001" });
        assert!(reset_pro_to_opus_default(&mut s));
        assert_eq!(s["model"].as_str(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn reset_pro_opus_default_preserves_when_user_set() {
        let mut s = json!({ "model": "claude-opus-4-5-20251001", "modelSetByUser": true });
        assert!(!reset_pro_to_opus_default(&mut s));
        assert_eq!(s["model"].as_str(), Some("claude-opus-4-5-20251001"));
    }

    // ---- run_migrations integration ----------------------------------------

    #[test]
    fn run_migrations_applies_chain() {
        // A Sonnet 4.5 model should end up as "sonnet" after the full chain.
        let mut s = json!({ "model": "claude-sonnet-4-5-20250929" });
        let changed = run_migrations(&mut s);
        assert!(changed);
        assert_eq!(s["model"].as_str(), Some("sonnet"));
    }

    #[test]
    fn run_migrations_returns_false_when_nothing_changes() {
        let mut s = json!({ "model": "claude-sonnet-4-6", "someOtherKey": 42 });
        assert!(!run_migrations(&mut s));
    }

    #[test]
    fn run_migrations_handles_empty_object() {
        let mut s = json!({});
        // No model fields, no sentinel values → nothing to do.
        assert!(!run_migrations(&mut s));
    }
}
