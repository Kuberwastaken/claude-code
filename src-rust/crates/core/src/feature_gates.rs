// feature_gates.rs — Env-var-based feature gates and dynamic config.
//
// Replaces the GrowthBook SDK used in the TypeScript source
// (`src/services/analytics/growthbook.ts`). Feature flags are toggled via
// environment variables instead of a remote service, which is simpler and
// dependency-free for the Rust port.

use std::collections::HashMap;

use serde::de::DeserializeOwned;

// ---------------------------------------------------------------------------
// Name normalization
// ---------------------------------------------------------------------------

/// Normalize a gate/config name to the env-var suffix form:
/// uppercase, replace `-` and `.` with `_`, strip other non-alphanumeric
/// characters (except `_`).
///
/// Examples:
///   "my-feature"       → "MY_FEATURE"
///   "tengu.tide.elm"   → "TENGU_TIDE_ELM"
///   "some:special!name" → "SOMESPECIALNAME"
fn normalize_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '-' | '.' => '_',
            c if c.is_alphanumeric() || c == '_' => c.to_ascii_uppercase(),
            _ => '\0', // sentinel — filtered out below
        })
        .filter(|&c| c != '\0')
        .collect()
}

// ---------------------------------------------------------------------------
// Feature gates
// ---------------------------------------------------------------------------

/// Check whether a named feature gate is enabled.
///
/// Reads `CLAURST_FEATURE_<NORMALIZED_NAME>` and returns `true` when the
/// value is truthy ("1", "true", "yes", "on" — case-insensitive).
///
/// Mirrors `checkStatsigFeatureGate_CACHED_MAY_BE_STALE` from the TypeScript
/// GrowthBook integration.
pub fn is_feature_enabled(gate_name: &str) -> bool {
    let key = format!("CLAURST_FEATURE_{}", normalize_name(gate_name));
    is_env_truthy(std::env::var(&key).ok().as_deref())
}

// ---------------------------------------------------------------------------
// Dynamic config
// ---------------------------------------------------------------------------

/// Read a JSON-encoded dynamic config from an environment variable.
///
/// Reads `CLAURST_DYNAMIC_CONFIG_<NORMALIZED_NAME>`.  If the variable is
/// not set, or parsing fails, `default` is returned unchanged.
///
/// Mirrors `getDynamicConfig_CACHED_MAY_BE_STALE` from the TypeScript source.
pub fn get_dynamic_config<T: DeserializeOwned>(name: &str, default: T) -> T {
    let key = format!("CLAURST_DYNAMIC_CONFIG_{}", normalize_name(name));
    match std::env::var(&key) {
        Ok(val) => serde_json::from_str(&val).unwrap_or(default),
        Err(_) => default,
    }
}

// ---------------------------------------------------------------------------
// Bare / simple mode
// ---------------------------------------------------------------------------

/// Return `true` when Claurst should run in "bare" (minimal) mode.
///
/// Bare mode skips LSP, plugin, and MCP startup for a faster, lighter
/// experience.  It is enabled by either:
///   - The `CLAURST_SIMPLE=1` environment variable, OR
///   - The `--bare` flag in `std::env::args()`.
pub fn is_bare_mode() -> bool {
    // Check env var
    if is_env_truthy(std::env::var("CLAURST_SIMPLE").ok().as_deref()) {
        return true;
    }
    // Check CLI args without going through clap (avoids a full parse at this stage)
    std::env::args().any(|a| a == "--bare")
}

// ---------------------------------------------------------------------------
// Env-var truthiness helpers
// ---------------------------------------------------------------------------

/// Return `true` when `val` is a truthy env-var value.
///
/// Truthy: `"1"`, `"true"`, `"yes"`, `"on"` (case-insensitive).
/// `None` (variable unset) is falsy.
pub fn is_env_truthy(val: Option<&str>) -> bool {
    match val {
        Some(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        None => false,
    }
}

/// Return `true` when `val` is an explicitly-falsy env-var value.
///
/// Falsy: `"0"`, `"false"`, `"no"`, `"off"` (case-insensitive).
/// `None` (variable unset) returns `false` — unset is *not* defined-falsy.
pub fn is_env_defined_falsy(val: Option<&str>) -> bool {
    match val {
        Some(v) => {
            matches!(
                v.to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Env-var parsing for --env KEY=VALUE arguments
// ---------------------------------------------------------------------------

/// Parse a slice of `"KEY=VALUE"` strings into a `HashMap`.
///
/// Returns an error if any entry lacks a `=` separator.
pub fn parse_env_vars(args: &[String]) -> anyhow::Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for entry in args {
        if let Some(pos) = entry.find('=') {
            let key = entry[..pos].to_string();
            let value = entry[pos + 1..].to_string();
            map.insert(key, value);
        } else {
            return Err(anyhow::anyhow!(
                "Invalid env-var format '{}': expected KEY=VALUE",
                entry
            ));
        }
    }
    Ok(map)
}

// ---------------------------------------------------------------------------
// AWS region
// ---------------------------------------------------------------------------

/// Resolve the AWS region, checking `AWS_REGION` then `AWS_DEFAULT_REGION`,
/// falling back to `"us-east-1"`.
pub fn get_aws_region() -> String {
    std::env::var("AWS_REGION")
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|_| "us-east-1".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- normalize_name ---

    #[test]
    fn normalize_replaces_dashes_and_dots() {
        assert_eq!(normalize_name("my-feature"), "MY_FEATURE");
        assert_eq!(normalize_name("tengu.tide.elm"), "TENGU_TIDE_ELM");
        assert_eq!(normalize_name("a-b.c"), "A_B_C");
    }

    #[test]
    fn normalize_strips_special_chars() {
        assert_eq!(normalize_name("some:special!name"), "SOMESPECIALNAME");
    }

    #[test]
    fn normalize_preserves_underscores() {
        assert_eq!(normalize_name("already_upper"), "ALREADY_UPPER");
    }

    // --- is_env_truthy ---

    #[test]
    fn truthy_values() {
        for v in &["1", "true", "True", "TRUE", "yes", "YES", "on", "ON"] {
            assert!(is_env_truthy(Some(v)), "expected truthy for {:?}", v);
        }
    }

    #[test]
    fn falsy_values_are_not_truthy() {
        for v in &["0", "false", "no", "off", "", "anything"] {
            assert!(!is_env_truthy(Some(v)), "expected non-truthy for {:?}", v);
        }
        assert!(!is_env_truthy(None));
    }

    // --- is_env_defined_falsy ---

    #[test]
    fn defined_falsy_values() {
        for v in &["0", "false", "False", "FALSE", "no", "NO", "off", "OFF"] {
            assert!(
                is_env_defined_falsy(Some(v)),
                "expected defined-falsy for {:?}",
                v
            );
        }
    }

    #[test]
    fn non_falsy_values() {
        for v in &["1", "true", "yes", "on", ""] {
            assert!(
                !is_env_defined_falsy(Some(v)),
                "expected non-defined-falsy for {:?}",
                v
            );
        }
        assert!(!is_env_defined_falsy(None));
    }

    // --- parse_env_vars ---

    #[test]
    fn parse_env_vars_basic() {
        let args = vec!["KEY=VALUE".to_string(), "FOO=bar=baz".to_string()];
        let map = parse_env_vars(&args).unwrap();
        assert_eq!(map["KEY"], "VALUE");
        // value may contain `=`
        assert_eq!(map["FOO"], "bar=baz");
    }

    #[test]
    fn parse_env_vars_error_on_no_equals() {
        let args = vec!["NOEQUALSSIGN".to_string()];
        assert!(parse_env_vars(&args).is_err());
    }

    // --- get_aws_region ---

    #[test]
    fn aws_region_fallback() {
        // Ensure the fallback works when neither env var is set.
        // We can't easily unset env vars in tests, so we just verify the
        // function returns a non-empty string.
        let region = get_aws_region();
        assert!(!region.is_empty());
    }

    // --- get_dynamic_config ---

    #[test]
    fn dynamic_config_returns_default_when_unset() {
        // Use an unlikely key so we don't collide with a real env var.
        let val: u32 = get_dynamic_config("__test_unset_key_xyzzy__", 42u32);
        assert_eq!(val, 42);
    }
}
