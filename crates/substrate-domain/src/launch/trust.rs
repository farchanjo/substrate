//! `TrustRecord` — one bless entry in the user-scope trust store (`trust.toml`).
//!
//! Mirrors `#TrustRecord` in `docs/arch/schemas/launch.cue` (ADR-0064). It binds
//! a canonical Profile path to its full inode-and-content identity tuple, which
//! is re-verified on every load to defeat permission-flip and rewrite attacks.
//!
//! References: ADR-0064 §"Trust model (TOFU)".

use serde::{Deserialize, Serialize};

/// One trust-store entry binding a Profile path to its inode/content identity.
///
/// Every field captured at bless time is re-checked on each load; a single
/// mismatch invalidates the record (the Profile is then treated as untrusted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRecord {
    /// Absolute canonical path of the trusted `.substrate.toml`.
    pub path: String,
    /// Device id from `fstat` at bless time.
    pub dev: u64,
    /// Inode number from `fstat` at bless time.
    pub ino: u64,
    /// Owning user id from `fstat` at bless time.
    pub uid: u32,
    /// Permission bits masked to `0o7777` (`0..=4095`).
    pub mode: u32,
    /// Prefixed content hash captured at bless time, matching `^(blake3|sha256):`.
    pub content: String,
    /// RFC 3339 timestamp the record was created.
    pub blessed_at: String,
}

impl TrustRecord {
    /// Returns `true` when the supplied live identity tuple matches this record exactly.
    ///
    /// Full-tuple equality: `dev`, `ino`, `uid`, `mode`, and `content` must all
    /// match. A single differing field returns `false`, invalidating the trust
    /// (per `launch-trust-invalidated-on-edit.feature`).
    #[must_use]
    pub fn matches(&self, dev: u64, ino: u64, uid: u32, mode: u32, content: &str) -> bool {
        self.dev == dev
            && self.ino == ino
            && self.uid == uid
            && self.mode == mode
            && self.content == content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> TrustRecord {
        TrustRecord {
            path: "/home/dev/project/.substrate.toml".to_owned(),
            dev: 66,
            ino: 1234,
            uid: 1000,
            mode: 0o644,
            content: "blake3:abc123".to_owned(),
            blessed_at: "2026-06-30T12:00:00Z".to_owned(),
        }
    }

    #[test]
    fn matches_full_tuple() {
        let r = record();
        assert!(r.matches(66, 1234, 1000, 0o644, "blake3:abc123"));
    }

    #[test]
    fn content_mismatch_fails() {
        let r = record();
        assert!(!r.matches(66, 1234, 1000, 0o644, "blake3:deadbeef"));
    }

    #[test]
    fn ino_mismatch_fails() {
        let r = record();
        assert!(!r.matches(66, 9999, 1000, 0o644, "blake3:abc123"));
    }

    #[test]
    fn mode_mismatch_fails() {
        let r = record();
        assert!(!r.matches(66, 1234, 1000, 0o600, "blake3:abc123"));
    }

    #[test]
    fn uid_mismatch_fails() {
        let r = record();
        assert!(!r.matches(66, 1234, 0, 0o644, "blake3:abc123"));
    }

    #[test]
    fn dev_mismatch_fails() {
        let r = record();
        assert!(!r.matches(1, 1234, 1000, 0o644, "blake3:abc123"));
    }
}
