//! Transactional temp-file exclusion filter per ADR-0033 and ADR-0041.
//!
//! Every file whose name matches `.tmp.<uuid7>` is excluded from index insertion
//! at walk time. Only the post-rename atomic commit (write-through Layer 1) is
//! allowed to promote an entry into the snapshot under its final, canonical name.
//!
//! Stale `.tmp.*` entries left by a prior crash are evicted by the lazy lstat
//! pass (Layer 0) the first time a lookup would have returned them, since lstat
//! on the temp path succeeds while the caller's filter on the final path would
//! not match.

/// The prefix that identifies a transactional temp file.
///
/// Per ADR-0033, every write tool creates its staging file as
/// `<target>.tmp.<uuid7>` where the suffix is a 26-character
/// Crockford-base32-encoded UUID v7.
const TMP_PREFIX: &str = ".tmp.";

/// UUID v7 in Crockford base32 is exactly 26 characters (128 bits / 5 bits).
const UUID7_BASE32_LEN: usize = 26;

/// Returns `true` when `name` matches the transactional temp-file pattern.
///
/// The pattern is: name ends with `.tmp.` followed by exactly 26 Crockford
/// base32 characters (the canonical UUID v7 encoding used by ADR-0033).
///
/// # Examples
///
/// ```ignore
/// // Module is crate-internal; use the unit tests in this file instead.
/// // is_tmp_file(OsStr::new("myfile.tmp.01hwz3bk6x4b3gvdqc5kzs7yaf")) == true
/// ```
#[must_use]
#[expect(
    clippy::redundant_pub_crate,
    reason = "pub(crate) documents intentional crate-internal visibility for cross-module use"
)]
pub(crate) fn is_tmp_file(name: &std::ffi::OsStr) -> bool {
    let s = name.to_string_lossy();
    // Fast-path: must contain the prefix somewhere before the trailing uuid7.
    let Some(dot_pos) = s.rfind(TMP_PREFIX) else {
        return false;
    };
    let suffix = &s[dot_pos + TMP_PREFIX.len()..];
    // The suffix must be exactly UUID7_BASE32_LEN Crockford base32 characters.
    suffix.len() == UUID7_BASE32_LEN && suffix.chars().all(is_crockford_base32)
}

/// Returns `true` for characters in the Crockford Base32 alphabet.
///
/// Crockford Base32 uses digits `0-9` and letters `A-Z` (case-insensitive)
/// excluding `I`, `L`, `O`, and `U` to avoid visual ambiguity.
#[inline]
const fn is_crockford_base32(c: char) -> bool {
    matches!(c,
        '0'..='9'
        | 'A'..='H' | 'J'..='N' | 'P'..='T' | 'V'..='Z'
        | 'a'..='h' | 'j'..='n' | 'p'..='t' | 'v'..='z'
    )
}

// ---- Unit tests -------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::is_tmp_file;

    // A plausible UUID v7 in Crockford base32 (26 chars, valid alphabet).
    const VALID_UUID7: &str = "01hwz3bk6x4b3gvdqc5kzs7yaf";

    #[test]
    fn detects_valid_tmp_suffix() {
        let name = format!("myfile.tmp.{VALID_UUID7}");
        assert!(is_tmp_file(OsStr::new(&name)), "expected tmp file detected");
    }

    #[test]
    fn rejects_normal_file() {
        assert!(!is_tmp_file(OsStr::new("lib.rs")));
        assert!(!is_tmp_file(OsStr::new("Cargo.toml")));
    }

    #[test]
    fn rejects_too_short_uuid() {
        let short = "01hwz3bk6x4b3gvdqc5kzs7ya"; // 25 chars
        let name = format!("file.tmp.{short}");
        assert!(!is_tmp_file(OsStr::new(&name)));
    }

    #[test]
    fn rejects_invalid_crockford_char() {
        // 'U' is excluded from the Crockford alphabet.
        let name = "file.tmp.UUUUUUUUUUUUUUUUUUUUUUUUU1".to_string();
        assert!(!is_tmp_file(OsStr::new(&name)));
    }

    #[test]
    fn rejects_no_prefix() {
        let name = format!("file.{VALID_UUID7}");
        assert!(!is_tmp_file(OsStr::new(&name)));
    }

    #[test]
    fn accepts_hidden_file_with_tmp_suffix() {
        let name = format!(".hidden.tmp.{VALID_UUID7}");
        assert!(is_tmp_file(OsStr::new(&name)));
    }
}
