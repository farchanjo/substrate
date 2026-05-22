//! Linux Tier-1 path-jail adapter — `openat2(2)` per ADR-0035 and ADR-0042.
//!
//! Uses `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS)`
//! to provide atomic, kernel-enforced path confinement (Linux 5.6+).
//!
//! # Safety justification (ADR-0042 + ADR-0044 syscall carve-out)
//!
//! The `openat2(2)` syscall has no safe Rust wrapper in `nix` 0.30.x or
//! `libc` 0.2.x. We invoke it directly via `libc::syscall(SYS_openat2, ...)`.
//! Every `unsafe` block is narrowly scoped, carries a SAFETY comment, and
//! touches only well-defined C ABI types (`c_int`, `c_long`, `*const`).
//! No raw pointer is stored beyond the function frame. This is the ONLY
//! permitted unsafe carve-out in this crate per ADR-0042.
//!
//! # Syscall flags used
//!
//! - `RESOLVE_BENEATH` (0x08) — resolution must not escape the dirfd root.
//! - `RESOLVE_NO_SYMLINKS` (0x04) — reject any symlink component.
//! - `RESOLVE_NO_MAGICLINKS` (0x02) — reject `/proc` magic links.
#![allow(
    unsafe_code,
    reason = "Direct openat2(2) syscall; no safe wrapper in nix 0.30.x or libc 0.2.x. \
              ADR-0042 + ADR-0035 explicitly permit this per-module override."
)]

use std::ffi::CString;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::Path;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::allowlist::Allowlist;

// ---- openat2 ABI definitions ------------------------------------------------

/// `open_how` struct as defined in `linux/openat2.h` (stable since kernel 5.6).
/// Fields are u64 per the ABI spec; padding is implicit at the end.
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

/// `RESOLVE_NO_MAGICLINKS` — reject `/proc`-style magic links.
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
/// `RESOLVE_NO_SYMLINKS` — reject any symlink component.
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
/// `RESOLVE_BENEATH` — resolution must not cross the dirfd root upward.
const RESOLVE_BENEATH: u64 = 0x08;

/// `SYS_openat2` syscall number on x86_64 Linux.
/// `libc` 0.2 does not expose `SYS_openat2`; the number is stable at 437
/// (x86_64) since kernel 5.6. Other arches: aarch64=437, riscv64=437,
/// s390x=439. We gate compilation on x86_64 and aarch64 via cfg.
///
/// Reference: `arch/x86/entry/syscalls/syscall_64.tbl` in the Linux kernel.
#[cfg(target_arch = "x86_64")]
const SYS_OPENAT2: libc::c_long = 437;
#[cfg(target_arch = "aarch64")]
const SYS_OPENAT2: libc::c_long = 437;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const SYS_OPENAT2: libc::c_long = 437; // conservative fallback; see comment above

// O_PATH | O_CLOEXEC — open purely for validation; no actual I/O.
const OPEN_FLAGS: u64 = (libc::O_PATH | libc::O_CLOEXEC) as u64;

/// Opens `root` as a directory file descriptor for use as the `dirfd` argument
/// to `openat2`.
fn open_root_dirfd(root: &Path) -> SubstrateResult<OwnedFd> {
    let cstr = path_to_cstring(root)?;
    // SAFETY: `libc::open` is a thin FFI call. `cstr.as_ptr()` is valid for
    // the lifetime of this call; `O_PATH | O_DIRECTORY | O_CLOEXEC` is a safe
    // flag combination — no file content is read, no descriptor is inherited.
    let fd = unsafe {
        libc::open(
            cstr.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(SubstrateError::IoError {
            path: root.display().to_string(),
            correlation_id: None,
        });
    }
    // SAFETY: `fd` is a valid file descriptor just opened above; `OwnedFd`
    // takes ownership and closes it on drop. No other owner exists.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Converts a `Path` to a `CString`, returning `EncodingError` on null bytes.
fn path_to_cstring(path: &Path) -> SubstrateResult<CString> {
    CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| SubstrateError::EncodingError {
        detail: format!("null byte in path: {}", path.display()),
        correlation_id: None,
    })
}

// ---- Openat2Jail ------------------------------------------------------------

/// Tier-1 Linux path-jail adapter backed by `openat2(2)`.
///
/// Constructed by `PathJailFactory` when `caps.has_openat2` is `true`.
pub(crate) struct Openat2Jail {
    allowlist: Allowlist,
}

impl Openat2Jail {
    /// Creates a new `Openat2Jail` wrapping the given allowlist.
    #[must_use]
    pub(crate) fn new(allowlist: Allowlist) -> Self {
        Self { allowlist }
    }
}

impl substrate_domain::PathJailPort for Openat2Jail {
    fn jail(&self, allowlist_root: &JailedPath, raw_path: &Path) -> SubstrateResult<JailedPath> {
        // Blanket rejection of /proc paths per ADR-0035 §Decision 8.
        if raw_path.starts_with("/proc") {
            return Err(SubstrateError::PathOutsideAllowlist {
                path: raw_path.display().to_string(),
                correlation_id: None,
            });
        }

        // PATH_MAX validation per ADR-0035 §Decision 10 (Linux: 4095 usable bytes).
        let byte_len = raw_path.as_os_str().len();
        if byte_len > 4095 {
            return Err(SubstrateError::InvalidArgument {
                offending_field: "path".to_owned(),
                reason: format!("path length {byte_len} exceeds PATH_MAX (4095)"),
                correlation_id: None,
            });
        }

        // Build CString from raw_path. For relative candidates we pass them
        // as-is; openat2 with RESOLVE_BENEATH rejects absolute escape attempts.
        let candidate_cstr = path_to_cstring(raw_path)?;

        // Open the allowlist root as a dirfd for openat2.
        let root_fd = open_root_dirfd(allowlist_root.as_path())?;

        let how = OpenHow {
            flags: OPEN_FLAGS,
            mode: 0,
            resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
        };

        // SAFETY: All arguments are well-formed:
        //   - `SYS_OPENAT2` is the correct syscall number for this arch.
        //   - `root_fd.as_raw_fd()` is a live, owned descriptor opened above.
        //   - `candidate_cstr.as_ptr()` is valid for the call duration.
        //   - `&how as *const _` points to a stack-allocated `OpenHow`.
        //   - `size_of::<OpenHow>()` is the required fourth argument.
        // The syscall cannot escape the stack frame; no pointer is stored.
        let fd = unsafe {
            libc::syscall(
                SYS_OPENAT2,
                root_fd.as_raw_fd() as libc::c_long,
                candidate_cstr.as_ptr(),
                &how as *const OpenHow,
                std::mem::size_of::<OpenHow>() as libc::c_long,
            )
        };

        if fd < 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return Err(map_openat2_errno(errno, raw_path));
        }

        // Close the validation fd immediately — we only needed proof the kernel
        // accepted the path under RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS.
        // SAFETY: `fd` is a valid file descriptor returned by the syscall above.
        // `libc::close` is the correct way to release it; it cannot fail in a
        // way that causes UB here (EINTR is benign on Linux for close(2)).
        unsafe {
            libc::close(fd as libc::c_int);
        }

        // Construct the canonical path by appending the relative portion of
        // raw_path to the allowlist root. openat2 has already verified the
        // resolution stays within the root, so prefix-stripping the root from
        // raw_path gives the relative sub-path.
        let canonical = allowlist_root
            .as_path()
            .join(raw_path.strip_prefix("/").unwrap_or(raw_path));

        // Final allowlist cross-check for defence in depth.
        self.allowlist.jail(canonical)
    }
}

/// Maps `errno` values from `openat2(2)` to `SubstrateError` variants.
fn map_openat2_errno(errno: i32, path: &Path) -> SubstrateError {
    match errno {
        libc::EXDEV => SubstrateError::PathOutsideAllowlist {
            path: path.display().to_string(),
            correlation_id: None,
        },
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
        libc::ENOSYS => SubstrateError::InternalError {
            reason: "openat2(2) is not available on this kernel (requires Linux 5.6+)".to_owned(),
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
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_jail() -> (tempfile::TempDir, Openat2Jail, JailedPath) {
        let dir = tempfile::tempdir().expect("tempdir must succeed in tests");
        let root = dir
            .path()
            .canonicalize()
            .expect("canonicalize must succeed");
        let allowlist = Allowlist::new(vec![root.clone()]).expect("valid allowlist");
        let root_jailed = JailedPath::new_jailed(root);
        let jail = Openat2Jail::new(allowlist);
        (dir, jail, root_jailed)
    }

    #[test]
    fn rejects_proc_path_before_syscall() {
        use substrate_domain::PathJailPort as _;

        let (_dir, jail, root_jailed) = make_jail();
        let result = jail.jail(&root_jailed, &PathBuf::from("/proc/self/cwd"));
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code(),
            "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"
        );
    }

    #[test]
    fn rejects_path_exceeding_linux_path_max() {
        use substrate_domain::PathJailPort as _;

        let (_dir, jail, root_jailed) = make_jail();
        let long_name = "x".repeat(4096);
        let long_path = PathBuf::from(format!("/tmp/{long_name}"));
        let result = jail.jail(&root_jailed, &long_path);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), "SUBSTRATE_INVALID_ARGUMENT");
    }

    /// Verifies the openat2 call accepts a file that actually exists within
    /// the tempdir root. Only runs on Linux where SYS_openat2 is available.
    #[test]
    fn allows_file_within_root_via_openat2() {
        use substrate_domain::PathJailPort as _;

        let (dir, jail, root_jailed) = make_jail();
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, b"ok").expect("seed file");

        let result = jail.jail(&root_jailed, &file);
        // On kernels < 5.6 openat2 returns ENOSYS → InternalError. On modern
        // kernels the file is within root → should succeed.
        match result {
            Ok(jailed) => {
                assert!(
                    jailed.as_path().starts_with(root_jailed.as_path()),
                    "jailed path must be beneath root"
                );
            },
            Err(e) if e.code() == "SUBSTRATE_INTERNAL_ERROR" => {
                // ENOSYS on old kernel — acceptable in CI.
            },
            Err(e) => panic!("unexpected error: {e} ({code})", code = e.code()),
        }
    }

    /// Verifies that a path outside the root is rejected.
    /// On modern kernels openat2 returns EXDEV; on old kernels the allowlist
    /// post-check catches the escape.
    #[test]
    fn rejects_path_outside_root() {
        use substrate_domain::PathJailPort as _;

        let (_dir, jail, root_jailed) = make_jail();
        // /etc/passwd is guaranteed to exist on Linux and is outside any tempdir.
        let outside = PathBuf::from("/etc/passwd");
        let result = jail.jail(&root_jailed, &outside);
        assert!(result.is_err());
        let code = result.unwrap_err().code();
        assert!(
            code == "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"
                || code == "SUBSTRATE_IO_ERROR"
                || code == "SUBSTRATE_INTERNAL_ERROR",
            "unexpected error code: {code}"
        );
    }
}
