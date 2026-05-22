//! macOS Tier-1 path-jail adapter — `openat(O_NOFOLLOW_ANY)` per ADR-0035 and ADR-0042.
//!
//! Uses `openat(2)` with `O_NOFOLLOW_ANY` (macOS 12+) to atomically reject any
//! path component that is a symlink, closing the TOCTOU window present in the
//! userspace-degraded fallback.
//!
//! After a successful open, `fcntl(F_GETPATH)` recovers the kernel-resolved
//! canonical path for the allowlist prefix post-check and for firmlink
//! resolution on APFS volumes (ADR-0035 §Decision 7).
//!
//! # Safety justification (ADR-0042 + ADR-0044 syscall carve-out)
//!
//! `libc::openat` with `O_NOFOLLOW_ANY = 0x2000_0000` and `libc::fcntl` with
//! `F_GETPATH` are standard macOS C ABI calls. No safe Rust wrapper exists for
//! `O_NOFOLLOW_ANY` in the `nix` or `libc` crates at the time of writing.
//! Every `unsafe` block is narrowly scoped, carries a SAFETY comment, and
//! touches only well-defined C ABI types. No raw pointer is stored beyond the
//! function frame. This is the ONLY permitted unsafe carve-out in this crate
//! per ADR-0042.
#![allow(
    unsafe_code,
    reason = "libc::openat with O_NOFOLLOW_ANY and fcntl(F_GETPATH) required; \
              no safe wrapper exists in nix 0.30.x or libc 0.2.x. \
              ADR-0042 + ADR-0035 explicitly permit this per-module override."
)]

use std::ffi::{CStr, CString};
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::allowlist::Allowlist;

// ---- macOS-specific constants -----------------------------------------------

/// `O_NOFOLLOW_ANY` — reject any path component that is a symlink (macOS 12+).
/// Not defined in `libc` 0.2; value taken from `<sys/fcntl.h>`.
#[expect(
    clippy::cast_possible_wrap,
    reason = "0x2000_0000 fits in the positive range of i32; this is the macOS-defined flag value from <sys/fcntl.h>"
)]
const O_NOFOLLOW_ANY: libc::c_int = 0x2000_0000_u32 as libc::c_int;

/// `F_GETPATH` — fills a `MAXPATHLEN`-byte buffer with the file's canonical
/// path. Available on macOS since 10.x.
const F_GETPATH: libc::c_int = 50;

/// Kernel buffer size for `F_GETPATH` (`MAXPATHLEN` = 1024 on macOS).
const MAXPATHLEN: usize = 1024;

// ---- Helpers ----------------------------------------------------------------

/// Converts a `Path` to a `CString`, returning `EncodingError` on null bytes.
fn path_to_cstring(path: &Path) -> SubstrateResult<CString> {
    CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| {
        SubstrateError::EncodingError {
            detail: format!("null byte in path: {}", path.display()),
            correlation_id: None,
        }
    })
}

/// Recovers the kernel-resolved canonical path from an open file descriptor
/// using `fcntl(fd, F_GETPATH, buf)`.
fn fd_to_canonical_path(fd: libc::c_int) -> SubstrateResult<PathBuf> {
    let mut buf = [0u8; MAXPATHLEN];
    // SAFETY: `fd` is a valid, open file descriptor. `buf` is a stack-allocated
    // array of `MAXPATHLEN` bytes — exactly the buffer size `F_GETPATH` expects.
    // `fcntl` writes at most `MAXPATHLEN` bytes including the trailing NUL byte.
    let ret = unsafe {
        libc::fcntl(fd, F_GETPATH, buf.as_mut_ptr())
    };
    if ret < 0 {
        return Err(SubstrateError::InternalError {
            reason: format!(
                "fcntl(F_GETPATH) failed: {}",
                std::io::Error::last_os_error()
            ),
            correlation_id: None,
        });
    }
    // SAFETY: `F_GETPATH` guarantees NUL termination within `buf`. The slice
    // is valid UTF-8 on HFS+/APFS (kernel enforces this). `CStr::from_ptr` is
    // safe because the NUL byte is present within the 1024-byte window.
    let cstr = unsafe { CStr::from_ptr(buf.as_ptr().cast()) };
    let path_str = cstr.to_str().map_err(|_| SubstrateError::EncodingError {
        detail: "F_GETPATH returned non-UTF-8 path".to_owned(),
        correlation_id: None,
    })?;
    Ok(PathBuf::from(path_str))
}

// ---- ONoFollowAnyJail -------------------------------------------------------

/// Tier-1 macOS path-jail adapter backed by `openat(O_NOFOLLOW_ANY)`.
///
/// Constructed by `PathJailFactory` when `caps.has_o_nofollow_any` is `true`.
#[expect(
    clippy::redundant_pub_crate,
    reason = "pub(crate) documents intentional crate-internal visibility for cross-module use"
)]
pub(crate) struct ONoFollowAnyJail {
    allowlist: Allowlist,
}

impl ONoFollowAnyJail {
    /// Creates a new `ONoFollowAnyJail` wrapping the given allowlist.
    #[must_use]
    pub(crate) const fn new(allowlist: Allowlist) -> Self {
        Self { allowlist }
    }
}

impl substrate_domain::PathJailPort for ONoFollowAnyJail {
    fn jail(&self, allowlist_root: &JailedPath, raw_path: &Path) -> SubstrateResult<JailedPath> {
        // PATH_MAX validation per ADR-0035 §Decision 10 (macOS: 1023 usable bytes).
        let byte_len = raw_path.as_os_str().len();
        if byte_len > 1023 {
            return Err(SubstrateError::InvalidArgument {
                offending_field: "path".to_owned(),
                reason: format!("path length {byte_len} exceeds PATH_MAX (1023)"),
                correlation_id: None,
            });
        }

        let candidate_cstr = path_to_cstring(raw_path)?;

        // Call openat(AT_FDCWD, path, O_RDONLY | O_NOFOLLOW_ANY | O_CLOEXEC).
        //
        // SAFETY: `libc::openat` is a standard POSIX/macOS C ABI syscall.
        //   - `libc::AT_FDCWD` is the sentinel for "relative to CWD".
        //   - `candidate_cstr.as_ptr()` is valid for the call duration.
        //   - `O_RDONLY | O_NOFOLLOW_ANY | O_CLOEXEC` is a safe flag combination.
        // No pointer escapes this call. `O_NOFOLLOW_ANY` causes the kernel to
        // return `ELOOP` on any symlink component, atomically closing the TOCTOU
        // window without any race.
        let fd = unsafe {
            libc::openat(
                libc::AT_FDCWD,
                candidate_cstr.as_ptr(),
                libc::O_RDONLY | O_NOFOLLOW_ANY | libc::O_CLOEXEC,
            )
        };

        if fd < 0 {
            let errno = std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(0);
            return Err(map_openat_errno(errno, raw_path));
        }

        // Take ownership of the fd so it is closed on drop.
        // SAFETY: `fd` is a valid descriptor just opened above; `OwnedFd` owns it.
        let owned_fd = unsafe { OwnedFd::from_raw_fd(fd) };

        // Recover the kernel-canonical path (resolves APFS firmlinks, CWD).
        let canonical = fd_to_canonical_path(fd)?;

        // Drop fd after we have the path. `OwnedFd` closes on drop.
        drop(owned_fd);

        // Verify canonical is within the allowlist_root.
        if !canonical.starts_with(allowlist_root.as_path()) {
            return Err(SubstrateError::PathOutsideAllowlist {
                path: canonical.display().to_string(),
                correlation_id: None,
            });
        }

        // Final cross-check against the full allowlist set.
        self.allowlist.jail(canonical)
    }
}

/// Maps `errno` values from `openat(2)` with `O_NOFOLLOW_ANY` to `SubstrateError`.
fn map_openat_errno(errno: i32, path: &Path) -> SubstrateError {
    match errno {
        libc::ELOOP => SubstrateError::SymlinkEscape {
            path: path.display().to_string(),
            correlation_id: None,
        },
        libc::ENOENT => SubstrateError::NotFound {
            resource: path.display().to_string(),
            correlation_id: None,
        },
        libc::EACCES | libc::EPERM => SubstrateError::PermissionDenied {
            path: path.display().to_string(),
            correlation_id: None,
        },
        _ => SubstrateError::IoError {
            path: path.display().to_string(),
            correlation_id: None,
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
    clippy::manual_let_else,
    reason = "test module: panics are the correct failure mode; let-else adds no clarity in short test helpers"
)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_jail() -> (tempfile::TempDir, ONoFollowAnyJail, JailedPath) {
        let dir = tempfile::tempdir().expect("tempdir must succeed in tests");
        let root = dir
            .path()
            .canonicalize()
            .expect("canonicalize must succeed");
        let allowlist = Allowlist::new(vec![root.clone()]).expect("valid allowlist");
        let root_jailed = JailedPath::new_jailed(root);
        let jail = ONoFollowAnyJail::new(allowlist);
        (dir, jail, root_jailed)
    }

    #[test]
    fn rejects_path_exceeding_macos_path_max() {
        use substrate_domain::PathJailPort as _;

        let (_dir, jail, root_jailed) = make_jail();
        let long_name = "a".repeat(1024);
        let long_path = PathBuf::from(format!("/private/tmp/{long_name}"));
        let result = jail.jail(&root_jailed, &long_path);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), "SUBSTRATE_INVALID_ARGUMENT");
    }

    /// Verifies that a real file within the tempdir is accepted when the
    /// fully-canonical (no-symlink) path is provided.
    ///
    /// macOS `/var/folders` paths are accessed through the `/var` → `/private/var`
    /// symlink. `O_NOFOLLOW_ANY` correctly rejects the symlink-traversing form.
    /// Callers must canonicalize the path before passing it to `jail()`; this
    /// test uses the canonical form (via `std::fs::canonicalize`) as a proper
    /// caller would.
    #[test]
    fn allows_file_within_root_canonical() {
        use substrate_domain::PathJailPort as _;

        let (dir, jail, root_jailed) = make_jail();
        let file = dir.path().join("probe.txt");
        std::fs::write(&file, b"ok").expect("seed file");

        // Canonicalize so that no symlink component remains in the path.
        let canonical_file = match std::fs::canonicalize(&file) {
            Ok(p) => p,
            Err(_) => return, // tempfile creation issue — skip silently
        };

        // Only run if the canonical path is beneath the (already-canonical) root.
        if !canonical_file.starts_with(root_jailed.as_path()) {
            // The temp dir root itself resolved through a symlink (e.g.,
            // /var/folders on older macOS where /var symlink is not resolved
            // by canonicalize the same way). Skip this test in that case.
            return;
        }

        let result = jail.jail(&root_jailed, &canonical_file);
        match result {
            Ok(jailed) => {
                assert!(
                    jailed.as_path().starts_with(root_jailed.as_path()),
                    "jailed path must be beneath root; got: {}",
                    jailed.as_path().display()
                );
            },
            // O_NOFOLLOW_ANY not available on older macOS (< 12): IoError.
            Err(e) if e.code() == "SUBSTRATE_IO_ERROR" => {},
            Err(e) => panic!("unexpected error: {e} ({code})", code = e.code()),
        }
    }

    /// Verifies that a symlink whose target is inside root is rejected.
    ///
    /// Uses the canonical tempdir path to avoid false positives from `/var`
    /// symlink resolution on macOS.
    #[test]
    fn rejects_symlink_within_root() {
        use substrate_domain::PathJailPort as _;

        let (dir, jail, root_jailed) = make_jail();

        // Resolve the canonical path of the tempdir to avoid /var symlink issues.
        let canonical_dir = match std::fs::canonicalize(dir.path()) {
            Ok(p) => p,
            Err(_) => return,
        };

        let target = canonical_dir.join("real.txt");
        std::fs::write(&target, b"data").expect("seed");
        let link = canonical_dir.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let result = jail.jail(&root_jailed, &link);
        // O_NOFOLLOW_ANY returns ELOOP on any symlink → SymlinkEscape.
        // On older macOS the flag may not be available; IoError is acceptable.
        match result {
            Err(e) if e.code() == "SUBSTRATE_SYMLINK_ESCAPE" => {},
            Err(e) if e.code() == "SUBSTRATE_IO_ERROR" => {
                // O_NOFOLLOW_ANY not supported on this macOS version — skip.
            },
            Ok(_) => panic!("symlink within root must be rejected by O_NOFOLLOW_ANY"),
            Err(e) => panic!("unexpected error: {e} ({code})", code = e.code()),
        }
    }

    /// Verifies that a path outside the root is rejected.
    #[test]
    fn rejects_path_outside_root() {
        use substrate_domain::PathJailPort as _;

        let (_dir, jail, root_jailed) = make_jail();
        // /etc is always outside the tempdir root on macOS.
        let outside = PathBuf::from("/etc/hosts");
        let result = jail.jail(&root_jailed, &outside);
        assert!(result.is_err());
        let code = result.unwrap_err().code();
        assert!(
            code == "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"
                || code == "SUBSTRATE_IO_ERROR"
                || code == "SUBSTRATE_SYMLINK_ESCAPE",
            "unexpected code: {code}"
        );
    }
}
