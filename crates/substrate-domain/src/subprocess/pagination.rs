//! Pagination value objects and search request/result types for the subprocess BC.
//!
//! Introduced in ADR-0057 to support line-based pagination on `subprocess.result`
//! and the new `subprocess.search` tool with regex matching.
//!
//! `Pagination` controls which lines to return and in which order.
//! `Order::Tail` (default) returns the most-recent lines first, matching the
//! behaviour of `tail -n N`. `Order::Head` returns lines in chronological order.
//!
//! References: ADR-0057.

use serde::{Deserialize, Serialize};

use crate::subprocess::errors::SubprocessError;
use crate::subprocess::stream::Stream;
use crate::value_objects::JobId;

// ---- Order ------------------------------------------------------------------

/// The ordering direction applied to line-based pagination.
///
/// Serialized as `"Tail"` / `"Head"` to match the CUE/JSON wire format from
/// ADR-0057 (§"Wire Shape").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub enum Order {
    /// Return the most-recent lines first (equivalent to `tail -n N`).
    ///
    /// This is the default because agents typically care most about recent output.
    #[default]
    Tail,
    /// Return lines in chronological (oldest-first) order.
    ///
    /// Use `Head` when replaying logs from the beginning or building diffs.
    Head,
}

// ---- Pagination -------------------------------------------------------------

/// Line-based pagination cursor for `subprocess.result` and `subprocess.search`.
///
/// `offset` is the 0-based line index into the (ordered) line slice.
/// A first-page call omits `offset` (defaults to `0`).
/// Subsequent pages use `next_offset` from the previous response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pagination {
    /// 0-based line offset into the result set.
    ///
    /// Defaults to `0` (start of the ordered slice).
    #[serde(default)]
    pub offset: u64,

    /// Number of lines to return per page.
    ///
    /// Must be in the range `1..=10_000`. Defaults to `100`.
    #[serde(default = "Pagination::default_page_size")]
    pub page_size: u32,

    /// Ordering direction applied before slicing.
    ///
    /// Defaults to `Tail` (most-recent lines first) per ADR-0057.
    #[serde(default)]
    pub order: Order,
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            offset: 0,
            page_size: Self::default_page_size(),
            order: Order::default(),
        }
    }
}

impl Pagination {
    /// The default number of lines returned per page.
    #[must_use]
    pub const fn default_page_size() -> u32 {
        100
    }

    /// Validates the pagination parameters.
    ///
    /// # Errors
    ///
    /// Returns `SubprocessError::InvalidRequest` when `page_size` is outside
    /// the range `1..=10_000`.
    pub fn validate(&self) -> Result<(), SubprocessError> {
        if self.page_size == 0 || self.page_size > 10_000 {
            return Err(SubprocessError::InvalidRequest {
                msg: format!(
                    "pagination.page_size must be 1..=10_000, got {}",
                    self.page_size
                ),
            });
        }
        Ok(())
    }
}

// ---- SubprocessSearchRequest ------------------------------------------------

/// Request to search subprocess output lines by regex pattern.
///
/// Introduced in ADR-0057. The adapter resolves the job's captured line buffer,
/// applies the compiled regex, and returns paginated `SearchMatch` results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubprocessSearchRequest {
    /// The job whose captured output is searched.
    pub job_id: JobId,

    /// Regular expression pattern (RE2/PCRE compatible per the `regex` crate).
    ///
    /// Length must be in the range `1..=1_024` characters.
    pub pattern: String,

    /// Streams to include in the search.
    ///
    /// Defaults to `[Stdout, Stderr]` when omitted.
    #[serde(default = "SubprocessSearchRequest::default_streams")]
    pub streams: Vec<Stream>,

    /// When `true`, pattern matching ignores ASCII case.
    ///
    /// Defaults to `false`.
    #[serde(default)]
    pub case_insensitive: bool,

    /// Optional pagination for the matched results.
    ///
    /// When `None`, uses `Pagination::default()` (100 lines, `Tail` order,
    /// offset 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<Pagination>,
}

impl Default for SubprocessSearchRequest {
    /// Returns a sentinel `SubprocessSearchRequest` whose `job_id` is the nil UUID
    /// and `pattern` is an empty string.
    ///
    /// This impl exists to satisfy the ADR-0061 contract: every request struct that
    /// has `#[serde(default = "fn")]` field overrides MUST have a manual `Default`
    /// impl (not `#[derive(Default)]`) so that the `is_null() || empty_object`
    /// handler shortcut can be safely introduced in the future without silently
    /// delivering Rust zero-values instead of API-contract defaults.
    ///
    /// The sentinel values are intentionally invalid for production use; callers
    /// MUST supply `job_id` and `pattern` explicitly.  The `streams` field is
    /// initialized to match `default_streams()`, honoring the
    /// `#[serde(default = "SubprocessSearchRequest::default_streams")]` override.
    fn default() -> Self {
        Self {
            job_id: JobId::from_uuid(uuid::Uuid::nil()),
            pattern: String::new(),
            streams: Self::default_streams(),
            case_insensitive: false,
            pagination: None,
        }
    }
}

impl SubprocessSearchRequest {
    /// Default stream set used when the caller omits the `streams` field.
    #[must_use]
    pub fn default_streams() -> Vec<Stream> {
        vec![Stream::Stdout, Stream::Stderr]
    }

    /// Validates the search request.
    ///
    /// # Errors
    ///
    /// - `SubprocessError::InvalidRequest` — `pattern` is empty or exceeds 1 024 chars.
    /// - `SubprocessError::InvalidRequest` — `streams` is explicitly set to an empty list.
    /// - `SubprocessError::InvalidRequest` — `pagination` fails its own validation.
    pub fn validate(&self) -> Result<(), SubprocessError> {
        if self.pattern.is_empty() {
            return Err(SubprocessError::InvalidRequest {
                msg: "search.pattern must not be empty".to_string(),
            });
        }
        if self.pattern.len() > 1_024 {
            return Err(SubprocessError::InvalidRequest {
                msg: format!(
                    "search.pattern length {} exceeds maximum 1_024 characters",
                    self.pattern.len()
                ),
            });
        }
        if self.streams.is_empty() {
            return Err(SubprocessError::InvalidRequest {
                msg: "search.streams must contain at least one stream when explicitly set"
                    .to_string(),
            });
        }
        if let Some(ref p) = self.pagination {
            p.validate()?;
        }
        Ok(())
    }
}

// ---- SubprocessSearchResult -------------------------------------------------

/// Paginated result returned by `SubprocessPort::search`.
///
/// `matches` contains at most `pagination.page_size` entries. When `next_offset`
/// is `Some`, there are more results to fetch using that value as `pagination.offset`
/// in a follow-up call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubprocessSearchResult {
    /// The matched lines, ordered according to `pagination.order`.
    pub matches: Vec<SearchMatch>,

    /// Total number of lines that matched the pattern across all requested streams,
    /// before pagination is applied.
    pub total_matches: u64,

    /// Offset to pass as `pagination.offset` in the next call to retrieve more
    /// results.
    ///
    /// `None` when this page exhausts the result set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
}

// ---- SearchMatch ------------------------------------------------------------

/// A single line in a subprocess output stream that matched the search pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    /// The stream (stdout or stderr) that produced this line.
    pub stream: Stream,

    /// 1-based line number within the stream.
    ///
    /// Line numbers are per-stream and reset to `1` for each spawned job.
    pub line_number: u64,

    /// The full text of the matching line (newline stripped).
    pub line_text: String,
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ADR-0061: SubprocessSearchRequest Default contract tests ------------

    /// `SubprocessSearchRequest::default()` must initialize `streams` to match
    /// the `#[serde(default = "SubprocessSearchRequest::default_streams")]` value.
    ///
    /// Regression guard: if `#[derive(Default)]` were used, `streams` would be
    /// an empty `Vec` (`Vec::default()`), not `[Stdout, Stderr]`.
    #[test]
    fn subprocess_search_request_default_honors_streams_serde_default() {
        let req = SubprocessSearchRequest::default();
        assert_eq!(
            req.streams,
            SubprocessSearchRequest::default_streams(),
            "Default::default() must use default_streams(), not Vec::default()"
        );
        assert!(
            !req.streams.is_empty(),
            "streams must not be empty in the default impl"
        );
        assert!(!req.case_insensitive);
        assert!(req.pagination.is_none());
    }

    // ---- Pagination Default contract test ------------------------------------

    /// `Pagination::default()` must initialize `page_size` to `100` (the value
    /// returned by `Pagination::default_page_size()`), not `0`.
    #[test]
    fn pagination_default_honors_page_size() {
        let p = Pagination::default();
        assert_eq!(
            p.page_size,
            Pagination::default_page_size(),
            "Pagination::default() must use default_page_size()={}, not 0",
            Pagination::default_page_size()
        );
        assert_eq!(p.offset, 0);
        assert_eq!(p.order, Order::Tail);
    }
}
