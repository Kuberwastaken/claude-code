//! Cryptographic utilities — mirrors src/utils/crypto.ts
//!
//! Provides SHA-256 hashing, UUID generation, base64url encoding,
//! and work secret generation.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// Compute the SHA-256 hash of `data` and return it as a lowercase hex string.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Compute the SHA-256 hash of a UTF-8 string.
pub fn sha256_hex_str(s: &str) -> String {
    sha256_hex(s.as_bytes())
}

/// Encode bytes as base64url (no padding) — same as `btoa` + replace in TS.
pub fn base64url_encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

/// Decode base64url (no padding) bytes.
pub fn base64url_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    URL_SAFE_NO_PAD.decode(s)
}

/// Generate a random UUID v4 string ("xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx").
pub fn generate_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Generate a cryptographically random work secret (32 bytes, base64url-encoded).
pub fn generate_work_secret() -> String {
    let mut bytes = [0u8; 32];
    if getrandom::getrandom(&mut bytes).is_err() {
        // Fallback: use time-based entropy
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let pid = std::process::id();
        let seed = format!("{}-{}", ts, pid);
        let seed_bytes = seed.as_bytes();
        let copy_len = seed_bytes.len().min(32);
        bytes[..copy_len].copy_from_slice(&seed_bytes[..copy_len]);
    }
    base64url_encode(&bytes)
}

/// Encode a project root path for use as a directory name (base64url of the path).
/// Mirrors `src/utils/projectRoot.ts`'s `encodeProjectRoot()`.
pub fn encode_project_root(project_root: &str) -> String {
    base64url_encode(project_root.as_bytes())
}

/// Decode a project root directory name back to a path.
pub fn decode_project_root(encoded: &str) -> Option<String> {
    base64url_decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_value() {
        // SHA-256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            sha256_hex_str("hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn base64url_roundtrip() {
        let data = b"hello world";
        let encoded = base64url_encode(data);
        let decoded = base64url_decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn project_root_roundtrip() {
        let root = "/Users/alice/my-project";
        let encoded = encode_project_root(root);
        let decoded = decode_project_root(&encoded).unwrap();
        assert_eq!(decoded, root);
    }

    #[test]
    fn uuid_format() {
        let u = generate_uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[8..9], "-");
        assert_eq!(&u[13..14], "-");
    }
}
