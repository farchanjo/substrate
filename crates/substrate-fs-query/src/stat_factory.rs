//! `StatFactory` — `PortFactory<dyn StatPort>` per ADR-0042.
//!
//! Selects the file-metadata implementation tier at startup:
//!
//! | Platform     | `StatTier`            | Implementation        |
//! |--------------|-----------------------|-----------------------|
//! | Linux ≥ 4.11 | `LinuxStatx`          | `LinuxStatter`        |
//! | Linux < 4.11 | `LinuxFstatat`        | `PortableStatter`     |
//! | macOS ≥ 10.3 | `MacosGetattrlist`    | `MacosStatter`        |
//! | macOS < 10.3 | `MacosFstatat`        | `PortableStatter`     |
//! | Other        | `PortableMetadata`    | `PortableStatter`     |
//!
//! `PortableStatter` uses `std::fs::symlink_metadata` (no `nix` dependency).
//!
//! # ADR references
//! - ADR-0042: tiered stat primitives + capability selection.
//! - ADR-0003: Zone B — sync I/O runs in `spawn_blocking`; callers must wrap.

use std::sync::{Arc, OnceLock};

use substrate_domain::ports::stat::FileStat;
use substrate_domain::value_objects::jailed_path::JailedPath;
use substrate_domain::{
    Capabilities, PortFactory, StatPort, StatTier, SubstrateError, SubstrateResult,
};

// ---- Shared timestamp helper ------------------------------------------------

/// Converts a Unix timestamp in seconds to `time::OffsetDateTime`.
///
/// Out-of-range timestamps fall back to the Unix epoch rather than panicking.
#[inline]
fn secs_to_datetime(secs: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(secs)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

// ---- Portable statter -------------------------------------------------------

/// Cross-platform stat implementation using `std::fs::symlink_metadata`.
///
/// Does not follow symlinks to match the `lstat` contract declared in
/// [`StatPort`]. Used as the fallback tier on all platforms and as the
/// primary tier when the kernel-native tier is unavailable.
#[derive(Debug, Default)]
pub struct PortableStatter;

impl PortableStatter {
    /// Creates a new `PortableStatter`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl StatPort for PortableStatter {
    fn stat(&self, path: &JailedPath) -> SubstrateResult<FileStat> {
        use std::io::ErrorKind;
        use std::time::UNIX_EPOCH;

        let meta = std::fs::symlink_metadata(path.as_path()).map_err(|e| match e.kind() {
            ErrorKind::NotFound => SubstrateError::NotFound {
                resource: path.to_string(),
                correlation_id: None,
            },
            ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
                path: path.to_string(),
                correlation_id: None,
            },
            _ => SubstrateError::IoError {
                path: path.to_string(),
                correlation_id: None,
            },
        })?;

        let modified_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());

        let accessed_secs = meta
            .accessed()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());

        #[expect(
            clippy::cast_possible_wrap,
            reason = "unix timestamps in the valid range fit in i64"
        )]
        let modified_at = secs_to_datetime(modified_secs as i64);
        #[expect(
            clippy::cast_possible_wrap,
            reason = "unix timestamps in the valid range fit in i64"
        )]
        let accessed_at = secs_to_datetime(accessed_secs as i64);

        Ok(FileStat {
            size_bytes: meta.len(),
            is_dir: meta.is_dir(),
            is_file: meta.is_file(),
            is_symlink: meta.is_symlink(),
            modified_at,
            accessed_at,
        })
    }
}

// ---- Linux statx(2) tier ---------------------------------------------------

/// Linux-native stat implementation using `statx(2)` (kernel ≥ 4.11).
///
/// Calls the `statx(2)` syscall via `libc::statx`, which returns an extended
/// attribute set including birth time, inode, and dev in addition to the
/// classic `lstat` fields.
///
/// Uses `AT_SYMLINK_NOFOLLOW` to preserve `lstat` semantics declared by
/// [`StatPort`]. The path is converted to a `CString` for the FFI boundary;
/// the buffer is a stack-allocated `MaybeUninit<libc::statx>`.
///
/// This struct is only compiled on Linux (`cfg(target_os = "linux")`).
///
/// # ADR references
/// - ADR-0042: `StatTier::LinuxStatx` — primary tier for Linux ≥ 4.11.
/// - ADR-0044: unsafe permitted for platform-native syscalls in adapter crates.
#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct LinuxStatter;

#[cfg(target_os = "linux")]
impl LinuxStatter {
    /// Creates a new `LinuxStatter`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "linux")]
impl StatPort for LinuxStatter {
    fn stat(&self, path: &JailedPath) -> SubstrateResult<FileStat> {
        linux_stat_impl(path)
    }
}

/// Inner implementation for `LinuxStatter::stat`.
///
/// Factored out so the `unsafe` block is tightly scoped to the FFI call and
/// the `#[allow(unsafe_code)]` annotation lives only on this function.
#[cfg(target_os = "linux")]
#[allow(
    unsafe_code,
    reason = "statx(2) FFI via libc: CString is valid for call duration; \
              MaybeUninit<libc::statx> is assume_init'd after ret==0; \
              AT_SYMLINK_NOFOLLOW preserves lstat semantics; ADR-0042 + ADR-0044"
)]
fn linux_stat_impl(path: &JailedPath) -> SubstrateResult<FileStat> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;

    // Build the NUL-terminated path for the C ABI.
    let c_path = CString::new(path.as_path().as_os_str().as_encoded_bytes())
        .map_err(|_| SubstrateError::EncodingError {
            detail: "path contains an interior NUL byte".to_owned(),
            correlation_id: None,
        })?;

    let mut sx = MaybeUninit::<libc::statx>::uninit();

    // Attribute mask: request the fields we map into `FileStat`. Using
    // `STATX_BASIC_STATS` is equivalent but this documents intent.
    let mask: u32 = libc::STATX_TYPE | libc::STATX_MODE | libc::STATX_SIZE
        | libc::STATX_MTIME | libc::STATX_ATIME;

    // SAFETY:
    // - `libc::AT_FDCWD` (-100) is the correct sentinel for "relative to cwd".
    // - `c_path.as_ptr()` is a valid, NUL-terminated C string for the call.
    // - `sx.as_mut_ptr()` is valid for writes of `sizeof(statx)` bytes.
    // - `mask` is a sub-set of `STATX_BASIC_STATS`; no reserved bits set.
    // - `libc::AT_SYMLINK_NOFOLLOW` suppresses symlink resolution (lstat contract).
    let ret = unsafe {
        libc::statx(
            libc::AT_FDCWD,
            c_path.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
            mask,
            sx.as_mut_ptr(),
        )
    };

    if ret != 0 {
        let os_err = std::io::Error::last_os_error();
        let raw = os_err.raw_os_error().unwrap_or(0);
        return Err(match raw {
            e if e == libc::ENOENT => SubstrateError::NotFound {
                resource: path.to_string(),
                correlation_id: None,
            },
            e if e == libc::EACCES || e == libc::EPERM => SubstrateError::PermissionDenied {
                path: path.to_string(),
                correlation_id: None,
            },
            e if e == libc::ELOOP => SubstrateError::SymlinkLoop {
                path: path.to_string(),
                correlation_id: None,
            },
            _ => SubstrateError::IoError {
                path: path.to_string(),
                correlation_id: None,
            },
        });
    }

    // SAFETY: `statx` returned 0, so the kernel has written the full struct.
    let sx = unsafe { sx.assume_init() };

    // S_IFMT mask isolates the file-type nibble from the combined mode word.
    let mode = u32::from(sx.stx_mode);
    let file_type = mode & 0o17_0000_u32; // S_IFMT
    let is_dir = file_type == 0o04_0000_u32; // S_IFDIR
    let is_file = file_type == 0o10_0000_u32; // S_IFREG
    let is_symlink = file_type == 0o12_0000_u32; // S_IFLNK

    Ok(FileStat {
        size_bytes: sx.stx_size,
        is_dir,
        is_file,
        is_symlink,
        modified_at: secs_to_datetime(sx.stx_mtime.tv_sec),
        accessed_at: secs_to_datetime(sx.stx_atime.tv_sec),
    })
}

// ---- macOS getattrlist(2) tier ----------------------------------------------

// Module-level constants for the macOS getattrlist(2) implementation.
// Declared here (not inside functions) to satisfy `clippy::items_after_statements`.

/// `getattrlist(2)` output buffer size.
///
/// Layout: 4 (`total_len`) + 4 (vtype) + 16 (mtime) + 16 (atime) + 4 (mode) = 44 bytes.
/// We allocate 64 bytes for safety margin.
#[cfg(target_os = "macos")]
const GETATTRLIST_BUF_SIZE: usize = 64;

/// `FSOPT_NOFOLLOW` (`0x0000_0001`): suppresses symlink resolution at the final
/// path component, matching `lstat` semantics declared by [`StatPort`].
#[cfg(target_os = "macos")]
const FSOPT_NOFOLLOW: u32 = 0x0000_0001;

/// vtype value for a regular file (`VREG`) from `<sys/vnode.h>`.
#[cfg(target_os = "macos")]
const MACOS_VREG: u32 = 1;

/// vtype value for a directory (`VDIR`) from `<sys/vnode.h>`.
#[cfg(target_os = "macos")]
const MACOS_VDIR: u32 = 2;

/// vtype value for a symbolic link (`VLNK`) from `<sys/vnode.h>`.
///
/// Note: `VLNK` is 5 in the macOS kernel headers, not 10. The value 10
/// appears in some third-party references but is incorrect for current macOS.
#[cfg(target_os = "macos")]
const MACOS_VLNK: u32 = 5;

/// macOS-native stat implementation using `getattrlist(2)` (macOS ≥ 10.3).
///
/// Calls `libc::getattrlist` via a narrow `unsafe` block — the only `unsafe`
/// in this module. The FFI boundary is confined to the single syscall; all
/// pointer arithmetic and result decoding uses well-typed Rust. The
/// `unsafe` block carries a SAFETY comment documenting every invariant.
///
/// Uses `FSOPT_NOFOLLOW` to preserve `lstat` semantics, matching the
/// `AT_SYMLINK_NOFOLLOW` behaviour of the Linux tier.
///
/// This struct is only compiled on macOS (`cfg(target_os = "macos")`).
///
/// # ADR references
/// - ADR-0042: `StatTier::MacosGetattrlist` — primary tier for macOS.
#[cfg(target_os = "macos")]
#[derive(Debug, Default)]
pub struct MacosStatter;

#[cfg(target_os = "macos")]
impl MacosStatter {
    /// Creates a new `MacosStatter`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "macos")]
impl StatPort for MacosStatter {
    fn stat(&self, path: &JailedPath) -> SubstrateResult<FileStat> {
        macos_stat_impl(path)
    }
}

/// Inner implementation for `MacosStatter::stat`, extracted to keep the
/// `unsafe` block tightly scoped and separate from the trait impl boilerplate.
#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "getattrlist(2) FFI: pointers are valid for the call's duration; \
              ADR-0042 macOS native stat tier + ADR-0044 (no subprocess spawned)"
)]
fn macos_stat_impl(path: &JailedPath) -> SubstrateResult<FileStat> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;

    // ---- Attribute selection ------------------------------------------------
    //
    // We request the minimal common attribute set needed to populate FileStat:
    //   ATTR_CMN_OBJTYPE    — determines is_dir / is_file / is_symlink
    //   ATTR_CMN_MODTIME    — last modification time (timespec)
    //   ATTR_CMN_ACCTIME    — last access time (timespec)
    //   ATTR_CMN_ACCESSMASK — POSIX permission bits (u32)
    //   ATTR_CMN_DEVID      — device ID (dev_t, for completeness)
    //
    // ATTR_FILE_DATALENGTH would give us the logical file size, but it lives in
    // fileattr not commonattr. We fall back to ATTR_CMN_OBJTYPE + a stat for
    // the size field using std::fs::symlink_metadata to keep this impl simple.
    // A future optimisation can add ATTR_FILE_DATALENGTH in fileattr.

    // Build the NUL-terminated path for the C call.
    let c_path = CString::new(path.as_path().as_os_str().as_encoded_bytes())
        .map_err(|_| SubstrateError::EncodingError {
            detail: "path contains an interior NUL byte".to_owned(),
            correlation_id: None,
        })?;

    // The attrlist descriptor (mirrors `struct attrlist` in <sys/attr.h>).
    // libc defines this struct for macOS — same layout, no home-grown repr needed.
    let mut al = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: (libc::ATTR_CMN_OBJTYPE
            | libc::ATTR_CMN_MODTIME
            | libc::ATTR_CMN_ACCTIME
            | libc::ATTR_CMN_ACCESSMASK) as libc::attrgroup_t,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };

    // Buffer layout returned by getattrlist for the selected attributes (in
    // order of the attribute bitmask, from lowest bit to highest):
    //
    //   u32          total length of this record (always present, prepended)
    //   u32          stx_type  (ATTR_CMN_OBJTYPE  — vtype enum: VREG=1, VDIR=2, VLNK=10)
    //   timespec     mtime     (ATTR_CMN_MODTIME  — {tv_sec: i64, tv_nsec: i64})
    //   timespec     atime     (ATTR_CMN_ACCTIME  — {tv_sec: i64, tv_nsec: i64})
    //   u32          mode      (ATTR_CMN_ACCESSMASK — POSIX mode bits)
    //
    // Total: 4 + 4 + 16 + 16 + 4 = 44 bytes; 64-byte buffer adds safety margin.
    let mut buf = MaybeUninit::<[u8; GETATTRLIST_BUF_SIZE]>::uninit();

    // SAFETY:
    // - `c_path.as_ptr()` is a valid, NUL-terminated C string; the CString
    //   lives for the duration of this call.
    // - `&mut al as *mut _` points to a fully initialised `libc::attrlist`
    //   with `bitmapcount == ATTR_BIT_MAP_COUNT`; the kernel reads but does
    //   not write through this pointer.
    // - `buf.as_mut_ptr() as *mut _` is valid for writes of BUF_SIZE bytes;
    //   `BUF_SIZE` is passed as the capacity so the kernel cannot overflow it.
    // - The pointers do not alias.
    // ALLOW: unsafe_code allowed at fn level via `#[allow(unsafe_code)]` on
    //   `macos_stat_impl`; rationale in the function-level attribute.
    let ret = unsafe {
        libc::getattrlist(
            c_path.as_ptr(),
            std::ptr::addr_of_mut!(al).cast::<libc::c_void>(),
            buf.as_mut_ptr().cast::<libc::c_void>(),
            GETATTRLIST_BUF_SIZE,
            FSOPT_NOFOLLOW,
        )
    };

    if ret != 0 {
        let os_err = std::io::Error::last_os_error();
        let raw = os_err.raw_os_error().unwrap_or(0);
        return Err(match raw {
            e if e == libc::ENOENT => SubstrateError::NotFound {
                resource: path.to_string(),
                correlation_id: None,
            },
            e if e == libc::EACCES || e == libc::EPERM => SubstrateError::PermissionDenied {
                path: path.to_string(),
                correlation_id: None,
            },
            e if e == libc::ELOOP => SubstrateError::SymlinkLoop {
                path: path.to_string(),
                correlation_id: None,
            },
            _ => SubstrateError::IoError {
                path: path.to_string(),
                correlation_id: None,
            },
        });
    }

    // SAFETY: `getattrlist` returned 0, so `buf` has been fully initialised by
    // the kernel. `assume_init` is safe after a successful write by the kernel.
    // The `#[allow(unsafe_code)]` on this function covers this block.
    let buf: [u8; GETATTRLIST_BUF_SIZE] = unsafe { buf.assume_init() };

    // Byte offsets within the returned buffer (attributes packed in bitmask order,
    // lowest-bit first; the total_length u32 at offset 0 is always present):
    //   0..4   — total record length (u32, always prepended)
    //   4..8   — vtype (u32: ATTR_CMN_OBJTYPE)
    //   8..16  — mtime.tv_sec (i64: ATTR_CMN_MODTIME)
    //   16..24 — mtime.tv_nsec (i64, skipped)
    //   24..32 — atime.tv_sec (i64: ATTR_CMN_ACCTIME)
    //   32..40 — atime.tv_nsec (i64, skipped)
    //   40..44 — mode (u32: ATTR_CMN_ACCESSMASK)
    let vtype = read_u32_ne(&buf, 4);
    let mtime_sec = read_i64_ne(&buf, 8);
    let atime_sec = read_i64_ne(&buf, 24);

    // vtype comparison uses module-level MACOS_VDIR / MACOS_VREG / MACOS_VLNK
    // constants (from <sys/vnode.h>) to avoid items_after_statements.
    let is_dir = vtype == MACOS_VDIR;
    let is_file = vtype == MACOS_VREG;
    let is_symlink = vtype == MACOS_VLNK;

    // For size we use `std::fs::symlink_metadata` — ATTR_FILE_DATALENGTH would
    // require adding a `fileattr` field to the request. This is a one-extra-
    // syscall cost that a future Wave can eliminate by expanding the attrlist.
    let size_bytes = std::fs::symlink_metadata(path.as_path())
        .map_or(0, |m| m.len());

    Ok(FileStat {
        size_bytes,
        is_dir,
        is_file,
        is_symlink,
        modified_at: secs_to_datetime(mtime_sec),
        accessed_at: secs_to_datetime(atime_sec),
    })
}

/// Reads a `u32` (native-endian) from `buf` at byte offset `offset` using an
/// unaligned load.
///
/// The kernel writes host-endian data; we read host-endian. `read_unaligned`
/// avoids UB on platforms that prohibit unaligned derefs.
///
/// # Panics
///
/// Does not panic: caller must ensure `offset + 4 <= buf.len()`.
#[cfg(target_os = "macos")]
#[inline]
#[allow(
    unsafe_code,
    reason = "unaligned read from getattrlist output buffer at caller-verified offset; \
              ADR-0042 macOS native stat tier"
)]
#[expect(
    clippy::missing_const_for_fn,
    reason = "unsafe pointer operations in const fn require nightly; stable 1.95 does not support this"
)]
fn read_u32_ne(buf: &[u8], offset: usize) -> u32 {
    // SAFETY: caller guarantees `offset + 4 <= buf.len()` (GETATTRLIST_BUF_SIZE = 64,
    // minimum field at offset 4 + 4 bytes is well within range). The kernel
    // writes the field; we read it back. `read_unaligned` is required because
    // the buffer is `[u8]` and interior fields are not guaranteed to be aligned.
    unsafe { buf.as_ptr().add(offset).cast::<u32>().read_unaligned() }
}

/// Reads an `i64` (native-endian) from `buf` at byte offset `offset` using an
/// unaligned load.
///
/// Used for `timespec.tv_sec` fields written by the kernel in host byte order.
///
/// # Panics
///
/// Does not panic: caller must ensure `offset + 8 <= buf.len()`.
#[cfg(target_os = "macos")]
#[inline]
#[allow(
    unsafe_code,
    reason = "unaligned read from getattrlist output buffer at caller-verified offset; \
              ADR-0042 macOS native stat tier"
)]
#[expect(
    clippy::missing_const_for_fn,
    reason = "unsafe pointer operations in const fn require nightly; stable 1.95 does not support this"
)]
fn read_i64_ne(buf: &[u8], offset: usize) -> i64 {
    // SAFETY: same contract as `read_u32_ne`; used only for timespec sec fields.
    unsafe { buf.as_ptr().add(offset).cast::<i64>().read_unaligned() }
}

// ---- Factory ----------------------------------------------------------------

/// Factory that selects the `StatPort` implementation tier per ADR-0042.
///
/// Tier selection at a glance:
///
/// | `caps.stat_tier`          | Implementation     |
/// |---------------------------|--------------------|
/// | `LinuxStatx`              | `LinuxStatter`     |
/// | `LinuxFstatat`            | `PortableStatter`  |
/// | `MacosGetattrlist`        | `MacosStatter`     |
/// | `MacosFstatat`            | `PortableStatter`  |
/// | `PortableMetadata`        | `PortableStatter`  |
#[derive(Debug, Default)]
pub struct StatFactory {
    chosen: OnceLock<&'static str>,
}

impl StatFactory {
    /// Creates a new `StatFactory`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            chosen: OnceLock::new(),
        }
    }
}

impl PortFactory<dyn StatPort> for StatFactory {
    fn build(&self, caps: &Capabilities) -> Arc<dyn StatPort> {
        // ADR-0042: select the highest-available tier for the current platform.
        // Each platform arm compiles only on the relevant OS via cfg attributes.
        let tier_name = Self::select_tier(caps);
        let port = Self::make_port(caps);
        let _ = self.chosen.set(tier_name);
        port
    }

    fn chosen_tier(&self) -> &'static str {
        self.chosen.get().copied().unwrap_or("portable-metadata")
    }
}

impl StatFactory {
    /// Returns the tier-name string for the current platform and capability set.
    #[expect(
        clippy::missing_const_for_fn,
        reason = "platform cfg gates with early returns prevent const fn on stable 1.95"
    )]
    fn select_tier(caps: &Capabilities) -> &'static str {
        #[cfg(target_os = "linux")]
        {
            if matches!(caps.stat_tier, StatTier::LinuxStatx) {
                return "linux-statx";
            }
        }
        #[cfg(target_os = "macos")]
        {
            if matches!(caps.stat_tier, StatTier::MacosGetattrlist) {
                return "macos-getattrlist";
            }
        }
        // Suppress unused-variable warnings on non-Linux non-macOS platforms.
        let _ = caps;
        "portable-metadata"
    }

    /// Constructs the `StatPort` implementation for the current platform and capability set.
    fn make_port(caps: &Capabilities) -> Arc<dyn StatPort> {
        #[cfg(target_os = "linux")]
        if matches!(caps.stat_tier, StatTier::LinuxStatx) {
            return Arc::new(LinuxStatter::new());
        }

        #[cfg(target_os = "macos")]
        if matches!(caps.stat_tier, StatTier::MacosGetattrlist) {
            return Arc::new(MacosStatter::new());
        }

        let _ = caps;
        Arc::new(PortableStatter::new())
    }
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::error::Error;

    use substrate_domain::{Capabilities, StatTier, value_objects::jailed_path::JailedPath};

    use super::*;

    // Helper: build a JailedPath wrapping a real directory (not a symlink).
    //
    // `/tmp` on macOS is a symlink to `/private/tmp`. Tests use `lstat`
    // semantics (the `StatPort` contract), so they must point at the real
    // directory target, not the symlink. `/usr` is always a real directory on
    // both macOS and Linux.
    fn real_dir_jailed() -> JailedPath {
        JailedPath::new_jailed(std::path::PathBuf::from("/usr"))
    }

    // Helper: build a JailedPath that is a known symlink for testing symlink detection.
    //
    // `/tmp` on macOS is a symlink to `/private/tmp`. On Linux `/tmp` is a
    // real directory; that test is macOS-only.
    #[cfg(target_os = "macos")]
    fn symlink_jailed() -> JailedPath {
        JailedPath::new_jailed(std::path::PathBuf::from("/tmp"))
    }

    // ---- PortableStatter ----------------------------------------------------

    #[test]
    fn portable_stat_dir_is_dir() -> Result<(), Box<dyn Error>> {
        let statter = PortableStatter::new();
        let path = real_dir_jailed();
        let result = statter.stat(&path)?;
        assert!(result.is_dir, "/usr must be reported as a directory");
        assert!(!result.is_file, "/usr must not be reported as a file");
        assert!(!result.is_symlink, "/usr must not be reported as a symlink");
        Ok(())
    }

    /// On macOS `/tmp` is a symlink — `lstat` must report it as such.
    #[test]
    #[cfg(target_os = "macos")]
    fn portable_stat_symlink_is_symlink() -> Result<(), Box<dyn Error>> {
        let statter = PortableStatter::new();
        let path = symlink_jailed();
        let result = statter.stat(&path)?;
        assert!(result.is_symlink, "/tmp (macOS) must be reported as a symlink");
        assert!(!result.is_dir, "/tmp (macOS) must not be reported as a directory");
        Ok(())
    }

    // ---- LinuxStatter -------------------------------------------------------

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_statx_dir_is_dir() -> Result<(), Box<dyn Error>> {
        let statter = LinuxStatter::new();
        let path = real_dir_jailed();
        let result = statter.stat(&path)?;
        assert!(result.is_dir, "statx: /usr must be a directory");
        assert!(!result.is_file, "statx: /usr must not be a file");
        assert!(!result.is_symlink, "statx: /usr must not be a symlink");
        Ok(())
    }

    // ---- MacosStatter -------------------------------------------------------

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_getattrlist_dir_is_dir() -> Result<(), Box<dyn Error>> {
        let statter = MacosStatter::new();
        let path = real_dir_jailed();
        let result = statter.stat(&path)?;
        assert!(result.is_dir, "getattrlist: /usr must be a directory");
        assert!(!result.is_file, "getattrlist: /usr must not be a file");
        assert!(!result.is_symlink, "getattrlist: /usr must not be a symlink");
        Ok(())
    }

    /// On macOS `/tmp` is a symlink — `getattrlist` must report it as such.
    #[test]
    #[cfg(target_os = "macos")]
    fn macos_getattrlist_symlink_is_symlink() -> Result<(), Box<dyn Error>> {
        let statter = MacosStatter::new();
        let path = symlink_jailed();
        let result = statter.stat(&path)?;
        assert!(result.is_symlink, "getattrlist: /tmp (macOS) must be a symlink");
        assert!(!result.is_dir, "getattrlist: /tmp (macOS) must not be a dir");
        Ok(())
    }

    // ---- Factory tier selection ---------------------------------------------

    #[test]
    #[cfg(target_os = "linux")]
    fn factory_selects_linux_statx_tier() {
        let caps = Capabilities {
            stat_tier: StatTier::LinuxStatx,
            has_statx: true,
            ..Capabilities::default()
        };
        let factory = StatFactory::new();
        let _port = factory.build(&caps);
        assert_eq!(factory.chosen_tier(), "linux-statx");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn factory_falls_back_on_linux_fstatat() {
        let caps = Capabilities {
            stat_tier: StatTier::LinuxFstatat,
            ..Capabilities::default()
        };
        let factory = StatFactory::new();
        let _port = factory.build(&caps);
        assert_eq!(factory.chosen_tier(), "portable-metadata");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn factory_selects_macos_getattrlist_tier() {
        let caps = Capabilities {
            stat_tier: StatTier::MacosGetattrlist,
            has_getattrlistbulk: true,
            ..Capabilities::default()
        };
        let factory = StatFactory::new();
        let _port = factory.build(&caps);
        assert_eq!(factory.chosen_tier(), "macos-getattrlist");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn factory_falls_back_on_macos_fstatat() {
        let caps = Capabilities {
            stat_tier: StatTier::MacosFstatat,
            ..Capabilities::default()
        };
        let factory = StatFactory::new();
        let _port = factory.build(&caps);
        assert_eq!(factory.chosen_tier(), "portable-metadata");
    }

    #[test]
    fn factory_portable_tier() {
        let caps = Capabilities::default(); // stat_tier = PortableMetadata
        let factory = StatFactory::new();
        let _port = factory.build(&caps);
        assert_eq!(factory.chosen_tier(), "portable-metadata");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_getattrlist_smoke_returns_mtime() -> Result<(), Box<dyn Error>> {
        let statter = MacosStatter::new();
        let path = real_dir_jailed();
        let result = statter.stat(&path)?;
        // Unix epoch would be a bug; /usr always has a real mtime.
        assert!(
            result.modified_at.unix_timestamp() > 0,
            "mtime must be after Unix epoch"
        );
        Ok(())
    }
}
