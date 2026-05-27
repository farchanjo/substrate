//! Pagination value objects and search request/result types for the subprocess BC.
//!
//! Introduced in ADR-0057 to support line-based pagination on `subprocess.result`
//! and the new `subprocess.search` tool with regex matching.
//!
//! `Pagination` controls which lines to return and in which order.
//! `Order::Tail` (default) returns the most-recent lines first, matching the
//! behaviour of `tail -n N`. `Order::Head` returns lines in chronological order.
//!
//! References: ADR-0057, ADR-0060.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::subprocess::errors::SubprocessError;
use crate::subprocess::stream::Stream;
use crate::value_objects::JobId;
use crate::value_objects::pagination::PageSize;

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
///
/// The wire format keeps `page_size` as a raw `u32` JSON value for backward
/// compatibility.  The in-memory representation uses [`PageSize`], so
/// validation (range `1..=10_000`) is enforced at deserialization time.
///
/// Custom `Serialize` / `Deserialize` impls are provided via the private
/// [`PaginationWire`] helper; they mirror the derive-generated output exactly.
#[derive(Debug, Clone)]
pub struct Pagination {
    /// 0-based line offset into the result set.
    ///
    /// Defaults to `0` (start of the ordered slice).
    pub offset: u64,

    /// Number of lines to return per page.
    ///
    /// Must be in the range `1..=10_000`. Defaults to
    /// [`PageSize::DEFAULT_PAGINATION`] (100) per ADR-0057.
    pub page_size: PageSize,

    /// Ordering direction applied before slicing.
    ///
    /// Defaults to `Tail` (most-recent lines first) per ADR-0057.
    pub order: Order,
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            offset: 0,
            page_size: PageSize::DEFAULT_PAGINATION,
            order: Order::default(),
        }
    }
}

impl Pagination {
    /// Validates the pagination parameters.
    ///
    /// Range validation (`page_size` in `1..=10_000`) is enforced by
    /// [`PageSize`] at deserialization time, so this method is now a no-op
    /// retained for call-site compatibility.
    ///
    /// # Errors
    ///
    /// Always returns `Ok(())`.
    pub const fn validate(&self) -> Result<(), SubprocessError> {
        Ok(())
    }
}

// ---- Wire helper for Serialize / Deserialize --------------------------------

/// Private wire representation of [`Pagination`].
///
/// Holds `page_size` as a plain `u32` so the JSON wire format is
/// backward-compatible (`{"offset":0,"page_size":100,"order":"Tail"}`).
/// The `Serialize` / `Deserialize` impls on [`Pagination`] delegate through
/// this struct.
#[derive(Serialize, Deserialize)]
struct PaginationWire {
    #[serde(default)]
    offset: u64,
    #[serde(default = "PaginationWire::default_page_size")]
    page_size: u32,
    #[serde(default)]
    order: Order,
}

impl PaginationWire {
    /// Wire default mirrors [`PageSize::DEFAULT_PAGINATION`] (100).
    const fn default_page_size() -> u32 {
        PageSize::DEFAULT_PAGINATION.get()
    }
}

impl Serialize for Pagination {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        PaginationWire {
            offset: self.offset,
            page_size: self.page_size.get(),
            order: self.order,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Pagination {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = PaginationWire::deserialize(deserializer)?;
        let page_size =
            PageSize::try_from(wire.page_size).map_err(|e| de::Error::custom(e.to_string()))?;
        Ok(Self {
            offset: wire.offset,
            page_size,
            order: wire.order,
        })
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
    #![expect(
        clippy::unwrap_used,
        reason = "test code: idiomatic panicking assertions are intentional"
    )]

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

    /// `Pagination::default()` must initialize `page_size` to `100`
    /// (`PageSize::DEFAULT_PAGINATION`), not `0` or `50`.
    #[test]
    fn pagination_default_honors_page_size() {
        let p = Pagination::default();
        assert_eq!(
            p.page_size.get(),
            100,
            "Pagination::default() must use PageSize::DEFAULT_PAGINATION (100)"
        );
        assert_eq!(p.offset, 0);
        assert_eq!(p.order, Order::Tail);
    }

    // ---- Pagination serde round-trip tests -----------------------------------

    /// Serialize `Pagination` → JSON and parse back; values must be equal.
    #[test]
    fn pagination_serde_round_trip() {
        let original = Pagination {
            offset: 0,
            page_size: PageSize::try_from(100_u32).unwrap(),
            order: Order::Tail,
        };
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, r#"{"offset":0,"page_size":100,"order":"Tail"}"#);
        let parsed: Pagination = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.offset, original.offset);
        assert_eq!(parsed.page_size.get(), original.page_size.get());
        assert_eq!(parsed.order, original.order);
    }

    /// Wire value `page_size: 0` must be rejected with an `InvalidArgument` error.
    #[test]
    fn pagination_deserialize_page_size_zero_is_err() {
        let result: Result<Pagination, _> = serde_json::from_str(r#"{"offset":0,"page_size":0}"#);
        assert!(result.is_err(), "page_size=0 must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("page_size"),
            "error message must mention page_size; got: {msg}"
        );
    }

    /// Wire value `page_size: 10001` must be rejected (above MAX).
    #[test]
    fn pagination_deserialize_page_size_above_max_is_err() {
        let result: Result<Pagination, _> =
            serde_json::from_str(r#"{"offset":0,"page_size":10001}"#);
        assert!(result.is_err(), "page_size=10001 must be rejected");
    }

    /// Wire without `page_size` field must use the default of 100.
    #[test]
    fn pagination_deserialize_absent_page_size_defaults_to_100() {
        let p: Pagination = serde_json::from_str(r#"{"offset":0}"#).unwrap();
        assert_eq!(
            p.page_size.get(),
            100,
            "absent page_size must default to PageSize::DEFAULT_PAGINATION (100)"
        );
    }
}
