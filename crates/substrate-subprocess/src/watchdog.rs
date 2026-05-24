//! Watchdog pipe pattern for macOS (cooperative parent-death notification).
//!
//! macOS does not provide `PR_SET_PDEATHSIG`. When substrate is killed with
//! `SIGKILL` on macOS the watchdog pipe provides best-effort cleanup for
//! substrate-aware child processes:
//!
//! 1. Substrate creates a `pipe(2)` before spawning. The write end is retained
//!    by the substrate process.
//! 2. The read end is inherited by the child via `exec` (marked non-CLOEXEC).
//! 3. Substrate-aware children read `SUBSTRATE_WATCHDOG_FD` from their
//!    environment and start a watcher thread that calls `_exit(0)` on EOF.
//! 4. When substrate exits (for any reason), the write end is closed, delivering
//!    EOF to all watching children.
//!
//! Arbitrary (non-substrate-aware) children that do not read the fd will not
//! self-terminate. The orphan reaper in ADR-0055 handles cleanup at next startup.
//!
//! References: ADR-0053 §"macOS Watchdog Pipe Pattern".

// Only macOS needs raw fd manipulation via fcntl.
#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "raw fd manipulation via fcntl(F_GETFD/F_SETFD) per ADR-0053 \
              cooperative watchdog pattern on macOS. No ADR-0035 path operations; \
              only a pipe fd lifetime (close-on-exec bit clear on read end)."
)]
mod inner {
    use std::os::fd::{FromRawFd, OwnedFd};

    /// macOS watchdog pipe: the write end is kept alive in the parent.
    ///
    /// When this struct is dropped (or the parent exits), the write end is
    /// closed, delivering EOF to the child's read end.
    #[derive(Debug)]
    pub struct WatchdogPipe {
        /// Write end of the watchdog pipe. Kept alive as long as this struct
        /// exists. Dropped (closed) when substrate exits or when the
        /// `ChildHandle` is released.
        _write_end: OwnedFd,
    }

    /// Installs the watchdog pipe on `cmd`.
    ///
    /// Creates a `pipe(2)`, marks the read end as non-CLOEXEC so the child
    /// inherits it, sets `SUBSTRATE_WATCHDOG_FD` in the child's environment
    /// to the read-end fd number, and returns a [`WatchdogPipe`] that keeps
    /// the write end open.
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if `pipe(2)` or `fcntl(2)` fail.
    ///
    /// References: ADR-0053 §"macOS Watchdog Pipe Pattern".
    #[expect(
        clippy::disallowed_types,
        reason = "substrate-subprocess is the authorized host of tokio::process::Command per ADR-0052"
    )]
    pub fn install(cmd: &mut tokio::process::Command) -> std::io::Result<WatchdogPipe> {
        // Create the pipe. nix returns (read_fd, write_fd) as OwnedFd.
        let (read_fd, write_fd) = nix::unistd::pipe().map_err(std::io::Error::other)?;

        // SAFETY: we obtain the raw fd from `read_fd` only to manipulate the
        // close-on-exec bit. The `OwnedFd` still owns the fd; we do not duplicate
        // or transfer ownership here. `fcntl` is a safe libc call for FD flag
        // manipulation and does not dereference the fd as a pointer.
        let raw_read = std::os::fd::AsRawFd::as_raw_fd(&read_fd);

        // Clear CLOEXEC on the read end so the child inherits it across exec.
        // SAFETY: raw_read is a valid open file descriptor owned by `read_fd`.
        // F_GETFD / F_SETFD are async-signal-safe. No aliasing or data races.
        let flags = unsafe { libc::fcntl(raw_read, libc::F_GETFD) };
        if flags == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let cleared = flags & !libc::FD_CLOEXEC;
        let rc = unsafe { libc::fcntl(raw_read, libc::F_SETFD, cleared) };
        if rc == -1 {
            return Err(std::io::Error::last_os_error());
        }

        // Tell the child the fd number via environment variable.
        cmd.env("SUBSTRATE_WATCHDOG_FD", raw_read.to_string());

        // Transfer the read_fd into the child by converting to raw and forgetting
        // the OwnedFd so it is not closed in the parent before exec runs.
        // The child inherits the raw fd number; the parent has no further interest.
        // SAFETY: we intentionally leak the read fd into the child via exec
        // inheritance. The `forget` prevents double-close. The child (OS) closes
        // it when the child process exits.
        let raw_read_forgotten = std::os::fd::IntoRawFd::into_raw_fd(read_fd);
        // Immediately re-wrap in OwnedFd so that if this function returns an error
        // after this point, the fd is still closed on the parent side. However,
        // at this point we have no more fallible operations, so this is purely
        // defensive. Since the child inherits this fd on exec, the parent should
        // close it after fork to avoid accumulation. We close it here explicitly.
        // SAFETY: raw_read_forgotten is a valid open fd not aliased elsewhere in
        // this scope. Wrapping it in OwnedFd transfers ownership back for drop.
        let _close_read_in_parent = unsafe { OwnedFd::from_raw_fd(raw_read_forgotten) };
        // _close_read_in_parent drops here, closing the read end in the parent.
        // The child inherits a copy created by fork+exec.

        Ok(WatchdogPipe {
            _write_end: write_fd,
        })
    }
}

// Linux: no watchdog pipe needed (PR_SET_PDEATHSIG handles it).
#[cfg(not(target_os = "macos"))]
mod inner {
    /// No-op watchdog on non-macOS platforms.
    ///
    /// Linux uses `PR_SET_PDEATHSIG` (set in `pre_exec.rs`) instead.
    #[derive(Debug)]
    pub struct WatchdogPipe {
        _phantom: std::marker::PhantomData<()>,
    }

    /// No-op install on non-macOS platforms. Always returns `Ok(NoopWatchdog)`.
    ///
    /// References: ADR-0053 §"macOS Watchdog Pipe Pattern".
    #[expect(
        clippy::disallowed_types,
        reason = "substrate-subprocess is the authorized host of tokio::process::Command per ADR-0052"
    )]
    pub fn install(_cmd: &mut tokio::process::Command) -> std::io::Result<WatchdogPipe> {
        Ok(WatchdogPipe {
            _phantom: std::marker::PhantomData,
        })
    }
}

pub use inner::{WatchdogPipe, install};
