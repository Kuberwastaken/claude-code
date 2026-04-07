//! CCH (Client-Computed Hash) request signing.
//!
//! Computes an xxHash64 fingerprint of the serialised request body and embeds
//! it in the x-anthropic-billing-header.
//! The server uses the hash to verify the request originated from a legitimate
//! Claurst client and to gate features like fast-mode.

use xxhash_rust::xxh64::xxh64;

const CCH_SEED: u64 = 0x6E52_736A_C806_831E;
const CCH_MASK: u64 = 0xF_FFFF; // 5 hex digits
const CCH_PLACEHOLDER: &str = "cch=00000";

/// Compute the 5-hex-digit CCH hash for `body`.
pub fn compute_cch(body: &[u8]) -> String {
    let hash = xxh64(body, CCH_SEED) & CCH_MASK;
    format!("cch={hash:05x}")
}

/// Return true if `header` contains the placeholder that should be replaced.
pub fn has_cch_placeholder(s: &str) -> bool {
    s.contains(CCH_PLACEHOLDER)
}

/// Replace the placeholder in `s` with the computed hash.
pub fn replace_cch_placeholder(s: &str, hash: &str) -> String {
    s.replacen(CCH_PLACEHOLDER, hash, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_cch_format() {
        let hash = compute_cch(b"test body");
        assert!(hash.starts_with("cch="));
        assert_eq!(hash.len(), 9); // cch= + 5 hex digits
    }

    #[test]
    fn test_has_cch_placeholder() {
        assert!(has_cch_placeholder("header cch=00000 more"));
        assert!(!has_cch_placeholder("header cch=abc12 more"));
        assert!(!has_cch_placeholder("header cch= more"));
    }

    #[test]
    fn test_replace_cch_placeholder() {
        let result = replace_cch_placeholder("cch=00000; other", "cch=abcde");
        assert_eq!(result, "cch=abcde; other");
    }

    #[test]
    fn test_cch_deterministic() {
        let body = b"same body";
        let hash1 = compute_cch(body);
        let hash2 = compute_cch(body);
        assert_eq!(hash1, hash2);
    }
}
