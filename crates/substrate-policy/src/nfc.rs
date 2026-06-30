//! Unicode NFC normalization for path containment checks (ADR-0035 §Decision 6).
//!
//! On macOS APFS/HFS+ a path may arrive in NFD form (decomposed) while the
//! canonicalized allowlist root is stored in NFC form (composed), or vice
//! versa. The two byte strings differ even though they name the same file, so
//! a naive `starts_with` prefix check fails to recognize containment.
//!
//! ADR-0035 §Decision 6 requires normalizing both sides to NFC before any
//! comparison. This module provides the single normalization primitive used at
//! every jail tier and at the allowlist boundary.
//!
//! # Non-UTF-8 paths
//!
//! `unicode-normalization` operates on `&str`. Unix paths are arbitrary byte
//! strings and need not be valid UTF-8. NFC/NFD divergence is, by definition, a
//! property of decoded Unicode scalar values, so it can only occur in
//! valid-UTF-8 paths. When a path is not valid UTF-8 there is no NFC form to
//! compute, so the original bytes are preserved verbatim. This keeps the
//! function total and lossless: it never corrupts a non-UTF-8 path.

use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// Returns `path` with its Unicode scalar values normalized to NFC.
///
/// When `path` is valid UTF-8, the returned `PathBuf` holds the NFC-composed
/// form. When `path` is not valid UTF-8 (no well-defined NFC form), the
/// original bytes are returned unchanged.
///
/// This is idempotent: normalizing an already-NFC path yields the same path.
#[must_use]
pub fn normalize_path(path: &Path) -> PathBuf {
    path.to_str().map_or_else(
        || path.to_path_buf(),
        |utf8| PathBuf::from(utf8.nfc().collect::<String>()),
    )
}

/// Returns `true` when `candidate` is contained beneath `root` after both
/// sides are normalized to NFC (ADR-0035 §Decision 6).
///
/// Use this instead of a bare `Path::starts_with` for any allowlist-root
/// containment post-check, so a kernel-resolved path in one normalization form
/// still matches a stored root in the other form on macOS APFS/HFS+ volumes.
#[must_use]
pub fn is_contained(candidate: &Path, root: &Path) -> bool {
    normalize_path(candidate).starts_with(normalize_path(root))
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use super::*;

    /// "é" as NFD (e + U+0301 combining acute) must normalize to the NFC
    /// single-codepoint form (U+00E9), so the two compare equal afterwards.
    #[test]
    fn nfd_input_normalizes_to_nfc() {
        let nfd = PathBuf::from("/root/cafe\u{0301}");
        let nfc = PathBuf::from("/root/caf\u{00e9}");
        assert_ne!(nfd, nfc, "precondition: NFD and NFC byte strings differ");
        assert_eq!(normalize_path(&nfd), normalize_path(&nfc));
    }

    /// An already-NFC path is returned byte-identical (idempotence).
    #[test]
    fn nfc_input_is_idempotent() {
        let nfc = PathBuf::from("/root/caf\u{00e9}/file.txt");
        assert_eq!(normalize_path(&nfc), nfc);
    }

    /// A plain ASCII path is unchanged.
    #[test]
    fn ascii_path_unchanged() {
        let p = PathBuf::from("/data/sub/file.txt");
        assert_eq!(normalize_path(&p), p);
    }

    /// An NFD candidate is recognized as contained beneath an NFC root.
    #[test]
    fn is_contained_matches_across_normalization_forms() {
        let nfc_root = Path::new("/root/caf\u{00e9}");
        let nfd_child = Path::new("/root/cafe\u{0301}/file.txt");
        assert!(
            is_contained(nfd_child, nfc_root),
            "NFD child must be recognized under NFC root"
        );
    }

    /// A path genuinely outside the root is not reported as contained.
    #[test]
    fn is_contained_rejects_outside() {
        let root = Path::new("/root/data");
        let outside = Path::new("/root/other/file.txt");
        assert!(!is_contained(outside, root));
    }

    /// A non-UTF-8 path is preserved verbatim (no panic, no corruption).
    #[cfg(unix)]
    #[test]
    fn non_utf8_path_preserved() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        // 0xFF is never a valid UTF-8 lead byte.
        let raw = OsStr::from_bytes(b"/data/\xff\xfe");
        let path = Path::new(raw);
        assert!(path.to_str().is_none(), "precondition: path is not UTF-8");
        assert_eq!(normalize_path(path), path.to_path_buf());
    }
}
