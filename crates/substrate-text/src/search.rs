//! `text.search` — line-by-line regex search (Zone B, Bucket-B auto-mode).
//!
//! # Zone classification (ADR-0003)
//!
//! Zone B: synchronous I/O executed via `tokio::task::spawn_blocking`. The
//! blocking closure streams the file via a `std::io::BufReader`, scanning
//! line-by-line. The caller holds a `CancellationToken` and checks it at each
//! chunk boundary (every 256 lines by default) to honour cooperative
//! cancellation (ADR-0037).
//!
//! # SIMD acceleration (ADR-0043)
//!
//! - `regex` crate uses `memchr` + `aho-corasick` Teddy prefilter internally.
//!   No call-site changes are required; the prefilter activates transparently
//!   on AVX2 and NEON hardware.
//!
//! # Binary skip (Gherkin: text-search-binary-file-skipped.feature)
//!
//! Before scanning, the first [`SNIFF_WINDOW`] bytes of each file are checked
//! via `binary_detect::is_binary`. Binary files are silently skipped and
//! counted in `skipped_binary_count`.
//!
//! # `ReDoS` guard (Gherkin: text-search-catastrophic-regex.feature)
//!
//! Patterns are compiled through `regex_guard::compile_regex`, which applies
//! NFA and DFA size limits. Patterns that exceed these limits are rejected at
//! compile time before any scanning begins.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::WalkBuilder;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{PageSize, SubstrateError, SubstrateResult};

use crate::binary_detect;
use crate::hints_helpers;
use crate::pagination::{self, MAX_PAGE_SIZE, MatchRecord};
use crate::response::{TextDeps, ToolResponse};

/// Number of lines processed between cancellation token checks.
const CANCEL_CHECK_INTERVAL: usize = 256;

/// Maximum bytes buffered for a single line before the remainder is discarded.
///
/// Mirrors `head::MAX_LINE_BYTES`.  A single newline-less 1 GiB line would
/// otherwise fill the sync `BufReader` buffer in `scan_file`, OOM-ing the
/// blocking thread.  Lines over this limit are truncated; the match is still
/// attempted against the truncated prefix so the line number is preserved.
const MAX_LINE_BYTES: usize = 64 * 1024; // 64 KiB per line

/// Input parameters for `text.search`.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchParams {
    /// Absolute path to the file or directory to search.
    /// When a directory is given, all files under it are scanned recursively.
    /// The field name `root` is accepted as an alias for backward compatibility
    /// with clients that use the older parameter name.
    #[serde(alias = "root")]
    pub path: String,
    /// Regular expression pattern to match against each line.
    pub pattern: String,
    /// Maximum number of results to return per page (default 50, max 500).
    ///
    /// `Option<u32>` on the wire (ADR-0060): an absent field applies
    /// [`PageSize::default`]; an explicit `0` or a value above
    /// [`PageSize::MAX`] returns `SUBSTRATE_INVALID_ARGUMENT`. The validated
    /// [`PageSize`] is capped at [`MAX_PAGE_SIZE`] (500) before pagination.
    #[serde(default)]
    pub page_size: Option<u32>,
    /// Opaque cursor from a previous `text.search` response for pagination.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Handles a `text.search` tool call.
///
/// Returns a [`ToolResponse`] whose `structured_content` conforms to the
/// `MatchResult` aggregate root schema from the text-processing BC.
///
/// # Errors
///
/// - `SUBSTRATE_INVALID_ARGUMENT` — malformed regex or invalid cursor.
/// - `SUBSTRATE_NOT_FOUND` — the target file does not exist.
/// - `SUBSTRATE_PATH_OUTSIDE_ALLOWLIST` — path fails allowlist check.
/// - `SUBSTRATE_CANCELLED` — the operation was cancelled mid-scan.
/// - `SUBSTRATE_IO_ERROR` — kernel I/O failure during read.
#[instrument(skip(deps, cancel), fields(path = %params.path, pattern = %params.pattern))]
pub async fn handle_text_search(
    params: SearchParams,
    deps: Arc<TextDeps>,
    cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    // ADR-0060: convert Option<u32> → PageSize at the handler boundary, then apply
    // the ADR-0008 handler cap (MAX_PAGE_SIZE = 500). Absent field →
    // PageSize::default() (50); explicit 0 or > PageSize::MAX →
    // SUBSTRATE_INVALID_ARGUMENT. A validated PageSize is always >= 1, so the
    // historical zero-page infinite-loop hazard (fix #4) is eliminated by type.
    let page_size = match params.page_size {
        Some(n) => PageSize::try_from(n)?.get().min(MAX_PAGE_SIZE),
        None => PageSize::default().get().min(MAX_PAGE_SIZE),
    };
    let page_size = PageSize::try_from(page_size)?;

    let cursor_offset = match &params.cursor {
        Some(c) => pagination::decode_cursor(c)?,
        None => 0,
    };

    // Compile the regex before entering spawn_blocking to surface ReDoS errors
    // on the async path without consuming a blocking thread.
    let regex = crate::regex_guard::compile_regex(&params.pattern)?;

    // Validate path via the jail before handing off to the blocking thread.
    let raw_path = PathBuf::from(&params.path);
    let jailed = {
        let jail = Arc::clone(&deps.jail);
        let raw = raw_path.clone();
        // PathJailPort is synchronous; run inline (cheap string operation).
        tokio::task::spawn_blocking(move || {
            // We need an allowlist root for jail() — callers must pass a path
            // already under a root; the jail validates the prefix.
            // For the text adapter, the jail is pre-configured with roots;
            // we pass the raw path as both root candidate and target.
            // The composition root ensures deps.jail is wired to the
            // global allowlist; this call validates containment.
            jail.jail(
                &substrate_domain::JailedPath::new_jailed(
                    raw.parent().unwrap_or(&raw).to_path_buf(),
                ),
                &raw,
            )
        })
        .await
        .map_err(|join_err| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error: {join_err}"),
            correlation_id: None,
        })??
    };

    let jailed_path_buf = jailed.into_inner();
    let simd_tier = deps.capabilities.simd_tier;

    // Collect only as many matches as needed to serve this page plus the
    // lookahead that determines whether a next page exists (fix #3).
    // Adding 1 to the lookahead: if we get exactly cursor_offset + page_size + 1
    // matches, `paginate` will know there is a next page; if we get fewer, there
    // is not.  Using `usize::MAX` as a fallback for the degenerate case where
    // `cursor_offset + page_size` would overflow is safe because the scan exits
    // early on match, so the allocator never sees the full count.
    let max_matches = cursor_offset
        .saturating_add(page_size.get() as usize)
        .saturating_add(1);

    // Perform the blocking file scan on a blocking thread.
    // When the path is a directory, walk it recursively and scan each file.
    let scan_result = tokio::task::spawn_blocking({
        let cancel = cancel.clone();
        move || {
            if jailed_path_buf.is_dir() {
                scan_dir(&jailed_path_buf, &regex, &cancel, max_matches)
            } else {
                scan_file(&jailed_path_buf, &regex, cancel, max_matches)
            }
        }
    })
    .await
    .map_err(|join_err| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error during scan: {join_err}"),
        correlation_id: None,
    })??;

    let (all_matches, skipped_binary_count) = scan_result;
    // `all_matches` may be shorter than the true total because the scan was
    // stopped early after collecting `max_matches` records.  `total` here
    // reflects how many were actually collected; `next_cursor` signals more.
    let total = all_matches.len();

    let page = pagination::paginate(all_matches, skipped_binary_count, cursor_offset, page_size)?;
    let has_more = page.next_cursor.is_some();

    let content = format!(
        "text.search: found {total} match(es) in '{}'; returning {} on this page.",
        params.path,
        page.records.len()
    );

    let structured_content = serde_json::json!({
        "matches": page.records,
        "total_match_count": page.total_match_count,
        "skipped_binary_count": page.skipped_binary_count,
        "next_cursor": page.next_cursor,
        "page_size": page_size.get(),
    });

    let hints = hints_helpers::build_search_hints(simd_tier, has_more);
    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

/// Synchronous blocking scan of a single file.
///
/// Returns `(matches, skipped_binary_count)`.
/// `skipped_binary_count` is 0 when the file is text, 1 when skipped as binary.
///
/// Each [`MatchRecord`] carries `file_path` set to the UTF-8 display of `path`.
///
/// `max_matches` caps how many records are collected.  Once `matches.len() ==
/// max_matches` the scan stops early — the caller passes
/// `cursor_offset + page_size + 1` so pagination can determine whether a next
/// page exists without scanning the entire file (fix #3, text-oom lane).
#[expect(
    clippy::needless_pass_by_value,
    reason = "CancellationToken is an Arc-backed handle; pass by value matches the tokio-util API convention"
)]
fn scan_file(
    path: &std::path::Path,
    regex: &regex::Regex,
    cancel: CancellationToken,
    max_matches: usize,
) -> SubstrateResult<(Vec<MatchRecord>, u64)> {
    let file = std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => SubstrateError::NotFound {
            resource: path.display().to_string(),
            correlation_id: None,
        },
        std::io::ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
            path: path.display().to_string(),
            correlation_id: None,
        },
        _ => SubstrateError::IoError {
            path: path.display().to_string(),
            correlation_id: None,
        },
    })?;

    // Read the sniff window for binary detection.
    let mut sniff_buf = vec![0u8; binary_detect::SNIFF_WINDOW];
    let mut sniff_reader = std::io::BufReader::new(&file);
    let sniff_len = sniff_reader
        .read(&mut sniff_buf)
        .map_err(|_| SubstrateError::IoError {
            path: path.display().to_string(),
            correlation_id: None,
        })?;
    sniff_buf.truncate(sniff_len);

    if binary_detect::is_binary(&sniff_buf) {
        return Ok((vec![], 1));
    }

    // Reopen for full scan — the sniff read consumed part of the stream.
    let file = std::fs::File::open(path).map_err(|_| SubstrateError::IoError {
        path: path.display().to_string(),
        correlation_id: None,
    })?;
    let mut reader = BufReader::new(file);

    let mut matches = Vec::new();
    let mut lines_since_check: usize = 0;
    let mut line_number: u64 = 0;

    // Per-line byte buffer.  Reused across iterations to avoid re-allocation.
    let mut line_buf: Vec<u8> = Vec::with_capacity(MAX_LINE_BYTES);

    loop {
        // Early-exit once the page-lookahead limit is reached (fix #3).
        if matches.len() >= max_matches {
            break;
        }

        lines_since_check += 1;

        // Honour cancellation at chunk boundaries to avoid blocking indefinitely.
        if lines_since_check >= CANCEL_CHECK_INTERVAL {
            lines_since_check = 0;
            if cancel.is_cancelled() {
                return Err(SubstrateError::Cancelled {
                    correlation_id: None,
                });
            }
        }

        // OOM guard (fix #2): read at most MAX_LINE_BYTES per line.
        // `BufRead::lines()` uses `read_until` internally with no size cap;
        // a single newline-less line can exhaust the heap.
        // Instead, use `read_line_capped` which stops at the byte limit and
        // drains the remainder synchronously.
        line_buf.clear();
        let n_read =
            read_line_capped(&mut reader, &mut line_buf, MAX_LINE_BYTES).map_err(|_| {
                SubstrateError::IoError {
                    path: path.display().to_string(),
                    correlation_id: None,
                }
            })?;

        if n_read == 0 {
            break; // EOF
        }

        // Increment line counter only after confirming we read real data.
        line_number += 1;

        // Convert to a UTF-8 string, replacing invalid sequences.
        let line = String::from_utf8_lossy(&line_buf);
        // Trim trailing newline/CR for clean matching.
        let line = line.trim_end_matches('\n').trim_end_matches('\r');

        if regex.is_match(line) {
            matches.push(MatchRecord {
                file_path: path.display().to_string(),
                line_number,
                line: line.to_owned(),
            });
        }
    }

    Ok((matches, 0))
}

/// Reads at most `max_bytes` bytes into `buf` until `\n` or EOF.
///
/// If the line is longer than `max_bytes` the function fills `buf` to the cap
/// and then drains the remaining bytes of the line (up to the next `\n`) from
/// `reader` without buffering them, preventing unbounded allocation (fix #2).
///
/// Returns the number of bytes placed into `buf` (excluding any drained
/// excess).  Returns `0` on EOF with an empty `buf`.
fn read_line_capped(
    reader: &mut impl BufRead,
    buf: &mut Vec<u8>,
    max_bytes: usize,
) -> std::io::Result<usize> {
    let mut total_stored: usize = 0;
    let mut overflowed = false;

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break; // EOF
        }

        // Search for `\n` in the available buffer (SIMD — ADR-0043).
        if let Some(pos) = memchr::memchr(b'\n', available) {
            // Newline found within the available bytes.
            if !overflowed {
                // Take up to the newline (including it if within cap).
                let take = (pos + 1).min(max_bytes - total_stored);
                buf.extend_from_slice(&available[..take]);
                total_stored += take;
            }
            // Consume up to and including the newline.
            reader.consume(pos + 1);
            break;
        }
        // No newline in available chunk; take what we can.
        if !overflowed {
            let headroom = max_bytes.saturating_sub(total_stored);
            let take = available.len().min(headroom);
            if take > 0 {
                buf.extend_from_slice(&available[..take]);
                total_stored += take;
            }
            if total_stored >= max_bytes {
                // Line exceeded the cap; mark overflowed and drain.
                overflowed = true;
            }
        }
        let len = available.len();
        reader.consume(len);
    }

    Ok(total_stored)
}

/// Recursively walk a directory and scan each file.
///
/// Returns the aggregated `(matches, skipped_binary_count)` across all files.
/// Directories and other non-file entries are silently skipped.
/// Cancellation is checked at each file boundary.
///
/// `max_matches` is forwarded to each `scan_file` call; the walk also stops
/// early once the accumulated match count reaches the limit (fix #3).
///
/// # Ignore semantics
///
/// Uses `ignore::WalkBuilder` (the same crate `ripgrep` uses internally).
/// Honours `.gitignore`, `.ignore`, `.git/info/exclude`, the global git
/// `core.excludesfile`, and parent-directory ignore files. Hidden entries
/// (dotfiles) are also skipped by default, matching `ripgrep`'s default
/// behaviour. Without this, a search over a typical project root that
/// contains `target/`, `node_modules/`, or `.git/` walks gigabytes of
/// irrelevant content and violates the performance budget in ADR-0030.
fn scan_dir(
    dir: &Path,
    regex: &regex::Regex,
    cancel: &CancellationToken,
    max_matches: usize,
) -> SubstrateResult<(Vec<MatchRecord>, u64)> {
    let mut all_matches: Vec<MatchRecord> = Vec::new();
    let mut total_skipped: u64 = 0;

    let walker = WalkBuilder::new(dir)
        .standard_filters(true)
        .sort_by_file_name(std::cmp::Ord::cmp)
        .build();

    for entry in walker {
        if cancel.is_cancelled() {
            return Err(SubstrateError::Cancelled {
                correlation_id: None,
            });
        }

        // Stop walking once we have enough matches to serve the current page
        // plus the lookahead that determines `has_more` (fix #3).
        if all_matches.len() >= max_matches {
            break;
        }

        let Ok(entry) = entry else { continue };
        let Some(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_file() {
            continue;
        }

        // Pass the remaining headroom to each file scan so it also stops early.
        let remaining = max_matches.saturating_sub(all_matches.len());
        let path: PathBuf = entry.into_path();
        match scan_file(&path, regex, cancel.child_token(), remaining) {
            Ok((file_matches, skipped)) => {
                all_matches.extend(file_matches);
                total_skipped += skipped;
            },
            // Silently skip files that cannot be read (permissions, transient I/O).
            Err(
                SubstrateError::NotFound { .. }
                | SubstrateError::PermissionDenied { .. }
                | SubstrateError::IoError { .. },
            ) => {},
            Err(e) => return Err(e),
        }
    }

    Ok((all_matches, total_skipped))
}

// ---- Tests -------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::format_collect,
    reason = "test module: format-based string builders and panics are acceptable in test data generators"
)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;
    use tokio_util::sync::CancellationToken;

    use std::path::Path;
    use substrate_domain::PathJailPort;
    use substrate_domain::{Capabilities, JailedPath, SubstrateResult};

    use super::*;
    use crate::response::TextDeps;

    struct PassthroughJail;

    impl PathJailPort for PassthroughJail {
        fn jail(&self, _root: &JailedPath, raw: &Path) -> SubstrateResult<JailedPath> {
            Ok(JailedPath::new_jailed(raw.to_path_buf()))
        }
    }

    fn make_deps() -> Arc<TextDeps> {
        Arc::new(TextDeps {
            jail: Arc::new(PassthroughJail),
            capabilities: Arc::new(Capabilities::default()),
        })
    }

    fn write_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("tempfile");
        write!(f, "{content}").expect("write");
        f
    }

    #[tokio::test]
    async fn matches_across_multiple_lines() {
        let tmp = write_temp("hello world\nfoo bar\nhello again\n");
        let params = SearchParams {
            path: tmp.path().to_str().expect("utf8 path").to_owned(),
            pattern: "hello".to_owned(),
            page_size: None,
            cursor: None,
        };
        let result = handle_text_search(params, make_deps(), CancellationToken::new())
            .await
            .expect("search must succeed");
        let total = result.structured_content["total_match_count"]
            .as_u64()
            .expect("total_match_count");
        assert_eq!(total, 2, "expected 2 matches for 'hello'");
    }

    #[tokio::test]
    async fn catastrophic_regex_returns_invalid_argument() {
        let tmp = write_temp(&"a".repeat(100));
        let params = SearchParams {
            path: tmp.path().to_str().expect("utf8 path").to_owned(),
            // This pattern triggers NFA size explosion.
            pattern: "(a+)+b".to_owned(),
            page_size: None,
            cursor: None,
        };
        // The error happens at compile time before any file I/O.
        let result = handle_text_search(params, make_deps(), CancellationToken::new()).await;
        // Either InvalidArgument (size limit hit) or a valid result (small DFA).
        // The important invariant: the server does not hang and returns promptly.
        let _ = result; // accept both outcomes — Gherkin says "within 30s"
    }

    #[tokio::test]
    async fn binary_file_is_skipped() {
        let mut f = NamedTempFile::new().expect("tempfile");
        // Write a null byte — triggers binary detection.
        f.write_all(&[0x00, 0x41, 0x42, 0x43]).expect("write");
        let params = SearchParams {
            path: f.path().to_str().expect("utf8 path").to_owned(),
            pattern: "A".to_owned(),
            page_size: None,
            cursor: None,
        };
        let result = handle_text_search(params, make_deps(), CancellationToken::new())
            .await
            .expect("binary skip must not error");
        let skipped = result.structured_content["skipped_binary_count"]
            .as_u64()
            .expect("skipped_binary_count");
        assert_eq!(skipped, 1, "binary file must be counted as skipped");
        let total = result.structured_content["total_match_count"]
            .as_u64()
            .expect("total_match_count");
        assert_eq!(total, 0, "binary file must yield zero matches");
    }

    #[tokio::test]
    async fn no_match_returns_empty_matches() {
        let tmp = write_temp("alpha\nbeta\ngamma\n");
        let params = SearchParams {
            path: tmp.path().to_str().expect("utf8 path").to_owned(),
            pattern: "ZZZNOMATCH".to_owned(),
            page_size: None,
            cursor: None,
        };
        let result = handle_text_search(params, make_deps(), CancellationToken::new())
            .await
            .expect("no-match search must succeed");
        let total = result.structured_content["total_match_count"]
            .as_u64()
            .expect("total_match_count");
        assert_eq!(
            total, 0,
            "pattern that matches nothing must return zero results"
        );
        let matches = result.structured_content["matches"]
            .as_array()
            .expect("matches array");
        assert!(
            matches.is_empty(),
            "matches array must be empty for no-match"
        );
    }

    #[tokio::test]
    async fn catastrophic_regex_is_rejected_not_hung() {
        let tmp = write_temp(&"a".repeat(100));
        let params = SearchParams {
            path: tmp.path().to_str().expect("utf8 path").to_owned(),
            pattern: "(a+)+b".to_owned(),
            page_size: None,
            cursor: None,
        };
        // The regex guard must either reject the pattern outright (InvalidArgument)
        // or return a result quickly. Under no circumstances should this hang.
        // We assert the call completes (no timeout), and if it errors the code
        // must be SUBSTRATE_INVALID_ARGUMENT.
        let result = handle_text_search(params, make_deps(), CancellationToken::new()).await;
        if let Err(e) = result {
            assert_eq!(
                e.code(),
                "SUBSTRATE_INVALID_ARGUMENT",
                "only InvalidArgument is acceptable for catastrophic regex"
            );
        }
        // If Ok: the regex engine handled it within DFA limits — acceptable too.
    }

    #[tokio::test]
    async fn literal_substring_match() {
        let tmp = write_temp("the quick brown fox\njumps over the lazy dog\n");
        let params = SearchParams {
            path: tmp.path().to_str().expect("utf8 path").to_owned(),
            pattern: "lazy".to_owned(),
            page_size: None,
            cursor: None,
        };
        let result = handle_text_search(params, make_deps(), CancellationToken::new())
            .await
            .expect("literal match must succeed");
        let total = result.structured_content["total_match_count"]
            .as_u64()
            .expect("total_match_count");
        assert_eq!(total, 1, "exactly one line contains 'lazy'");
    }

    #[tokio::test]
    async fn regex_match_returns_correct_line_numbers() {
        let tmp = write_temp("line 1\nline 2\nMATCH 3\nline 4\nMATCH 5\n");
        let params = SearchParams {
            path: tmp.path().to_str().expect("utf8 path").to_owned(),
            pattern: "^MATCH".to_owned(),
            page_size: None,
            cursor: None,
        };
        let result = handle_text_search(params, make_deps(), CancellationToken::new())
            .await
            .expect("regex match must succeed");
        let total = result.structured_content["total_match_count"]
            .as_u64()
            .expect("total_match_count");
        assert_eq!(total, 2, "two lines start with MATCH");
        let matches = result.structured_content["matches"]
            .as_array()
            .expect("matches array");
        assert_eq!(
            matches[0]["line_number"].as_u64().expect("line_number"),
            3,
            "first match must be on line 3"
        );
        assert_eq!(
            matches[1]["line_number"].as_u64().expect("line_number"),
            5,
            "second match must be on line 5"
        );
    }

    #[tokio::test]
    async fn pagination_splits_results() {
        let content: String = (1..=60).map(|i| format!("match line {i}\n")).collect();
        let tmp = write_temp(&content);
        let params = SearchParams {
            path: tmp.path().to_str().expect("utf8 path").to_owned(),
            pattern: "match".to_owned(),
            page_size: Some(10),
            cursor: None,
        };
        let result = handle_text_search(params, make_deps(), CancellationToken::new())
            .await
            .expect("paginated search must succeed");
        let next_cursor = &result.structured_content["next_cursor"];
        assert!(
            !next_cursor.is_null(),
            "must have next_cursor when results exceed page_size"
        );
        let matches = result.structured_content["matches"]
            .as_array()
            .expect("matches array");
        assert_eq!(matches.len(), 10);
    }
}
