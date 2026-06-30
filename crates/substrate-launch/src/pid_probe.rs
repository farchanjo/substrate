//! Single-pid start-time + parent-pid probe for the PID-recycle guard (ADR-0068).
//!
//! The reaper compares a recorded child's start-time against a fresh read: when
//! the kernel recycles a dead pid onto an unrelated process the start-times
//! differ, so the recorded child is gone and a stranger now holds its pid
//! (ADR-0068 §"Reaper on boot"). The start-time is an opaque platform scalar
//! compared only for equality — never converted to a wall-clock instant:
//!
//! - Linux: `/proc/<pid>/stat` field 22 (`starttime`), clock-ticks-since-boot.
//! - macOS: `kinfo_proc.kp_proc.p_starttime.tv_sec` (Unix epoch seconds).
//!
//! A `None` return is also the liveness signal: it is equivalent to
//! `kill(pid, 0)` returning `ESRCH`, since a gone process has no `/proc` entry
//! (Linux) and yields zero `kinfo_proc` bytes (macOS). Both reads are blocking
//! syscalls, so callers run them on the blocking pool (async zone B per ADR-0003).
//!
//! References: ADR-0068.

/// A single process's recycle-guard fields, read in one syscall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PidStat {
    /// Opaque platform start-time scalar, compared only for equality.
    pub start_time: u64,
    /// Parent pid, used to detect reparenting away from the supervisor.
    pub ppid: i32,
}

/// Reads `pid`'s start-time and parent pid, or `None` when the process is gone
/// or unreadable.
#[cfg(target_os = "linux")]
pub(crate) fn read_pid_stat(pid: i32) -> Option<PidStat> {
    let stat = procfs::process::Process::new(pid).ok()?.stat().ok()?;
    Some(PidStat {
        start_time: stat.starttime,
        ppid: stat.ppid,
    })
}

/// Reads `pid`'s start-time and parent pid, or `None` when the process is gone
/// or unreadable.
#[cfg(target_os = "macos")]
pub(crate) fn read_pid_stat(pid: i32) -> Option<PidStat> {
    macos::read_pid_stat(pid)
}

#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "sysctl(KERN_PROC_PID) + kinfo_proc raw-byte parsing on macOS; \
              read-only kernel query, no subprocess spawned (ADR-0042/0044 proc carve-out)"
)]
mod macos {
    use super::PidStat;

    /// Size of a single `kinfo_proc` entry (macOS SDK 14, arm64 + `x86_64`).
    ///
    /// Re-derived locally rather than depending on the sibling `substrate-process`
    /// adapter (hexagonal layering forbids a sibling-adapter dependency); the
    /// constant is verified by the same C probe documented in
    /// `substrate-process/src/scanner/macos.rs`.
    const KINFO_PROC_SIZE: usize = 648;

    /// Byte offset of `kp_proc.p_starttime.tv_sec` (`i64`) within `kinfo_proc`.
    /// `extern_proc` begins at `kinfo_proc` offset 0 and `p_starttime` is its
    /// first field.
    const OFF_STARTTIME_TV_SEC: usize = 0;

    /// Byte offset of `kp_eproc.e_ppid` (`i32`) within `kinfo_proc`
    /// (`296 + 264 == 560`).
    const OFF_E_PPID: usize = 560;

    /// Reads one process's start-time and parent pid via a single
    /// `sysctl(KERN_PROC_PID)` call into a fixed-size `kinfo_proc` buffer.
    pub(super) fn read_pid_stat(pid: i32) -> Option<PidStat> {
        let buf = sysctl_kinfo_proc(pid)?;
        let tv_sec = read_i64_le(&buf, OFF_STARTTIME_TV_SEC)?;
        if tv_sec <= 0 {
            return None;
        }
        let parent = read_i32_le(&buf, OFF_E_PPID)?;
        #[expect(clippy::cast_sign_loss, reason = "tv_sec validated > 0 above")]
        Some(PidStat {
            start_time: tv_sec as u64,
            ppid: parent,
        })
    }

    /// Fetches one fixed-size `kinfo_proc` for `pid`. Returns `None` when the
    /// process does not exist (the kernel reports fewer than one full entry).
    fn sysctl_kinfo_proc(pid: i32) -> Option<Vec<u8>> {
        let mut mib: [libc::c_int; 4] =
            [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
        let mut buf = vec![0u8; KINFO_PROC_SIZE];
        let mut size: libc::size_t = KINFO_PROC_SIZE;

        // SAFETY: a single read-only KERN_PROC_PID query into a fixed
        // sizeof(kinfo_proc) buffer. `mib` is a valid 4-element MIB live for the
        // call; `size` is updated to the number of bytes written; null
        // `newp`/`newlen` mark a read-only query. No pointer escapes this frame.
        let ret = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                4,
                buf.as_mut_ptr().cast(),
                std::ptr::addr_of_mut!(size),
                std::ptr::null_mut(),
                0,
            )
        };

        if ret < 0 || size < KINFO_PROC_SIZE {
            return None;
        }
        Some(buf)
    }

    /// Reads a little-endian `i64` at `offset`, or `None` when out of bounds.
    ///
    /// Uses `from_le_bytes` (all macOS targets are little-endian), which needs no
    /// pointer cast and therefore no `unsafe`.
    fn read_i64_le(buf: &[u8], offset: usize) -> Option<i64> {
        let bytes: [u8; 8] = buf.get(offset..offset + 8)?.try_into().ok()?;
        Some(i64::from_le_bytes(bytes))
    }

    /// Reads a little-endian `i32` at `offset`, or `None` when out of bounds.
    fn read_i32_le(buf: &[u8], offset: usize) -> Option<i32> {
        let bytes: [u8; 4] = buf.get(offset..offset + 4)?.try_into().ok()?;
        Some(i32::from_le_bytes(bytes))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use super::*;

    #[test]
    fn reads_own_process_start_time_and_ppid() {
        let pid = i32::try_from(std::process::id()).expect("test pid fits in i32");
        let stat = read_pid_stat(pid).expect("current process must be readable");
        assert!(stat.start_time > 0, "start_time must be non-zero; got {}", stat.start_time);
        assert!(stat.ppid > 0, "ppid must be a real parent pid; got {}", stat.ppid);
    }

    #[test]
    fn unlikely_pid_reads_as_gone() {
        // A pid near i32::MAX is overwhelmingly unlikely to be live; the probe
        // must report it as gone (None) rather than fabricating a value.
        assert!(read_pid_stat(i32::MAX).is_none(), "i32::MAX pid must read as gone");
    }

    #[test]
    fn second_read_is_stable_for_same_process() {
        let pid = i32::try_from(std::process::id()).expect("test pid fits in i32");
        let first = read_pid_stat(pid).expect("first read");
        let second = read_pid_stat(pid).expect("second read");
        assert_eq!(
            first.start_time, second.start_time,
            "a live process's start-time must be stable across reads (the recycle-guard invariant)"
        );
    }
}
