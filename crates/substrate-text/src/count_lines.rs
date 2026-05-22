//! `text.count_lines` — line and byte count via SIMD popcount (Zone B).
//!
//! # Zone classification (ADR-0003)
//!
//! Zone B: the file is read into memory via `tokio::fs::read` (for small files
//! under [`SMALL_FILE_THRESHOLD`]) or streamed in chunks via a blocking
//! `std::io::BufReader` (for large files). Both paths execute inside
//! `tokio::task::spawn_blocking`.
//!
//! # SIMD acceleration (ADR-0043)
//!
//! `bytecount::count(&buf, b'\n')` uses SIMD popcount (AVX2 on x86-64,
//! NEON on aarch64) to count newlines without an explicit scalar loop.
//! The `bytecount` crate selects the backend via compile-time feature
//! detection with a portable scalar fallback.

use std::io::Read as _;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{SubstrateError, SubstrateResult};

use crate::hints_helpers;
use crate::response::{TextDeps, ToolResponse};

/// Files at or below this byte size are fully read into memory before counting.
const SMALL_FILE_THRESHOLD: u64 = 64 * 1024; // 64 KiB

/// Chunk size used when streaming large files through the blocking reader.
const CHUNK_SIZE: usize = 64 * 1024; // 64 KiB per read

/// Input parameters for `text.count_lines`.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct CountLinesParams {
    /// Absolute path to the file to measure.
    pub path: String,
}

/// Handles a `text.count_lines` tool call.
///
/// Returns a [`ToolResponse`] with `line_count` and `byte_count` in
/// `structured_content`.
///
/// # Errors
///
/// - `SUBSTRATE_NOT_FOUND` — the target file does not exist.
/// - `SUBSTRATE_PATH_OUTSIDE_ALLOWLIST` — path fails allowlist check.
/// - `SUBSTRATE_IO_ERROR` — kernel I/O failure during read.
#[instrument(skip(deps, cancel), fields(path = %params.path))]
pub async fn handle_text_count_lines(
    params: CountLinesParams,
    deps: Arc<TextDeps>,
    cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    let raw_path = std::path::PathBuf::from(&params.path);

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

    let jailed_path_buf = jailed.into_inner();
    let simd_tier = deps.capabilities.simd_tier;

    let (line_count, byte_count) = tokio::task::spawn_blocking({
        let cancel = cancel.clone();
        move || count_lines_blocking(&jailed_path_buf, cancel)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error during count: {e}"),
        correlation_id: None,
    })??;

    let content = format!(
        "text.count_lines: '{}' has {line_count} line(s) and {byte_count} byte(s).",
        params.path
    );

    let structured_content = serde_json::json!({
        "path": params.path,
        "line_count": line_count,
        "byte_count": byte_count,
    });

    let hints = hints_helpers::build_count_lines_hints(simd_tier);
    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

/// Blocking line and byte counter.
///
/// Uses `bytecount::count` (SIMD popcount) for newline counting and
/// tracks the byte count as a side effect of reading.
#[expect(
    clippy::needless_pass_by_value,
    reason = "CancellationToken is an Arc-backed handle; pass by value matches the tokio-util API convention"
)]
fn count_lines_blocking(
    path: &std::path::Path,
    cancel: CancellationToken,
) -> SubstrateResult<(u64, u64)> {
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

    let metadata = file.metadata().map_err(|_| SubstrateError::IoError {
        path: path.display().to_string(),
        correlation_id: None,
    })?;

    if metadata.len() <= SMALL_FILE_THRESHOLD {
        // Small-file fast path: read entire file then count.
        let buf = std::fs::read(path).map_err(|_| SubstrateError::IoError {
            path: path.display().to_string(),
            correlation_id: None,
        })?;
        let line_count = bytecount::count(&buf, b'\n') as u64;
        let byte_count = buf.len() as u64;
        return Ok((line_count, byte_count));
    }

    // Large-file streaming path.
    let mut reader = std::io::BufReader::with_capacity(CHUNK_SIZE, file);
    let mut chunk = vec![0u8; CHUNK_SIZE];
    let mut line_count: u64 = 0;
    let mut byte_count: u64 = 0;
    let mut chunks_read: u64 = 0;

    loop {
        if cancel.is_cancelled() {
            return Err(SubstrateError::Cancelled {
                correlation_id: None,
            });
        }

        let n = reader
            .read(&mut chunk)
            .map_err(|_| SubstrateError::IoError {
                path: path.display().to_string(),
                correlation_id: None,
            })?;

        if n == 0 {
            break;
        }

        // SIMD popcount via bytecount (ADR-0043).
        line_count += bytecount::count(&chunk[..n], b'\n') as u64;
        byte_count += n as u64;
        chunks_read += 1;

        // Check cancellation every 16 chunks (~1 MiB with 64 KiB chunks).
        if chunks_read.is_multiple_of(16) && cancel.is_cancelled() {
            return Err(SubstrateError::Cancelled {
                correlation_id: None,
            });
        }
    }

    Ok((line_count, byte_count))
}

// ---- Tests -------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::naive_bytecount,
    reason = "test module: naive bytecount is the intentional scalar baseline for comparison; panics and casts are acceptable"
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
    async fn counts_lines_correctly() {
        let tmp = write_temp("line1\nline2\nline3\n");
        let params = CountLinesParams {
            path: tmp.path().to_str().expect("utf8").to_owned(),
        };
        let result = handle_text_count_lines(params, make_deps(), CancellationToken::new())
            .await
            .expect("count must succeed");
        assert_eq!(result.structured_content["line_count"].as_u64(), Some(3));
    }

    #[tokio::test]
    async fn byte_count_matches_content_length() {
        let content = "hello\nworld\n";
        let tmp = write_temp(content);
        let params = CountLinesParams {
            path: tmp.path().to_str().expect("utf8").to_owned(),
        };
        let result = handle_text_count_lines(params, make_deps(), CancellationToken::new())
            .await
            .expect("count must succeed");
        assert_eq!(
            result.structured_content["byte_count"].as_u64(),
            Some(content.len() as u64)
        );
    }

    #[tokio::test]
    async fn empty_file_returns_zero_lines() {
        let tmp = write_temp("");
        let params = CountLinesParams {
            path: tmp.path().to_str().expect("utf8").to_owned(),
        };
        let result = handle_text_count_lines(params, make_deps(), CancellationToken::new())
            .await
            .expect("empty file count must succeed");
        assert_eq!(result.structured_content["line_count"].as_u64(), Some(0));
        assert_eq!(result.structured_content["byte_count"].as_u64(), Some(0));
    }

    /// Verifies that `bytecount::count` matches a naive scalar count on known data.
    #[test]
    fn bytecount_matches_scalar_newline_count() {
        let data = b"foo\nbar\nbaz\n";
        let simd_count = bytecount::count(data, b'\n');
        let scalar_count = data.iter().filter(|&&b| b == b'\n').count();
        assert_eq!(simd_count, scalar_count);
    }
}
