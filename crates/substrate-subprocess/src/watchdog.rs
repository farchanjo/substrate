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
    use std::os::fd::OwnedFd;

    /// macOS watchdog pipe.
    ///
    /// Holds the write end (kept alive for the lifetime of the child so the EOF
    /// notification is delivered when substrate exits) and, transiently, the
    /// parent's copy of the read end.
    ///
    /// The read end MUST stay open in the parent until AFTER `cmd.spawn()` forks,
    /// otherwise the child never inherits an open read end and the EOF-on-parent
    /// -death mechanism (ADR-0053) is silently non-functional. The caller invokes
    /// [`WatchdogPipe::notify_spawned`] immediately after `cmd.spawn()` returns to
    /// drop the parent's read-end copy; from that point only the child holds the
    /// read end, so closing the write end delivers EOF to the child alone.
    #[derive(Debug)]
    pub struct WatchdogPipe {
        /// Write end of the watchdog pipe. Kept alive as long as this struct
        /// exists. Dropped (closed) when substrate exits or when the
        /// `ChildHandle` is released, delivering EOF to the child's read end.
        _write_end: OwnedFd,

        /// Parent's copy of the read end. Held open across `cmd.spawn()` so the
        /// child inherits a valid, open read end at exec time. Dropped by
        /// [`WatchdogPipe::notify_spawned`] once the fork has occurred.
        read_end_parent_copy: Option<OwnedFd>,
    }

    impl WatchdogPipe {
        /// Closes the parent's copy of the read end after the child has been forked.
        ///
        /// Call exactly once, immediately after `cmd.spawn()` returns. Before this
        /// call the parent keeps the read end open so the fork inherits it; after
        /// it, only the child holds the read end and closing the write end (on
        /// substrate exit / `WatchdogPipe` drop) delivers a clean EOF to the child.
        ///
        /// Idempotent: a second call is a no-op.
        pub fn notify_spawned(&mut self) {
            // Dropping the OwnedFd closes the parent's read-end copy.
            self.read_end_parent_copy = None;
        }
    }

    /// Installs the watchdog pipe on `cmd`.
    ///
    /// Creates a `pipe(2)`, marks the read end as non-CLOEXEC so the child
    /// inherits it, sets `SUBSTRATE_WATCHDOG_FD` in the child's environment to the
    /// read-end fd number, and returns a [`WatchdogPipe`] that keeps BOTH ends open
    /// in the parent until [`WatchdogPipe::notify_spawned`] is called post-spawn.
    ///
    /// Keeping the read end open across the spawn is required: the child inherits
    /// the fd at fork/exec, so the `SUBSTRATE_WATCHDOG_FD` number must reference a
    /// still-open descriptor when exec runs.
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

        // Tell the child the fd number via environment variable. This number stays
        // valid at exec time because `read_fd` is retained in `read_end_parent_copy`
        // until `notify_spawned` is called AFTER `cmd.spawn()` forks the child.
        cmd.env("SUBSTRATE_WATCHDOG_FD", raw_read.to_string());

        Ok(WatchdogPipe {
            _write_end: write_fd,
            read_end_parent_copy: Some(read_fd),
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

    impl WatchdogPipe {
        /// No-op on non-macOS platforms (Linux uses `PR_SET_PDEATHSIG`).
        ///
        /// Present so the spawn path can call `notify_spawned` unconditionally
        /// after `cmd.spawn()` regardless of platform.
        #[inline]
        pub const fn notify_spawned(&mut self) {}
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
