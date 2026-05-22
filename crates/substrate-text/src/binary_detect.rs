//! Binary-file detector for text-processing tools.
//!
//! Per the Gherkin spec `text-search-binary-file-skipped.feature`, files that
//! appear to contain binary content are silently excluded from `text.search`
//! results. The detector is heuristic — it inspects only the first 4096 bytes
//! of the file content supplied by the caller.
//!
//! Two independent checks are applied in order (both via SIMD-accelerated
//! crates, per ADR-0043):
//!
//! 1. **Null-byte scan** — `memchr::memchr(0, ...)` locates the first zero
//!    byte. A null byte within the sniff window is a strong binary indicator
//!    (used by git, ripgrep, file(1)).
//!
//! 2. **UTF-8 validation** — `simdutf8::basic::from_utf8(...)` validates the
//!    sniff window. Non-UTF-8 sequences indicate binary or non-text encodings.
//!
//! Both checks operate on `buf[..buf.len().min(SNIFF_WINDOW)]` so callers do
//! not need to pre-slice the buffer.

/// Number of bytes inspected from the start of a file for binary detection.
pub const SNIFF_WINDOW: usize = 4096;

/// Returns `true` when `buf` appears to contain binary (non-text) content.
///
/// The check is heuristic, not exhaustive. Files that pass this check are
/// treated as UTF-8 text for search purposes. Files that fail are silently
/// skipped and counted in `skipped_binary_count` metadata.
///
/// # Arguments
///
/// * `buf` — raw file content. Only the first [`SNIFF_WINDOW`] bytes are
///   examined; callers may pass a larger buffer.
#[must_use]
pub fn is_binary(buf: &[u8]) -> bool {
    let window = &buf[..buf.len().min(SNIFF_WINDOW)];

    // Check 1: null-byte scan via memchr (SIMD-accelerated on x86-64 AVX2/SSE2
    // and aarch64 NEON per ADR-0043).
    if memchr::memchr(0, window).is_some() {
        return true;
    }

    // Check 2: UTF-8 validation via simdutf8 (AVX2 / SSE4.2 / NEON per ADR-0043).
    simdutf8::basic::from_utf8(window).is_err()
}

// ---- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_is_not_binary() {
        assert!(!is_binary(&[]));
    }

    #[test]
    fn ascii_text_is_not_binary() {
        let content = b"Hello, world!\nThis is plain ASCII text.\n";
        assert!(!is_binary(content));
    }

    #[test]
    fn utf8_text_is_not_binary() {
        let content = "Hello \u{1F600} world\n".as_bytes();
        assert!(!is_binary(content));
    }

    #[test]
    fn null_byte_triggers_binary() {
        let mut content = vec![b'H', b'e', b'l', b'l', b'o'];
        content.push(0u8); // null byte
        content.extend_from_slice(b" world");
        assert!(is_binary(&content));
    }

    #[test]
    fn invalid_utf8_triggers_binary() {
        // Lone continuation byte — invalid UTF-8.
        let content = &[0x80u8, 0x41, 0x41];
        assert!(is_binary(content));
    }

    #[test]
    fn null_after_sniff_window_not_detected() {
        // Content that is valid UTF-8 for the first SNIFF_WINDOW bytes.
        let mut content = vec![b'a'; SNIFF_WINDOW];
        content.push(0u8); // null byte AFTER the sniff window
        // The function only inspects the first SNIFF_WINDOW bytes,
        // so this should NOT be detected as binary.
        assert!(!is_binary(&content));
    }
}
