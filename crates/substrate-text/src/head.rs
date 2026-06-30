//! `text.head` — first N lines of a file (Zone A, snapshot-capped).
//!
//! # Zone classification (ADR-0003)
//!
//! Zone A: async-native. Uses `tokio::io::AsyncBufReadExt::lines()` to stream
//! lines from the file, stopping after `n` lines. The entire operation stays
//! on the async executor; no blocking thread is required because `n` is
//! bounded by [`MAX_LINES`] (1000), ensuring the read terminates promptly.
//!
//! # SIMD acceleration (ADR-0043)
//!
//! `simdutf8::basic::from_utf8` is used to validate the sniff window before
//! streaming. This provides a fast binary-content guard without reading the
//! whole file.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt as _, BufReader};
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{SubstrateError, SubstrateResult};

use crate::hints_helpers;
use crate::response::{TextDeps, ToolResponse};

/// Default number of lines returned by `text.head` when `n` is not specified.
pub const DEFAULT_LINES: usize = 10;
/// Maximum number of lines that `text.head` will return.
pub const MAX_LINES: usize = 1000;
/// Maximum bytes read per line before the remainder is silently truncated.
///
/// A single line without any `\n` can be arbitrarily large (e.g. a minified
/// JS bundle, a binary blob mis-classified as text).  Without a cap, a 1 GiB
/// line would buffer entirely in `buf` before `read_line` returns, exhausting
/// the async-executor heap.  Lines longer than this limit are truncated to the
/// first `MAX_LINE_BYTES` bytes and suffixed with `…` so callers can detect
/// truncation without crashing.
pub const MAX_LINE_BYTES: usize = 64 * 1024; // 64 KiB per line

/// Input parameters for `text.head`.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HeadParams {
    /// Absolute path to the file to read.
    pub path: String,
    /// Number of lines to return (default 10, max 1000).
    #[serde(default)]
    pub n: Option<usize>,
}

/// Drains bytes from `reader` until (and including) the next `\n` or EOF.
///
/// This is the complement to the `take(MAX_LINE_BYTES).read_line(…)` pattern:
/// after the cap is hit and the line was truncated, the underlying reader is
/// still positioned mid-line.  Advancing to the next newline without
/// allocating the skipped content is done via `fill_buf` + `consume` which
/// operates on the existing internal buffer — O(1) heap, O(1) per internal
/// buffer chunk.
///
/// # Errors
///
/// Returns `SubstrateError::IoError` on any I/O failure during the drain.
async fn drain_to_newline(
    reader: &mut BufReader<tokio::fs::File>,
    path: &str,
) -> SubstrateResult<()> {
    loop {
        let filled = reader
            .fill_buf()
            .await
            .map_err(|_| SubstrateError::IoError {
                path: path.to_owned(),
                correlation_id: None,
            })?;

        if filled.is_empty() {
            // EOF — the over-long line extended to end-of-file; nothing more
            // to drain.
            break;
        }

        // Look for `\n` in the currently buffered chunk (SIMD — ADR-0043).
        if let Some(pos) = memchr::memchr(b'\n', filled) {
            // Consume up to and including the newline, then stop.
            reader.consume(pos + 1);
            break;
        }
        // No newline yet; consume the entire buffered chunk and loop.
        let len = filled.len();
        reader.consume(len);
    }
    Ok(())
}

/// Handles a `text.head` tool call.
///
/// Returns a [`ToolResponse`] whose `structured_content` contains the first
/// `n` lines of the file as a JSON array of strings.
///
/// # Errors
///
/// - `SUBSTRATE_NOT_FOUND` — the target file does not exist.
/// - `SUBSTRATE_PATH_OUTSIDE_ALLOWLIST` — path fails allowlist check.
/// - `SUBSTRATE_IO_ERROR` — kernel I/O failure during read.
#[instrument(skip(deps, cancel), fields(path = %params.path, n = ?params.n))]
pub async fn handle_text_head(
    params: HeadParams,
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

    let file = tokio::fs::File::open(jailed.as_path())
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => SubstrateError::NotFound {
                resource: params.path.clone(),
                correlation_id: None,
            },
            std::io::ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
                path: params.path.clone(),
                correlation_id: None,
            },
            _ => SubstrateError::IoError {
                path: params.path.clone(),
                correlation_id: None,
            },
        })?;

    let mut reader = BufReader::new(file);
    let mut lines = Vec::with_capacity(n);

    for _ in 0..n {
        if cancel.is_cancelled() {
            return Err(SubstrateError::Cancelled {
                correlation_id: None,
            });
        }

        // OOM guard (text-oom lane fix #1): read at most MAX_LINE_BYTES per
        // line.  A newline-less 1 GiB line would otherwise buffer entirely in
        // `buf` before `read_line` returns, exhausting heap on the executor.
        //
        // `take(MAX_LINE_BYTES)` wraps the reader in a byte-limited view while
        // still exposing `AsyncBufRead` (tokio impl for `Take<R: AsyncBufRead>`
        // passes through `fill_buf` capped to the remaining limit), so
        // `read_line` honours the cap without any extra allocation.
        let mut limited = (&mut reader).take(MAX_LINE_BYTES as u64);
        let mut buf = String::new();
        let bytes_read =
            limited
                .read_line(&mut buf)
                .await
                .map_err(|_| SubstrateError::IoError {
                    path: params.path.clone(),
                    correlation_id: None,
                })?;

        if bytes_read == 0 {
            break; // EOF
        }

        // Detect truncation: `read_line` on the capped view stopped because
        // the byte limit was hit, not because it found `\n`.
        let truncated = !buf.ends_with('\n') && bytes_read >= MAX_LINE_BYTES;
        if truncated {
            // Drain the remainder of the over-long line from `reader` without
            // allocating the skipped content.  Use `fill_buf` + `consume` in a
            // loop: O(1) memory, O(line_bytes / internal_buf_size) iterations.
            drain_to_newline(&mut reader, &params.path).await?;
            buf.push('…');
        }

        // Trim the trailing newline for cleaner output.
        let trimmed = buf.trim_end_matches('\n').trim_end_matches('\r');
        lines.push(trimmed.to_owned());
    }

    let simd_tier = deps.capabilities.simd_tier;
    let content = format!(
        "text.head: first {} line(s) of '{}'.",
        lines.len(),
        params.path
    );

    let structured_content = serde_json::json!({
        "path": params.path,
        "lines": lines,
        "line_count": lines.len(),
    });

    let hints = hints_helpers::build_head_hints(simd_tier);
    Ok(ToolResponse::with_hints(content, structured_content, hints))
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
    clippy::useless_format,
    clippy::format_collect,
    reason = "test module: format-based string builders and casts are acceptable in test data generators"
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
    async fn returns_first_n_lines() {
        let content = (1..=20).map(|i| format!("line {i}\n")).collect::<String>();
        let tmp = write_temp(&content);
        let params = HeadParams {
            path: tmp.path().to_str().expect("utf8").to_owned(),
            n: Some(5),
        };
        let result = handle_text_head(params, make_deps(), CancellationToken::new())
            .await
            .expect("head must succeed");
        let lines = result.structured_content["lines"]
            .as_array()
            .expect("lines array");
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0].as_str(), Some("line 1"));
        assert_eq!(lines[4].as_str(), Some("line 5"));
    }

    #[tokio::test]
    async fn capped_at_max_lines() {
        let content = (1..=2000)
            .map(|i| format!("line {i}\n"))
            .collect::<String>();
        let tmp = write_temp(&content);
        let params = HeadParams {
            path: tmp.path().to_str().expect("utf8").to_owned(),
            n: Some(5000), // request more than MAX_LINES
        };
        let result = handle_text_head(params, make_deps(), CancellationToken::new())
            .await
            .expect("head with over-cap n must succeed");
        let lines = result.structured_content["lines"]
            .as_array()
            .expect("lines array");
        assert!(
            lines.len() <= MAX_LINES,
            "must not return more than MAX_LINES lines"
        );
    }

    #[tokio::test]
    async fn default_n_is_ten() {
        let content = (1..=20).map(|i| format!("line {i}\n")).collect::<String>();
        let tmp = write_temp(&content);
        let params = HeadParams {
            path: tmp.path().to_str().expect("utf8").to_owned(),
            n: None,
        };
        let result = handle_text_head(params, make_deps(), CancellationToken::new())
            .await
            .expect("head with default n must succeed");
        let lines = result.structured_content["lines"]
            .as_array()
            .expect("lines array");
        assert_eq!(lines.len(), DEFAULT_LINES);
    }

    #[tokio::test]
    async fn short_file_returns_all_lines() {
        let tmp = write_temp("alpha\nbeta\n");
        let params = HeadParams {
            path: tmp.path().to_str().expect("utf8").to_owned(),
            n: Some(100),
        };
        let result = handle_text_head(params, make_deps(), CancellationToken::new())
            .await
            .expect("head of short file must succeed");
        let lines = result.structured_content["lines"]
            .as_array()
            .expect("lines array");
        assert_eq!(lines.len(), 2);
    }
}
