//! Paginated search result support for `text.search`.
//!
//! `text.search` is Bucket-B auto-mode per ADR-0040: results below configured
//! byte/match thresholds are returned inline; results above are promoted to an
//! async job. Within an inline result set, cursor-based pagination per ADR-0008
//! is used when the match count exceeds [`DEFAULT_PAGE_SIZE`].
//!
//! Cursors are opaque to the caller. Internally they encode the 0-based
//! match index from which the next page starts, base64-encoded as a decimal
//! ASCII string. This matches the `PageCursor` semantics in `substrate-domain`.

use substrate_domain::SubstrateResult;

/// Default number of match records returned per page.
pub const DEFAULT_PAGE_SIZE: usize = 50;

/// Maximum allowed page size requested by the caller.
pub const MAX_PAGE_SIZE: usize = 500;

/// A single line-match record produced by `text.search`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MatchRecord {
    /// 1-based line number within the file.
    pub line_number: u64,
    /// Raw content of the matching line (UTF-8).
    pub line: String,
}

/// Paged slice of `MatchRecord` values.
#[derive(Debug, Clone)]
pub struct MatchPage {
    /// Match records for the current page.
    pub records: Vec<MatchRecord>,
    /// Opaque cursor for the next page, or `None` if this is the last page.
    pub next_cursor: Option<String>,
    /// Total number of matches found (across all pages).
    pub total_match_count: u64,
    /// Number of files skipped because they appeared to be binary.
    pub skipped_binary_count: u64,
}

/// Encodes `offset` as an opaque page cursor string.
#[must_use]
pub fn encode_cursor(offset: usize) -> String {
    // Simple decimal encoding; callers treat this as opaque.
    offset.to_string()
}

/// Decodes a cursor string back to a `usize` offset.
///
/// # Errors
///
/// Returns `InvalidArgument` when the cursor is not a valid decimal integer.
pub fn decode_cursor(cursor: &str) -> SubstrateResult<usize> {
    cursor
        .parse::<usize>()
        .map_err(|_| substrate_domain::SubstrateError::InvalidArgument {
            offending_field: "cursor".to_owned(),
            reason: format!("cursor is not a valid page cursor: '{cursor}'"),
            correlation_id: None,
        })
}

/// Slices `all_matches` into a single page starting at `cursor_offset`.
///
/// Returns a [`MatchPage`] whose `next_cursor` is `Some` when more pages remain.
#[must_use]
#[expect(
    clippy::needless_pass_by_value,
    reason = "public API takes ownership to allow callers to move the full match vec; changing to slice would break the call site idiom"
)]
pub fn paginate(
    all_matches: Vec<MatchRecord>,
    skipped_binary_count: u64,
    cursor_offset: usize,
    page_size: usize,
) -> MatchPage {
    let total = all_matches.len();
    let start = cursor_offset.min(total);
    let end = (start + page_size).min(total);
    let records = all_matches[start..end].to_vec();

    let next_cursor = if end < total {
        Some(encode_cursor(end))
    } else {
        None
    };

    MatchPage {
        records,
        next_cursor,
        total_match_count: total as u64,
        skipped_binary_count,
    }
}

// ---- Tests -------------------------------------------------------------------

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

    fn make_records(n: usize) -> Vec<MatchRecord> {
        (1..=n)
            .map(|i| MatchRecord {
                line_number: i as u64,
                line: format!("line {i}"),
            })
            .collect()
    }

    #[test]
    fn first_page_has_no_next_cursor_when_within_size() {
        let records = make_records(10);
        let page = paginate(records, 0, 0, 50);
        assert_eq!(page.records.len(), 10);
        assert!(page.next_cursor.is_none());
        assert_eq!(page.total_match_count, 10);
    }

    #[test]
    fn first_page_returns_next_cursor_when_more_exist() {
        let records = make_records(100);
        let page = paginate(records, 0, 0, 50);
        assert_eq!(page.records.len(), 50);
        assert_eq!(page.next_cursor, Some("50".to_owned()));
    }

    #[test]
    fn second_page_exhausts_remaining() {
        let records = make_records(75);
        let page = paginate(records, 0, 50, 50);
        assert_eq!(page.records.len(), 25);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn cursor_roundtrip() {
        let offset = 137_usize;
        let encoded = encode_cursor(offset);
        let decoded = decode_cursor(&encoded).expect("valid cursor must decode");
        assert_eq!(decoded, offset);
    }

    #[test]
    fn invalid_cursor_returns_error() {
        let err = decode_cursor("not-a-number").unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_INVALID_ARGUMENT");
    }
}
