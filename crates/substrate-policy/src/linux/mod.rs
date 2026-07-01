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
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::allowlist::Allowlist;
use crate::nfc;

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

/// `SYS_openat2` syscall number per architecture.
///
/// Stable values since kernel 5.6 per `arch/*/entry/syscalls/` in the Linux tree:
/// - `x86_64`:  437 (`arch/x86/entry/syscalls/syscall_64.tbl`)
/// - aarch64: 437 (`arch/arm64/include/asm/unistd.h`)
/// - riscv64: 437 (`arch/riscv/include/asm/unistd.h`)
/// - riscv32: 437 (same table as riscv64)
/// - s390x:   439 (`arch/s390/kernel/syscalls/syscall.tbl`)
///
/// Unknown architectures produce a `compile_error!` so silent miscompilation
/// is impossible; add an explicit arm when porting to a new target.
#[cfg(target_arch = "x86_64")]
pub const SYS_OPENAT2: libc::c_long = 437;
#[cfg(target_arch = "aarch64")]
pub const SYS_OPENAT2: libc::c_long = 437;
#[cfg(target_arch = "s390x")]
pub const SYS_OPENAT2: libc::c_long = 439;
#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
pub const SYS_OPENAT2: libc::c_long = 437;
#[cfg(not(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "s390x",
    target_arch = "riscv64",
    target_arch = "riscv32",
)))]
compile_error!(
    "SYS_OPENAT2 is unknown for this target architecture; \
     add an explicit value from arch/*/entry/syscalls/ in the Linux kernel tree"
);

// O_PATH | O_CLOEXEC — open purely for validation; no actual I/O.
const OPEN_FLAGS: u64 = (libc::O_PATH | libc::O_CLOEXEC) as u64;

// ---- Capability probe -------------------------------------------------------

/// Probes whether `openat2(2)` is available on the running kernel (>= 5.6).
///
/// Strategy (ADR-0042 §"attempt with safe minimal arguments"):
/// attempts `openat2` with `AT_FDCWD`, an empty path, `O_PATH`, and
/// `RESOLVE_BENEATH` (0x08).  `ENOSYS` (38) → absent; any other result
/// (typically `ENOENT` for the empty path) → syscall is present.
///
/// This function is called from `substrate-mcp-server`'s capability probe so
/// that the unsafe syscall lives entirely within this crate's carve-out
/// (ADR-0042 + ADR-0044), keeping `substrate-mcp-server` free of `unsafe`.
///
/// # Safety (contained within)
///
/// All `unsafe` is narrowly scoped to `libc::syscall` + `libc::close`.
/// No pointer outlives the call frame; the fd (if any) is closed immediately.
#[must_use]
pub fn probe_openat2_available() -> bool {
    let how = OpenHow {
        flags: libc::O_PATH as u64,
        mode: 0,
        resolve: RESOLVE_BENEATH,
    };

    // SAFETY: `libc::syscall` with `SYS_OPENAT2`:
    //   - `SYS_OPENAT2` is the arch-correct syscall number from this module.
    //   - `AT_FDCWD` is a valid pseudo-fd; no open descriptor required.
    //   - `c"".as_ptr()` is a NUL-terminated empty path in static storage;
    //     valid for the call duration.
    //   - `&raw const how` points to a stack-allocated `OpenHow`; valid for
    //     the call duration; no pointer is retained after the syscall returns.
    //   - The fourth argument is `size_of::<OpenHow>()` as required by the ABI;
    //     the struct is a fixed 24 bytes (3x u64), so the usize -> c_long cast
    //     below can never wrap.
    // On success (unexpected for empty path), we close the fd immediately.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "size_of::<OpenHow>() is a fixed 24-byte struct; can never approach c_long::MAX"
    )]
    let result = unsafe {
        libc::syscall(
            SYS_OPENAT2,
            libc::c_long::from(libc::AT_FDCWD),
            c"".as_ptr(),
            &raw const how,
            std::mem::size_of::<OpenHow>() as libc::c_long,
        )
    };

    if result >= 0 {
        // Unexpected success with empty path — close fd, report present.
        // SAFETY: `result` is a valid file descriptor just returned by the kernel;
        // fd values are always small non-negative numbers well within i32 range.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "fd values are always small non-negative numbers well within i32 range"
        )]
        unsafe {
            libc::close(result as libc::c_int);
        }
        return true;
    }

    // SAFETY: `__errno_location()` returns a thread-local pointer valid for
    // an immediate read.  No write occurs here.
    let errno = unsafe { *libc::__errno_location() };
    // ENOSYS (libc::ENOSYS == 38 on Linux) means the syscall is absent.
    errno != libc::ENOSYS
}

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
    CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| SubstrateError::InvalidArgument {
        offending_field: "path".to_owned(),
        reason: format!("null byte in path: {}", path.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })
}

// ---- Openat2Jail ------------------------------------------------------------

/// Tier-1 Linux path-jail adapter backed by `openat2(2)`.
///
/// Constructed by `PathJailFactory` when `caps.has_openat2` is `true`.
#[expect(
    clippy::redundant_pub_crate,
    reason = "clippy::redundant_pub_crate and clippy::unreachable_pub directly \
              contradict each other for a pub(crate) item inside a private \
              module (each lint's suggested fix is what the other flags); \
              pub(crate) is the semantically correct choice. See \
              substrate-policy/nfc.rs for the same precedent."
)]
pub(crate) struct Openat2Jail {
    allowlist: Allowlist,
}

impl Openat2Jail {
    /// Creates a new `Openat2Jail` wrapping the given allowlist.
    #[must_use]
    pub(crate) const fn new(allowlist: Allowlist) -> Self {
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
        //   - `&raw const how` points to a stack-allocated `OpenHow`.
        //   - `size_of::<OpenHow>()` is the required fourth argument; the
        //     struct is a fixed 24 bytes, so the usize -> c_long cast below
        //     can never wrap.
        // The syscall cannot escape the stack frame; no pointer is stored.
        #[expect(
            clippy::cast_possible_wrap,
            reason = "size_of::<OpenHow>() is a fixed 24-byte struct; can never approach c_long::MAX"
        )]
        let fd = unsafe {
            libc::syscall(
                SYS_OPENAT2,
                libc::c_long::from(root_fd.as_raw_fd()),
                candidate_cstr.as_ptr(),
                &raw const how,
                std::mem::size_of::<OpenHow>() as libc::c_long,
            )
        };

        if fd < 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return Err(map_openat2_errno(errno, raw_path));
        }

        // Resolve the canonical path via /proc/self/fd/<fd> before closing the
        // fd, so the kernel gives us the true resolved path rather than a
        // lexically constructed one.  Lexical construction (join + strip_prefix)
        // produces a double-prefix phantom path when allowlist_root is a prefix
        // of raw_path (e.g. root=/data, path=/data/sub/f → /data/data/sub/f).
        // `fd` is formatted directly (no cast to c_int needed for display).
        let proc_link = format!("/proc/self/fd/{fd}");
        let readlink_result = std::fs::read_link(&proc_link);

        // Close the validation fd after we have read the procfs link (success
        // or failure). SAFETY: `fd` is a valid file descriptor returned by the
        // syscall above (checked `fd >= 0` earlier); fd values are always small
        // non-negative numbers well within i32 range. `libc::close` is the
        // correct way to release it; it cannot fail in a way that causes UB
        // here (EINTR is benign on Linux for close(2)).
        #[expect(
            clippy::cast_possible_truncation,
            reason = "fd values are always small non-negative numbers well within i32 range"
        )]
        unsafe {
            libc::close(fd as libc::c_int);
        }

        // Fail CLOSED on readlink failure (ADR-0035). A lexical fallback would
        // trivially satisfy its own containment post-check (it is constructed
        // from allowlist_root), defeating the jail — so we reject instead of
        // trusting an unverified path.
        let canonical = readlink_result.map_err(|err| {
            tracing::warn!(
                path = %raw_path.display(),
                error = %err,
                "openat2 jail: /proc/self/fd readlink failed; rejecting (fail-closed)"
            );
            SubstrateError::PathOutsideAllowlist {
                path: raw_path.display().to_string(),
                correlation_id: None,
            }
        })?;

        // Post-check: the kernel-resolved path must still be beneath
        // allowlist_root (defence in depth — rejects magic-link escapes
        // that somehow slipped past RESOLVE_NO_MAGICLINKS on older kernels).
        // Both sides are normalized to NFC (ADR-0035 §Decision 6).
        if !nfc::is_contained(&canonical, allowlist_root.as_path()) {
            return Err(SubstrateError::PathOutsideAllowlist {
                path: canonical.display().to_string(),
                correlation_id: None,
            });
        }

        // Final allowlist cross-check for defence in depth.
        self.allowlist.jail(&canonical)
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
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module — panics and unwraps on assertion failure are the intended behavior"
)]
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
    /// the tempdir root. Only runs on Linux where `SYS_openat2` is available.
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
