//! Handler for the `fs.read` tool.
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: read file content as UTF-8 text or base64 bytes with optional byte range
//! DOES: returns file content inline for files ≤ 1 MiB; larger files return PAYLOAD_TOO_LARGE
//! ARGS: path (string) — file to read;
//!       encoding (string, "text") — "text" | "base64";
//!       offset_bytes (u64, 0) — byte range start;
//!       length_bytes (u64, null) — byte range length; null reads to EOF
//! RETURNS: {path, encoding, content, size_bytes}
//! NEXT: fs.stat, fs.hash
//! AVOID: reading large binary files as text → use encoding:"base64"
//! ```
//!
//! # Zone classification
//!
//! Files ≤ `INLINE_MAX_BYTES` use Zone A (`tokio::fs::read`).
//! Files > `INLINE_MAX_BYTES` return `SUBSTRATE_PAYLOAD_TOO_LARGE` immediately.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{JailedPath, PathJailPort, SubstrateError, SubstrateResult};

use crate::hint_helpers::build_hints;
use crate::response::{FsQueryDeps, ToolResponse};

/// Maximum bytes returned inline (1 MiB).
const INLINE_MAX_BYTES: u64 = 1_048_576;

/// Encoding requested by the caller.
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReadEncoding {
    /// Validate as UTF-8 and return as a JSON string.
    #[default]
    Text,
    /// Encode as standard base64 and return as a JSON string.
    Base64,
}

/// Inbound request for `fs.read`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsReadRequest {
    /// Path to the file to read; must be within an allowlist root.
    pub path: String,

    /// Encoding for the returned content.
    #[serde(default)]
    pub encoding: ReadEncoding,

    /// Start offset in bytes (inclusive).
    #[serde(default)]
    pub offset_bytes: u64,

    /// Number of bytes to read; `None` reads to EOF.
    pub length_bytes: Option<u64>,
}

/// Handler for `fs.read`.
///
/// Zone A for files within the inline limit; returns an error for larger files.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, metadata I/O,
/// encoding errors, or when the requested byte range exceeds the inline limit.
#[expect(
    clippy::too_many_lines,
    reason = "handle_fs_read orchestrates jail, special-file detection, range slicing, encoding, and hints in one cohesive Zone-A handler"
)]
#[instrument(skip(deps, _cancel), fields(path = %req.path))]
pub async fn handle_fs_read(
    req: FsReadRequest,
    deps: &FsQueryDeps,
    _cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    // Pre-jail file-type guard: stat the raw path with lstat to detect special
    // files (FIFOs, sockets, devices) before the jail tries to open them.
    // Some jail backends fail with ENXIO/EOPNOTSUPP when opening Unix sockets
    // via openat2/O_NOFOLLOW_ANY, which surfaces as a SUBSTRATE_IO_ERROR rather
    // than the more accurate SUBSTRATE_INVALID_ARGUMENT.
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt as _;
        let raw_path = std::path::Path::new(&req.path);
        if let Ok(pre_meta) = std::fs::symlink_metadata(raw_path) {
            let pre_ft = pre_meta.file_type();
            if !pre_ft.is_file() && !pre_ft.is_dir() && !pre_ft.is_symlink() {
                let file_type_str = if pre_ft.is_fifo() {
                    "fifo"
                } else if pre_ft.is_socket() {
                    "socket"
                } else if pre_ft.is_block_device() {
                    "block device"
                } else if pre_ft.is_char_device() {
                    "char device"
                } else {
                    "special"
                };
                return Err(SubstrateError::InvalidArgument {
                    offending_field: "path".to_owned(),
                    reason: format!(
                        "path does not point to a regular file; target is a {file_type_str}. regular files only"
                    ),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                });
            }
        }
    }

    // Jail the path.
    let raw = std::path::Path::new(&req.path).to_path_buf();
    let jail: Arc<dyn PathJailPort> = Arc::clone(&deps.jail);
    let raw_clone = raw.clone();
    let jailed: JailedPath = tokio::task::spawn_blocking(move || {
        jail.jail(&JailedPath::new_jailed(raw_clone.clone()), &raw_clone)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error: {e}"),
        correlation_id: None,
    })??;

    // Use symlink_metadata (lstat semantics) so that FIFOs and sockets are
    // detected without following them — following a FIFO blocks indefinitely.
    let meta = tokio::fs::symlink_metadata(jailed.as_path())
        .await
        .map_err(|e| map_io_err(e, &req.path))?;

    // Reject special files (FIFO, socket, device) before attempting to open.
    let ft = meta.file_type();
    if ft.is_symlink() || !ft.is_file() {
        let file_type_str = classify_file_type(ft);
        return Err(SubstrateError::InvalidArgument {
            offending_field: "path".to_owned(),
            reason: format!(
                "path does not point to a regular file; target is a {file_type_str}. \
                 regular files only"
            ),
            correlation_id: Some(uuid::Uuid::now_v7()),
        });
    }

    let file_size = meta.len();

    // Calculate the actual slice to read.
    let start = req.offset_bytes.min(file_size);
    let max_readable = file_size.saturating_sub(start);
    let read_len = req.length_bytes.unwrap_or(max_readable).min(max_readable);

    if read_len > INLINE_MAX_BYTES {
        return Err(SubstrateError::ResourceLimit {
            detail: format!(
                "requested {read_len} bytes exceeds inline_max_bytes {INLINE_MAX_BYTES}; \
                 use a byte range (offset_bytes + length_bytes) to read in chunks"
            ),
            correlation_id: None,
        });
    }

    // Zone A: read the slice with tokio::fs.
    let bytes: Vec<u8> = if start == 0 && req.length_bytes.is_none() {
        // Fast path: read entire file.
        tokio::fs::read(jailed.as_path())
            .await
            .map_err(|e| map_io_err(e, &req.path))?
    } else {
        use tokio::io::AsyncReadExt as _;
        use tokio::io::AsyncSeekExt as _;
        let mut f = tokio::fs::File::open(jailed.as_path())
            .await
            .map_err(|e| map_io_err(e, &req.path))?;
        f.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| map_io_err(e, &req.path))?;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "read_len is bounded by INLINE_MAX_BYTES (1 MiB) which fits in usize on all supported targets"
        )]
        let mut buf = vec![0u8; read_len as usize];
        // `read_exact` fills the entire buffer or returns an error; the byte
        // count it yields is always `buf.len()`, so no truncation is required.
        f.read_exact(&mut buf)
            .await
            .map_err(|e| map_io_err(e, &req.path))?;
        buf
    };

    let actual_size = bytes.len() as u64;

    // Encode the content.
    let (encoded_content, used_encoding) = match req.encoding {
        ReadEncoding::Text => {
            // SIMD-accelerated UTF-8 validation (simdutf8).
            simdutf8::basic::from_utf8(&bytes).map_err(|_| SubstrateError::EncodingError {
                detail: format!("file at '{}' is not valid UTF-8", req.path),
                correlation_id: None,
            })?;
            // Safe: simdutf8 confirmed the bytes are valid UTF-8.
            let s = std::str::from_utf8(&bytes)
                .map_err(|_| SubstrateError::EncodingError {
                    detail: format!("file at '{}' is not valid UTF-8", req.path),
                    correlation_id: None,
                })?
                .to_owned();
            (s, "text")
        },
        ReadEncoding::Base64 => {
            let s = base64_simd::STANDARD.encode_to_string(bytes.as_slice());
            (s, "base64")
        },
    };

    let hints = build_hints(
        Some("fs.stat"),
        Some("fs.hash"),
        Some("Use offset_bytes+length_bytes for files >1 MiB"),
        &deps.capabilities,
        false,
    );

    // Include a short content preview in the narrative text so that
    // model-facing `content[0].text` carries the actual data for small files
    // (≤ 256 bytes). The structured content always carries the full payload.
    let preview_snippet = if used_encoding == "text" && actual_size <= 256 {
        format!(" | content: {encoded_content}")
    } else {
        String::new()
    };
    let content = format!(
        "USE: read file content\nDOES: returned {actual_size} bytes ({used_encoding}){preview_snippet}\nNEXT: fs.stat, fs.hash\nAVOID: reading binary as text → use encoding:base64"
    );

    let structured_content = json!({
        "tool": "fs.read",
        "path": req.path,
        "encoding": used_encoding,
        "content": encoded_content,
        "size_bytes": actual_size,
        "hints": hints,
    });

    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

/// Returns a human-readable name for a file type.
///
/// On Unix, distinguishes FIFOs and sockets via [`FileTypeExt`].
/// On non-Unix, only the standard `is_dir` / `is_symlink` variants are available.
fn classify_file_type(ft: std::fs::FileType) -> &'static str {
    if ft.is_dir() {
        return "directory";
    }
    if ft.is_symlink() {
        return "symlink";
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt as _;
        if ft.is_fifo() {
            return "fifo";
        }
        if ft.is_socket() {
            return "socket";
        }
        if ft.is_block_device() {
            return "block device";
        }
        if ft.is_char_device() {
            return "char device";
        }
    }
    "special"
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "std::io::Error is the conventional error-mapping pattern; taking by value avoids lifetime annotation at call sites"
)]
fn map_io_err(e: std::io::Error, path: &str) -> SubstrateError {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::NotFound => SubstrateError::NotFound {
            resource: path.to_owned(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        },
        ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
            path: path.to_owned(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        },
        _ => SubstrateError::IoError {
            path: path.to_owned(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        },
    }
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
    use std::sync::Arc;
    use tempfile::TempDir;

    struct NoopJail;
    impl substrate_domain::PathJailPort for NoopJail {
        fn jail(&self, _: &JailedPath, raw: &std::path::Path) -> SubstrateResult<JailedPath> {
            Ok(JailedPath::new_jailed(raw.to_path_buf()))
        }
    }

    fn make_deps() -> FsQueryDeps {
        use crate::response::FsQueryDeps;
        FsQueryDeps {
            jail: Arc::new(NoopJail),
            walker: Arc::new(crate::walker::legacy::LegacyWalker::new()),
            hasher: Arc::new(crate::hash_factory::Blake3Hasher::new()),
            statter: Arc::new(crate::stat_factory::PortableStatter::new()),
            capabilities: Arc::new(substrate_domain::Capabilities::default()),
        }
    }

    #[tokio::test]
    async fn read_utf8_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hello.txt");
        std::fs::write(&path, b"hello world").unwrap();
        let deps = make_deps();
        let req = FsReadRequest {
            path: path.to_string_lossy().into_owned(),
            encoding: ReadEncoding::Text,
            offset_bytes: 0,
            length_bytes: None,
        };
        let resp = handle_fs_read(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resp.structured_content["content"], "hello world");
        assert_eq!(resp.structured_content["encoding"], "text");
    }

    #[tokio::test]
    async fn read_as_base64() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.bin");
        std::fs::write(&path, [0u8, 1, 2, 3]).unwrap();
        let deps = make_deps();
        let req = FsReadRequest {
            path: path.to_string_lossy().into_owned(),
            encoding: ReadEncoding::Base64,
            offset_bytes: 0,
            length_bytes: None,
        };
        let resp = handle_fs_read(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resp.structured_content["encoding"], "base64");
        let encoded = resp.structured_content["content"].as_str().unwrap();
        let decoded = base64_simd::STANDARD
            .decode_to_vec(encoded.as_bytes())
            .unwrap();
        assert_eq!(decoded, vec![0u8, 1, 2, 3]);
    }

    #[tokio::test]
    async fn read_missing_file_returns_not_found() {
        let deps = make_deps();
        let req = FsReadRequest {
            path: "/tmp/__substrate_no_such_file_xyz".to_owned(),
            encoding: ReadEncoding::Text,
            offset_bytes: 0,
            length_bytes: None,
        };
        let err = handle_fs_read(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, SubstrateError::NotFound { .. }));
    }

    #[tokio::test]
    async fn read_byte_range() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("range.txt");
        std::fs::write(&path, b"abcdefghij").unwrap();
        let deps = make_deps();
        let req = FsReadRequest {
            path: path.to_string_lossy().into_owned(),
            encoding: ReadEncoding::Text,
            offset_bytes: 2,
            length_bytes: Some(4),
        };
        let resp = handle_fs_read(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resp.structured_content["content"], "cdef");
    }
}
