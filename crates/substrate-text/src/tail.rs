//! `text.tail` — last N lines of a file (Zone A for small files, Zone B for large).
//!
//! # Zone classification (ADR-0003)
//!
//! - **Small files (≤ 64 KiB)**: Zone A fast path. The entire file is read
//!   asynchronously via `tokio::fs::read`, then split into lines in memory.
//!   The `memchr::memchr_iter` SIMD iterator locates newlines during the
//!   final slice selection.
//!
//! - **Large files (> 64 KiB)**: Zone B. `tokio::task::spawn_blocking` wraps
//!   a seek-from-end strategy that scans backwards through fixed-size chunks
//!   using `memchr::memchr_iter` (SIMD) to find newline positions.
//!
//! # SIMD acceleration (ADR-0043)
//!
//! `memchr::memchr` and `memchr::memchr_iter` on x86-64 (AVX2/SSE2) and
//! aarch64 (NEON) — activated transparently by the `memchr` crate at runtime.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{SubstrateError, SubstrateResult};

use crate::hints_helpers;
use crate::response::{TextDeps, ToolResponse};

/// Default number of lines returned by `text.tail` when `n` is not specified.
pub const DEFAULT_LINES: usize = 10;
/// Maximum number of lines that `text.tail` will return.
pub const MAX_LINES: usize = 1000;
/// Files at or below this size use the small-file Zone A path.
const SMALL_FILE_THRESHOLD: u64 = 64 * 1024; // 64 KiB
/// Read chunk size for the reverse-scan Zone B path.
const CHUNK_SIZE: usize = 8 * 1024; // 8 KiB

/// Input parameters for `text.tail`.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct TailParams {
    /// Absolute path to the file to read.
    pub path: String,
    /// Number of lines to return from the end (default 10, max 1000).
    #[serde(default)]
    pub n: Option<usize>,
}

/// Handles a `text.tail` tool call.
///
/// Returns a [`ToolResponse`] whose `structured_content` contains the last
/// `n` lines of the file as a JSON array of strings.
///
/// # Errors
///
/// - `SUBSTRATE_NOT_FOUND` — the target file does not exist.
/// - `SUBSTRATE_PATH_OUTSIDE_ALLOWLIST` — path fails allowlist check.
/// - `SUBSTRATE_IO_ERROR` — kernel I/O failure during read.
#[instrument(skip(deps, cancel), fields(path = %params.path, n = ?params.n))]
pub async fn handle_text_tail(
    params: TailParams,
    deps: Arc<TextDeps>,
    cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    let n = params.n.unwrap_or(DEFAULT_LINES).min(MAX_LINES);
    let raw_path = PathBuf::from(&params.path);

    let jailed = {
        let jail = Arc::clone(&deps.jail);
        let raw = raw_path.clone();
        tokio::task::spawn_blocking(move || {
            jail.jail(
                &substrate_domain::JailedPath::new_jailed(
                    raw.parent().unwrap_or(&raw).to_path_buf(),
                ),
                &raw,
            )
        })
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error: {e}"),
            correlation_id: None,
        })??
    };

    let jailed_path = jailed.into_inner();
    let simd_tier = deps.capabilities.simd_tier;

    // Stat the file to decide between Zone A and Zone B.
    let metadata = tokio::fs::metadata(&jailed_path)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => SubstrateError::NotFound {
                resource: params.path.clone(),
                correlation_id: None,
            },
            _ => SubstrateError::IoError {
                path: params.path.clone(),
                correlation_id: None,
            },
        })?;

    let lines = if metadata.len() <= SMALL_FILE_THRESHOLD {
        // Zone A fast path: read entire file async, then extract last N lines.
        let bytes = tokio::fs::read(&jailed_path)
            .await
            .map_err(|_| SubstrateError::IoError {
                path: params.path.clone(),
                correlation_id: None,
            })?;

        last_n_lines_from_bytes(&bytes, n)
    } else {
        // Zone B: seek-from-end reverse scan in a blocking thread.
        let path_clone = jailed_path.clone();
        let path_str = params.path.clone();
        tokio::task::spawn_blocking({
            let cancel = cancel.clone();
            move || tail_blocking(&path_clone, &path_str, n, cancel)
        })
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error during tail: {e}"),
            correlation_id: None,
        })??
    };

    let content = format!(
        "text.tail: last {} line(s) of '{}'.",
        lines.len(),
        params.path
    );

    let structured_content = serde_json::json!({
        "path": params.path,
        "lines": lines,
        "line_count": lines.len(),
    });

    let hints = hints_helpers::build_tail_hints(simd_tier);
    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

/// Extracts the last `n` lines from a byte buffer.
///
/// Uses `memchr::memchr_iter` (SIMD) to locate `\n` boundaries, then
/// takes the last `n` segments. Strips trailing `\r\n`.
fn last_n_lines_from_bytes(buf: &[u8], n: usize) -> Vec<String> {
    if buf.is_empty() {
        return vec![];
    }

    // Collect newline positions via SIMD memchr_iter (ADR-0043).
    // Positions are the indices of `\n` bytes.
    let newline_positions: Vec<usize> = memchr::memchr_iter(b'\n', buf).collect();

    // Build line byte ranges: each line is the span between consecutive `\n` bytes.
    let total_lines = newline_positions.len();
    let skip = total_lines.saturating_sub(n);

    let mut lines = Vec::with_capacity(n.min(total_lines));

    // Determine the start of the first line we care about.
    let start_byte = if skip == 0 {
        0
    } else {
        // The line after newline at index `skip - 1`.
        newline_positions[skip - 1] + 1
    };

    let slice = &buf[start_byte..];

    for line_bytes in slice.split(|&b| b == b'\n') {
        // Skip empty trailing segment after a final newline.
        if line_bytes.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(line_bytes);
        let trimmed = line.trim_end_matches('\r');
        lines.push(trimmed.to_owned());
    }

    lines
}

/// Blocking seek-from-end tail for large files.
///
/// Reads backwards in [`CHUNK_SIZE`] chunks, scanning for `\n` via
/// `memchr::memchr` (SIMD), until `n` line boundaries are found.
#[expect(
    clippy::needless_pass_by_value,
    reason = "CancellationToken is an Arc-backed handle; pass by value matches the tokio-util API convention"
)]
fn tail_blocking(
    path: &std::path::Path,
    path_str: &str,
    n: usize,
    cancel: CancellationToken,
) -> SubstrateResult<Vec<String>> {
    let mut file = std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => SubstrateError::NotFound {
            resource: path_str.to_owned(),
            correlation_id: None,
        },
        std::io::ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
            path: path_str.to_owned(),
            correlation_id: None,
        },
        _ => SubstrateError::IoError {
            path: path_str.to_owned(),
            correlation_id: None,
        },
    })?;

    let file_size = file
        .seek(SeekFrom::End(0))
        .map_err(|_| SubstrateError::IoError {
            path: path_str.to_owned(),
            correlation_id: None,
        })?;

    if file_size == 0 {
        return Ok(vec![]);
    }

    // Collect enough bytes from the end to hold `n` lines.
    // Strategy: read backwards in CHUNK_SIZE blocks, accumulating content,
    // until we have more than `n` newlines.
    let mut collected: Vec<u8> = Vec::new();
    let mut remaining = file_size;
    let mut newline_count = 0usize;
    let mut chunk_idx = 0u64;

    while remaining > 0 && newline_count <= n {
        if cancel.is_cancelled() {
            return Err(SubstrateError::Cancelled {
                correlation_id: None,
            });
        }

        #[expect(
            clippy::cast_possible_truncation,
            reason = "CHUNK_SIZE is 64 KiB; remaining is bounded by file size; truncation cannot occur for files that fit in memory on any realistic target"
        )]
        let to_read = CHUNK_SIZE.min(remaining as usize);
        remaining -= to_read as u64;

        file.seek(SeekFrom::Start(remaining))
            .map_err(|_| SubstrateError::IoError {
                path: path_str.to_owned(),
                correlation_id: None,
            })?;

        let mut chunk = vec![0u8; to_read];
        file.read_exact(&mut chunk)
            .map_err(|_| SubstrateError::IoError {
                path: path_str.to_owned(),
                correlation_id: None,
            })?;

        // Count newlines in this chunk via SIMD memchr (ADR-0043).
        newline_count += memchr::memchr_iter(b'\n', &chunk).count();

        // Prepend chunk to accumulated buffer (we're scanning backwards).
        chunk.extend_from_slice(&collected);
        collected = chunk;
        chunk_idx += 1;

        // Check cancel every 4 chunks.
        if chunk_idx.is_multiple_of(4) && cancel.is_cancelled() {
            return Err(SubstrateError::Cancelled {
                correlation_id: None,
            });
        }
    }

    Ok(last_n_lines_from_bytes(&collected, n))
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
    async fn returns_last_n_lines_small_file() {
        let content = (1..=10).map(|i| format!("line {i}\n")).collect::<String>();
        let tmp = write_temp(&content);
        let params = TailParams {
            path: tmp.path().to_str().expect("utf8").to_owned(),
            n: Some(3),
        };
        let result = handle_text_tail(params, make_deps(), CancellationToken::new())
            .await
            .expect("tail must succeed");
        let lines = result.structured_content["lines"]
            .as_array()
            .expect("lines array");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2].as_str(), Some("line 10"));
    }

    #[tokio::test]
    async fn default_n_is_ten_small_file() {
        let content = (1..=15).map(|i| format!("line {i}\n")).collect::<String>();
        let tmp = write_temp(&content);
        let params = TailParams {
            path: tmp.path().to_str().expect("utf8").to_owned(),
            n: None,
        };
        let result = handle_text_tail(params, make_deps(), CancellationToken::new())
            .await
            .expect("tail default n must succeed");
        let lines = result.structured_content["lines"]
            .as_array()
            .expect("lines array");
        assert_eq!(lines.len(), DEFAULT_LINES);
        assert_eq!(lines[9].as_str(), Some("line 15"));
    }

    /// Verifies that `last_n_lines_from_bytes` extracts the correct lines from
    /// a buffer — the Zone A fast path used for files ≤ 64 KiB.
    #[test]
    fn last_n_lines_correct_for_small_buffer() {
        let buf = b"alpha\nbeta\ngamma\ndelta\n";
        let lines = last_n_lines_from_bytes(buf, 2);
        assert_eq!(lines, vec!["gamma", "delta"]);
    }

    // Proptest: for any file content and any n, the head+tail counts must
    // together be <= 2*n and <= total lines (they may overlap on short files).
    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(30))]
        #[test]
        fn head_and_tail_line_counts_are_bounded(
            lines_count in 0usize..40,
            n in 1usize..25,
        ) {
            use std::io::Write;
            let content: String = (0..lines_count).map(|i| format!("line{i}\n")).collect();
            let mut f = NamedTempFile::new().expect("tempfile");
            write!(f, "{content}").expect("write");

            let buf = std::fs::read(f.path()).expect("read");
            let head_lines = {
                // Simulate Zone A head: first n lines.
                let text = std::str::from_utf8(&buf).expect("utf8");
                text.lines().count().min(n)
            };
            let tail_lines = last_n_lines_from_bytes(&buf, n).len();

            proptest::prop_assert!(
                head_lines <= n,
                "head must return at most n lines (got {head_lines})"
            );
            proptest::prop_assert!(
                tail_lines <= n,
                "tail must return at most n lines (got {tail_lines})"
            );
            // Both head and tail are subsets of the full file lines.
            proptest::prop_assert!(
                head_lines <= lines_count,
                "head count must not exceed file line count"
            );
            proptest::prop_assert!(
                tail_lines <= lines_count,
                "tail count must not exceed file line count"
            );
        }
    }

    /// Verifies that the large-file Zone B blocking tail path produces the same
    /// result as the Zone A path for content that fits in memory.
    #[tokio::test]
    async fn large_file_path_matches_small_file_path() {
        // Write 200 short lines — well under 64 KiB so both paths work,
        // but we force the large-file path by calling tail_blocking directly.
        let content: String = (1..=200).map(|i| format!("item {i}\n")).collect();
        let tmp = write_temp(&content);

        // Zone A result.
        let buf = std::fs::read(tmp.path()).expect("read");
        let zone_a: Vec<String> = last_n_lines_from_bytes(&buf, 5);

        // Zone B result.
        let zone_b = tail_blocking(
            tmp.path(),
            tmp.path().to_str().expect("utf8"),
            5,
            CancellationToken::new(),
        )
        .expect("blocking tail must succeed");

        assert_eq!(
            zone_a, zone_b,
            "Zone A and Zone B must produce identical results"
        );
    }
}
