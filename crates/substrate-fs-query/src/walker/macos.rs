//! macOS native walker — `getattrlistbulk(2)` for batched per-entry metadata.
//!
//! `getattrlistbulk(2)` is available on macOS 10.10+ (Yosemite) and returns
//! multiple directory entries with their attributes in a single syscall,
//! reducing the syscall count dramatically compared to readdir + lstat.
//!
//! # Zone classification (ADR-0003)
//!
//! Zone B: the blocking FFI loop runs in `tokio::task::spawn_blocking`.
//! Results are streamed to the async caller via a bounded mpsc channel (64).
//!
//! # Cancellation (ADR-0037)
//!
//! The blocking task checks the `CancellationToken` every 256 entries so
//! it can exit cooperatively without draining the full directory tree.
//!
//! # Safety justification (ADR-0042 + ADR-0044)
//!
//! This module calls `libc::getattrlistbulk(2)`, `libc::open`, and
//! `libc::close` — POSIX-standard syscalls for directory enumeration on macOS.
//! No subprocess is spawned (ADR-0044). The `unsafe` blocks are narrowly
//! scoped to FFI calls and raw-buffer parsing. Every raw pointer is derived
//! from a `Vec<u8>` owned exclusively by this frame; no pointer escapes the
//! function. Buffer bounds are verified before every field read.
#![allow(
    unsafe_code,
    reason = "getattrlistbulk(2) FFI + variable-record buffer parsing on macOS. \
              ADR-0042 (macOS walker tier 1) + ADR-0044 (no subprocess spawned)."
)]

use futures::stream::BoxStream;
use substrate_domain::{
    SubstrateResult,
    ports::dir_walker::{DirEntry, DirWalkerPort, WalkOpts},
    value_objects::jailed_path::JailedPath,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;


// ---- Constants ---------------------------------------------------------------

/// mpsc channel depth for back-pressure between blocking walker and async consumer.
const CHANNEL_DEPTH: usize = 64;

/// How many entries to process between `CancellationToken` checks (ADR-0037).
const CANCEL_CHECK_INTERVAL: usize = 256;

/// Buffer size for `getattrlistbulk` — 64 KiB covers ~100 entries with names.
const BULK_BUF_SIZE: usize = 65_536;

// ---- getattrlistbulk attribute constants ------------------------------------
//
// These are the raw constant values from <sys/attr.h> (macOS SDK 14).
// We re-declare them here because `libc` does not expose the full
// `getattrlistbulk` attribute constant set.

/// Number of bitmaps in `attrlist` (always 5 per the macOS ABI).
const ATTR_BIT_MAP_COUNT: u16 = 5;

/// COMMONATTR bit: include the returned-attributes bitmap in each record.
/// When this bit is set, each record starts with an `attribute_set_t` (32 bytes)
/// describing which fields were actually returned.
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;

/// COMMONATTR bit: entry name as an `attrreference_t` (offset + length pair).
const ATTR_CMN_NAME: u32 = 0x0000_0001;

/// COMMONATTR bit: object type (`u32` vtype — VREG, VDIR, VLNK, …).
const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;

/// COMMONATTR bit: modification time as a `struct timespec` (16 bytes on arm64).
const ATTR_CMN_MODTIME: u32 = 0x0000_0400;

/// FILEATTR bit: data-fork length in bytes (`off_t` = `i64`).
const ATTR_FILE_DATALENGTH: u32 = 0x0000_0200;

/// vtype value for a regular file (`VREG`).
const VREG: u32 = 1;

/// vtype value for a directory (`VDIR`).
const VDIR: u32 = 2;

// ---- attrlist FFI struct -----------------------------------------------------

/// Mirror of `struct attrlist` from `<sys/attr.h>`.
///
/// Must be `#[repr(C)]` with the exact field order and types expected by the
/// macOS kernel. `bitmapcount` must always be `ATTR_BIT_MAP_COUNT` (5).
#[repr(C)]
struct AttrList {
    bitmapcount: u16,
    reserved:    u16,
    commonattr:  u32,
    volattr:     u32,
    dirattr:     u32,
    fileattr:    u32,
    forkattr:    u32,
}

impl AttrList {
    /// Builds an `AttrList` requesting name, object type, mtime, and file size.
    const fn new() -> Self {
        Self {
            bitmapcount: ATTR_BIT_MAP_COUNT,
            reserved:    0,
            commonattr:  ATTR_CMN_RETURNED_ATTRS
                | ATTR_CMN_NAME
                | ATTR_CMN_OBJTYPE
                | ATTR_CMN_MODTIME,
            volattr:  0,
            dirattr:  0,
            fileattr: ATTR_FILE_DATALENGTH,
            forkattr: 0,
        }
    }
}

// ---- Record parser -----------------------------------------------------------

/// Parsed fields from a single `getattrlistbulk` record.
struct BulkEntry {
    name:   String,
    is_dir: bool,
    size:   Option<u64>,
}

/// Parses one variable-length record from the `getattrlistbulk` output buffer.
///
/// Each record layout (when `ATTR_CMN_RETURNED_ATTRS` is requested) is:
///
/// ```text
/// [u32 total_length]
/// [attribute_set_t returned_attrs]   32 bytes  (commonattr, volattr, dirattr, fileattr, forkattr)
/// [attrreference_t name_ref]          8 bytes  (offset from &name_ref, length incl. NUL)
/// [u32 obj_type]                      4 bytes
/// [struct timespec mtime]            16 bytes  (tv_sec i64 + tv_nsec i64 on arm64)
/// [i64 file_datalength]               8 bytes  (FILEATTR — present for files only)
/// [name bytes at name_ref.offset]    variable
/// ```
///
/// Returns `None` if the record is too short or the name cannot be decoded.
///
/// # Safety
///
/// The caller guarantees `record` is exactly `total_length` bytes (the `u32`
/// at the start of the record). All byte reads are bounds-checked via slice
/// indexing; no raw pointer arithmetic without a corresponding bounds check.
fn parse_record(record: &[u8]) -> Option<BulkEntry> {
    // Minimum viable record body (after total_length is stripped by caller):
    // returned_attrs(20) + name_ref(8) + obj_type(4) + mtime(16) = 48 bytes.
    // Directory records do not include file_datalength, so 48 is the minimum.
    // File records add 8 bytes for datalength = 56 bytes minimum.
    if record.len() < 48 {
        return None;
    }

    let mut cursor = 0usize;

    // --- total_length already consumed by the caller; record starts after it ---

    // --- returned_attrs: 5 × u32 = 20 bytes ----------------------------------
    // We don't need to inspect which attrs were returned for the minimal
    // implementation — we requested a fixed set and assume all were filled.
    // Advance past the 5 bitmap words (20 bytes) but reserve 32 for
    // `attribute_set_t` padding on some kernel versions.
    // `attribute_set_t` on macOS is defined as:
    //   typedef struct attribute_set { attrgroup_t commonattr; ... forkattr; }
    // which is 5 × u32 = 20 bytes on all architectures; no padding to 32.
    let _common_returned = read_u32_le(record, cursor)?;
    cursor += 4;
    let _vol_returned    = read_u32_le(record, cursor)?;
    cursor += 4;
    let _dir_returned    = read_u32_le(record, cursor)?;
    cursor += 4;
    let _file_returned   = read_u32_le(record, cursor)?;
    cursor += 4;
    let _fork_returned   = read_u32_le(record, cursor)?;
    cursor += 4;
    // Total consumed for returned_attrs: 20 bytes.

    // --- name_ref: attrreference_t (i32 offset + u32 length) = 8 bytes ------
    let name_ref_pos = cursor; // position of the attrreference_t itself
    let name_offset  = read_i32_le(record, cursor)? as isize;
    cursor += 4;
    let name_length  = read_u32_le(record, cursor)? as usize;
    cursor += 4;

    // --- obj_type: u32 -------------------------------------------------------
    let obj_type = read_u32_le(record, cursor)?;
    cursor += 4;

    // --- mtime: struct timespec (tv_sec i64 + tv_nsec i64 = 16 bytes on arm64)
    // We don't need mtime for DirEntry yet — skip it.
    cursor += 16;

    // --- file_datalength: i64 (present when obj_type == VREG) ----------------
    let size_bytes: Option<u64> = if obj_type == VREG && cursor + 8 <= record.len() {
        let v = read_i64_le(record, cursor).unwrap_or(0);
        #[allow(clippy::cast_sign_loss, reason = "kernel guarantees non-negative size")]
        Some(v as u64)
    } else {
        None
    };

    // --- Name: located via name_ref ------------------------------------------
    // The offset in `attrreference_t` is relative to the address of the
    // `attrreference_t` field itself (i.e., `name_ref_pos`).
    let name_start =
        name_ref_pos.cast_signed().checked_add(name_offset)?.cast_unsigned();
    let name_end = name_start.checked_add(name_length)?;

    if name_end > record.len() || name_length == 0 {
        return None;
    }

    // name_length includes the NUL terminator; strip it.
    let name_bytes = &record[name_start..name_end.saturating_sub(1)];
    let name = String::from_utf8_lossy(name_bytes).into_owned();

    if name.is_empty() || name == "." || name == ".." {
        return None;
    }

    Some(BulkEntry {
        name,
        is_dir: obj_type == VDIR,
        size:   size_bytes,
    })
}

// ---- Byte-read helpers -------------------------------------------------------

fn read_u32_le(buf: &[u8], offset: usize) -> Option<u32> {
    let bytes: [u8; 4] = buf.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn read_i32_le(buf: &[u8], offset: usize) -> Option<i32> {
    let bytes: [u8; 4] = buf.get(offset..offset + 4)?.try_into().ok()?;
    Some(i32::from_le_bytes(bytes))
}

fn read_i64_le(buf: &[u8], offset: usize) -> Option<i64> {
    let bytes: [u8; 8] = buf.get(offset..offset + 8)?.try_into().ok()?;
    Some(i64::from_le_bytes(bytes))
}

// ---- Walker ------------------------------------------------------------------

/// macOS-native directory walker backed by `getattrlistbulk(2)`.
///
/// Retrieves multiple directory entries with their attributes in a single
/// syscall per batch, reducing syscall count vs the `ignore`-crate path.
#[derive(Debug, Default)]
pub struct MacosBulkWalker {
    /// Cancellation token for cooperative early-exit.
    cancel: CancellationToken,
}

impl MacosBulkWalker {
    /// Creates a new `MacosBulkWalker` with a fresh `CancellationToken`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
        }
    }

    /// Creates a walker that shares a caller-owned `CancellationToken`.
    #[must_use]
    pub const fn with_cancel(cancel: CancellationToken) -> Self {
        Self { cancel }
    }
}

impl DirWalkerPort for MacosBulkWalker {
    fn walk<'a>(
        &'a self,
        root: &'a JailedPath,
        opts: WalkOpts,
    ) -> BoxStream<'a, SubstrateResult<DirEntry>> {
        let root_path = root.as_path().to_path_buf();
        let cancel = self.cancel.clone();

        let (tx, rx) = mpsc::channel::<SubstrateResult<DirEntry>>(CHANNEL_DEPTH);

        let max_depth = opts.max_depth;

        tokio::task::spawn_blocking(move || {
            walk_bulk_recursive(
                &root_path,
                max_depth,
                0,
                &tx,
                &cancel,
                &mut 0usize,
            );
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Box::pin(stream)
    }
}

// ---- Recursive bulk walker (blocking) ----------------------------------------

/// Recursively walks `dir_path` using `getattrlistbulk`, sending results via `tx`.
fn walk_bulk_recursive(
    dir_path: &std::path::Path,
    max_depth: Option<usize>,
    current_depth: usize,
    tx: &mpsc::Sender<SubstrateResult<DirEntry>>,
    cancel: &CancellationToken,
    counter: &mut usize,
) {
    if max_depth.is_some_and(|limit| current_depth > limit) {
        return;
    }

    // Open the directory.
    let Ok(path_cstr) =
        std::ffi::CString::new(dir_path.as_os_str().as_encoded_bytes())
    else {
        tracing::debug!(path = %dir_path.display(), "macos walker: path contains NUL");
        return;
    };

    // SAFETY: `path_cstr` is a valid NUL-terminated C string; `O_RDONLY` is a
    // well-known flag; we check the return value before use.
    let fd = unsafe { libc::open(path_cstr.as_ptr(), libc::O_RDONLY) };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        tracing::debug!(%err, path = %dir_path.display(), "macos walker: open failed");
        let _ = tx.blocking_send(Err(substrate_domain::SubstrateError::IoError {
            path: dir_path.display().to_string(),
            correlation_id: None,
        }));
        return;
    }

    let attrlist = AttrList::new();
    let mut buf = vec![0u8; BULK_BUF_SIZE];
    let mut subdirs: Vec<std::path::PathBuf> = Vec::new();

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // SAFETY: `fd` is a valid open file descriptor for a directory.
        // `attrlist` has the correct layout (repr(C), initialized fields).
        // `buf` is a heap-owned slice of `BULK_BUF_SIZE` bytes passed as the
        // output buffer; `getattrlistbulk` writes at most `buf.len()` bytes.
        // The return value is the entry count (>0) or 0 (end) or -1 (error).
        let entry_count = unsafe {
            libc::getattrlistbulk(
                fd,
                std::ptr::addr_of!(attrlist).cast::<libc::c_void>().cast_mut(),
                buf.as_mut_ptr().cast::<libc::c_void>(),
                buf.len(),
                0,
            )
        };

        if entry_count == 0 {
            // No more entries in this directory.
            break;
        }
        if entry_count < 0 {
            let err = std::io::Error::last_os_error();
            tracing::debug!(%err, path = %dir_path.display(), "macos walker: getattrlistbulk error");
            break;
        }

        // Parse each returned record from the buffer.
        let mut pos = 0usize;
        #[allow(clippy::cast_sign_loss, reason = "entry_count > 0 is checked above")]
        let count = entry_count as usize;

        for _ in 0..count {
            if pos + 4 > buf.len() {
                break;
            }

            // Each record starts with a u32 total length (includes the length field).
            let bytes_4: [u8; 4] = buf[pos..pos + 4].try_into().unwrap_or([0; 4]);
            let total_len = u32::from_le_bytes(bytes_4) as usize;

            if total_len < 4 || pos + total_len > buf.len() {
                break;
            }

            // Slice covering the record body after the total_length field.
            let record_body = &buf[pos + 4..pos + total_len];

            *counter = counter.wrapping_add(1);
            if (*counter).is_multiple_of(CANCEL_CHECK_INTERVAL) && cancel.is_cancelled() {
                // SAFETY: closing fd is safe here; we break immediately after.
                unsafe { libc::close(fd) };
                return;
            }

            if let Some(entry) = parse_record(record_body) {
                let entry_path = dir_path.join(&entry.name);
                let jailed = JailedPath::new_jailed(entry_path.clone());

                if tx
                    .blocking_send(Ok(DirEntry {
                        path:       jailed,
                        is_dir:     entry.is_dir,
                        size_bytes: entry.size,
                    }))
                    .is_err()
                {
                    // SAFETY: receiver dropped — close fd and exit.
                    unsafe { libc::close(fd) };
                    return;
                }

                if entry.is_dir {
                    subdirs.push(entry_path);
                }
            }

            pos += total_len;
        }
    }

    // SAFETY: `fd` was successfully opened; we own it and must close it.
    unsafe { libc::close(fd) };

    for subdir in subdirs {
        if cancel.is_cancelled() {
            return;
        }
        walk_bulk_recursive(&subdir, max_depth, current_depth + 1, tx, cancel, counter);
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
    use std::fs;
    use tempfile::TempDir;
    use tokio::runtime::Runtime;

    fn make_tree() -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("alpha.txt"), b"hello").expect("write alpha");
        fs::write(dir.path().join("beta.txt"), b"world").expect("write beta");
        fs::create_dir(dir.path().join("sub")).expect("mkdir sub");
        fs::write(dir.path().join("sub/gamma.txt"), b"g").expect("write gamma");
        dir
    }

    #[test]
    fn walks_all_entries() {
        let dir = make_tree();
        let rt = Runtime::new().expect("runtime");
        let walker = MacosBulkWalker::new();
        let root = JailedPath::new_jailed(dir.path().to_path_buf());
        let opts = WalkOpts { max_depth: None };

        let entries: Vec<_> = rt.block_on(async {
            use futures::StreamExt;
            walker.walk(&root, opts).collect().await
        });

        let paths: Vec<_> = entries
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|e| e.path.as_path().to_path_buf())
            .collect();

        assert!(
            paths.iter().any(|p| p.ends_with("alpha.txt")),
            "alpha.txt not found; got {paths:?}"
        );
        assert!(paths.iter().any(|p| p.ends_with("beta.txt")), "beta.txt not found; got {paths:?}");
        assert!(paths.iter().any(|p| p.ends_with("gamma.txt")), "gamma.txt not found; got {paths:?}");
    }

    #[test]
    fn respects_max_depth_zero() {
        let dir = make_tree();
        let rt = Runtime::new().expect("runtime");
        let walker = MacosBulkWalker::new();
        let root = JailedPath::new_jailed(dir.path().to_path_buf());
        let opts = WalkOpts { max_depth: Some(0) };

        let entries: Vec<_> = rt.block_on(async {
            use futures::StreamExt;
            walker.walk(&root, opts).collect().await
        });

        let has_deep = entries
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .any(|e| e.path.as_path().ends_with("gamma.txt"));
        assert!(!has_deep, "depth-limited walk should not enter sub/");
    }

    #[test]
    fn files_report_size() {
        let dir = make_tree();
        let rt = Runtime::new().expect("runtime");
        let walker = MacosBulkWalker::new();
        let root = JailedPath::new_jailed(dir.path().to_path_buf());
        let opts = WalkOpts { max_depth: None };

        let entries: Vec<_> = rt.block_on(async {
            use futures::StreamExt;
            walker.walk(&root, opts).collect().await
        });

        let alpha = entries
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .find(|e| e.path.as_path().ends_with("alpha.txt"))
            .expect("alpha.txt must appear");
        // "hello" = 5 bytes
        assert_eq!(alpha.size_bytes, Some(5), "alpha.txt is 5 bytes");
    }
}
