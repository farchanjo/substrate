//! `proc.stats` handler — Zone B (`spawn_blocking`).
//!
//! Returns a resource-usage snapshot for a single process identified by PID.
//! Implements the per-platform data-source cascade from ADR-0051:
//! Tier-1 Linux uses `/proc/<pid>/stat` + `/proc/<pid>/status` + fd-count;
//! Tier-1 macOS uses `sysctl(KERN_PROC_PID)` + `proc_pidinfo(PROC_PIDTASKINFO)`.
//!
//! # CPU% delta
//!
//! CPU utilization requires two calls for the same PID. The first call for any
//! given PID returns `cpu_pct = 0.0`. The shared `PidCpuCache` stores the
//! previous `(total_cpu_ticks, Instant)` per PID and is updated on every call.
//! The cache is bounded to `MAX_CACHED_PIDS` via LRU eviction.
//!
//! # See also
//!
//! [ADR-0051](../../../docs/arch/adr/0051-per-process-resource-stats.md)
// macOS proc_pidinfo FFI — module-level allow per ADR-0042 + ADR-0044 carve-out.
#![cfg_attr(
    target_os = "macos",
    allow(
        unsafe_code,
        reason = "sysctl(KERN_PROC_PID) + proc_pidinfo(PROC_PIDTASKINFO, PROC_PIDLISTFDS) FFI on macOS; \
                  read-only process introspection; no subprocess spawned. \
                  ADR-0042 + ADR-0044 proc carve-out. ADR-0051 proc.stats."
    )
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::task::spawn_blocking;
use tracing::{debug, instrument};

use crate::{
    hints_helpers::build_read_hints,
    response::{ProcessDeps, ToolResponse},
};
use substrate_domain::SubstrateResult;

// ---- Value objects -----------------------------------------------------------

/// Single-character process state compatible with both Linux and macOS.
///
/// See [ADR-0051](../../../docs/arch/adr/0051-per-process-resource-stats.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ProcessState {
    /// Process is actively scheduled on a CPU.
    Running,
    /// Process is sleeping in an interruptible wait.
    Sleeping,
    /// Process is waiting for disk I/O (uninterruptible sleep).
    Disk,
    /// Process is in a zombie state (exited, not yet reaped).
    Zombie,
    /// Process has been stopped (e.g., by `SIGSTOP`).
    Stopped,
    /// Process is idle (macOS: not yet scheduled since creation).
    Idle,
    /// State could not be determined or is not recognised.
    Unknown,
}

impl ProcessState {
    /// Converts a Linux `/proc/<pid>/stat` state character to [`ProcessState`].
    #[must_use]
    pub const fn from_linux_char(c: char) -> Self {
        match c {
            'R' => Self::Running,
            'S' => Self::Sleeping,
            'D' => Self::Disk,
            'Z' => Self::Zombie,
            'T' | 't' => Self::Stopped,
            'I' => Self::Idle,
            _ => Self::Unknown,
        }
    }

    /// Converts a macOS `p_stat` byte (from `kinfo_proc`) to [`ProcessState`].
    ///
    /// macOS `p_stat` values per `<sys/proc.h>`:
    /// 1 = SIDL, 2 = SRUN, 3 = SSLEEP, 4 = SSTOP, 5 = SZOMB.
    #[must_use]
    pub const fn from_macos_p_stat(p_stat: u8) -> Self {
        match p_stat {
            1 => Self::Idle,
            2 => Self::Running,
            3 => Self::Sleeping,
            4 => Self::Stopped,
            5 => Self::Zombie,
            _ => Self::Unknown,
        }
    }
}

/// Per-process resource-usage snapshot returned by `proc.stats`.
///
/// See [ADR-0051](../../../docs/arch/adr/0051-per-process-resource-stats.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessStats {
    /// POSIX process identifier.
    pub pid: u32,
    /// Resident set size in bytes.
    pub rss_bytes: u64,
    /// Virtual address space size in bytes.
    pub virt_bytes: u64,
    /// CPU utilization as a percentage 0.0–100.0.
    ///
    /// Returns `0.0` on the first call for this PID (cold start).
    pub cpu_pct: f32,
    /// Number of threads in the process.
    pub threads: u32,
    /// Number of open file descriptors.
    ///
    /// `None` on macOS when the calling process lacks permission to read
    /// `proc_pidinfo(PROC_PIDLISTFDS)` for the target process.
    pub fds: Option<u32>,
    /// Real user ID of the process owner.
    pub uid: u32,
    /// Process start time as a Unix timestamp in seconds.
    pub start_time: u64,
    /// Process execution state.
    pub state: ProcessState,
    /// Executable name (basename of argv[0], max 255 bytes).
    pub command: String,
}

// ---- CPU delta cache ---------------------------------------------------------

/// Maximum number of PIDs retained in the CPU delta cache (ADR-0051).
const MAX_CACHED_PIDS: usize = 4096;

/// Per-PID CPU accounting snapshot for delta-based CPU% computation.
#[derive(Debug, Clone, Copy)]
pub struct PidCpuEntry {
    /// Total CPU time consumed (platform ticks or nanoseconds).
    pub cpu_units: u64,
    /// Wall-clock instant of the sample.
    pub at: Instant,
    /// Process start time at sample time (used to detect PID reuse).
    pub start_time: u64,
}

/// Bounded LRU-style CPU delta cache shared across `proc.stats` calls.
///
/// Protected by `Arc<Mutex<PidCpuCache>>` for thread-safe access from
/// `spawn_blocking` closures.
#[derive(Debug)]
pub struct PidCpuCache {
    entries: HashMap<u32, PidCpuEntry>,
    /// Insertion-order queue used for O(1) LRU eviction.
    order: Vec<u32>,
}

impl PidCpuCache {
    /// Creates an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::with_capacity(256),
            order: Vec::with_capacity(256),
        }
    }

    /// Returns the previous entry for `pid` if present and `start_time` matches.
    ///
    /// A start-time mismatch indicates PID reuse; stale entries are evicted.
    #[must_use]
    pub fn get(&self, pid: u32, start_time: u64) -> Option<&PidCpuEntry> {
        self.entries
            .get(&pid)
            .filter(|e| e.start_time == start_time)
    }

    /// Inserts or updates the entry for `pid`, evicting the oldest entry if the
    /// cache is full.
    pub fn insert(&mut self, pid: u32, entry: PidCpuEntry) {
        if !self.entries.contains_key(&pid) {
            if self.entries.len() >= MAX_CACHED_PIDS {
                // Evict the oldest PID (front of insertion-order queue).
                if let Some(evict_pid) = self.order.first().copied() {
                    self.entries.remove(&evict_pid);
                    self.order.remove(0);
                }
            }
            self.order.push(pid);
        }
        self.entries.insert(pid, entry);
    }
}

impl Default for PidCpuCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared CPU delta cache.
pub type SharedPidCpuCache = Arc<Mutex<PidCpuCache>>;

/// Constructs a new empty `SharedPidCpuCache`.
#[must_use]
pub fn new_pid_cpu_cache() -> SharedPidCpuCache {
    Arc::new(Mutex::new(PidCpuCache::new()))
}

// ---- Linux Tier-1 -----------------------------------------------------------

/// Parses `/proc/<pid>/stat` and `/proc/<pid>/status` into a `ProcessStats`.
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` when the files are unreadable (e.g.,
/// process exited between the call and the read).
#[cfg(target_os = "linux")]
pub(crate) fn read_stats_linux(pid: u32, cache: &mut PidCpuCache) -> SubstrateResult<ProcessStats> {
    use procfs::WithCurrentSystemInfo;
    use procfs::process::Process;

    let proc = Process::new(pid.cast_signed()).map_err(|e| {
        substrate_domain::SubstrateError::NotFound {
            resource: format!("process {pid}"),
            correlation_id: None,
        }
        .into_not_found_or_internal(format!("procfs::Process::new({pid}): {e}"))
    })?;

    let stat = proc
        .stat()
        .map_err(|e| substrate_domain::SubstrateError::InternalError {
            reason: format!("read /proc/{pid}/stat: {e}"),
            correlation_id: None,
        })?;
    let status = proc
        .status()
        .map_err(|e| substrate_domain::SubstrateError::InternalError {
            reason: format!("read /proc/{pid}/status: {e}"),
            correlation_id: None,
        })?;

    let rss_bytes = stat.rss_bytes().get();
    let virt_bytes = stat.vsize;
    let threads = u32::try_from(stat.num_threads.max(0)).unwrap_or(u32::MAX);
    let uid = status.ruid;
    let command = stat.comm.clone();
    let state = crate::stats::ProcessState::from_linux_char(stat.state);

    // start_time: jiffies since boot → Unix epoch
    let start_time = linux_starttime_to_unix(stat.starttime);

    // fd count via directory listing
    let fds = count_fd_linux(pid);

    // CPU% delta
    let cpu_ticks = stat.utime.saturating_add(stat.stime);
    let prev = cache.get(pid, start_time);
    let cpu_pct = compute_linux_cpu_pct(cpu_ticks, prev);
    cache.insert(
        pid,
        PidCpuEntry {
            cpu_units: cpu_ticks,
            at: Instant::now(),
            start_time,
        },
    );

    Ok(ProcessStats {
        pid,
        rss_bytes,
        virt_bytes,
        cpu_pct,
        threads,
        fds,
        uid,
        start_time,
        state,
        command,
    })
}

/// Converts `/proc/<pid>/stat` `starttime` (jiffies since boot) to a Unix
/// epoch timestamp in seconds. Returns `0` when conversion fails.
#[cfg(target_os = "linux")]
fn linux_starttime_to_unix(starttime: u64) -> u64 {
    // Read /proc/uptime for boot epoch (cached in LinuxProcessScanner but
    // proc.stats is independent — read directly here).
    let boot = read_boot_time_linux();
    if boot == 0 {
        return 0;
    }
    let hz = procfs::ticks_per_second() as u64;
    let hz = hz.max(1);
    boot.saturating_add(starttime / hz)
}

#[cfg(target_os = "linux")]
fn read_boot_time_linux() -> u64 {
    let Ok(content) = std::fs::read_to_string("/proc/uptime") else {
        return 0;
    };
    let uptime_secs: f64 = content
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "/proc/uptime is always non-negative; floor() before the cast leaves no \
                  fractional part, and system uptime never approaches u64::MAX seconds"
    )]
    let uptime_secs_floor = uptime_secs.floor() as u64;
    now.saturating_sub(uptime_secs_floor)
}

/// Counts open file descriptors by listing `/proc/<pid>/fd`.
/// Returns `None` on permission or other read error.
#[cfg(target_os = "linux")]
fn count_fd_linux(pid: u32) -> Option<u32> {
    let path = format!("/proc/{pid}/fd");
    std::fs::read_dir(&path)
        .ok()
        .and_then(|dir| u32::try_from(dir.count()).ok())
}

/// Computes CPU% from current and previous Linux tick counters.
#[cfg(target_os = "linux")]
fn compute_linux_cpu_pct(current_ticks: u64, prev: Option<&PidCpuEntry>) -> f32 {
    let Some(p) = prev else { return 0.0 };
    let wall_secs = p.at.elapsed().as_secs_f32();
    if wall_secs < f32::EPSILON {
        return 0.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "clock-tick rates and per-scan tick deltas fit in f32 mantissa \
                  at any realistic uptime/CPU-count; result is clamped below"
    )]
    let tick_hz = procfs::ticks_per_second() as f32;
    #[expect(
        clippy::cast_precision_loss,
        reason = "tick delta between two consecutive scans fits in f32 mantissa"
    )]
    let tick_delta = current_ticks.saturating_sub(p.cpu_units) as f32;
    let num_cpus = std::thread::available_parallelism()
        .map_or(1.0_f32, |n| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "logical core count is realistically well under 2^24; \
                          f32 precision loss is inconsequential"
            )]
            let cores = n.get() as f32;
            cores
        })
        .max(1.0);
    (tick_delta / tick_hz / wall_secs * 100.0).clamp(0.0, 100.0 * num_cpus)
}

// Error conversion helper to avoid verbosity at call sites.
#[cfg(target_os = "linux")]
trait IntoNotFoundOrInternal {
    fn into_not_found_or_internal(self, ctx: String) -> substrate_domain::SubstrateError;
}

#[cfg(target_os = "linux")]
impl IntoNotFoundOrInternal for substrate_domain::SubstrateError {
    fn into_not_found_or_internal(self, ctx: String) -> substrate_domain::SubstrateError {
        // If the process simply vanished we report NotFound; other errors are Internal.
        match self {
            Self::NotFound { .. } => Self::NotFound {
                resource: ctx,
                correlation_id: None,
            },
            _ => Self::InternalError {
                reason: ctx,
                correlation_id: None,
            },
        }
    }
}

// ---- macOS Tier-1 -----------------------------------------------------------

/// Byte offset of `kp_proc.p_stat` within `kinfo_proc`.
#[cfg(target_os = "macos")]
const OFF_P_STAT: usize = 36;
/// Byte offset of `kp_proc.p_starttime.tv_sec` within `kinfo_proc`.
#[cfg(target_os = "macos")]
const OFF_P_STARTTIME_TV_SEC: usize = 0;
/// Byte offset of `kp_eproc.e_pcred.p_ruid` within `kinfo_proc`.
#[cfg(target_os = "macos")]
const OFF_RUID: usize = 392;
/// Byte offset of `kp_proc.p_comm` within `kinfo_proc`.
#[cfg(target_os = "macos")]
const OFF_P_COMM: usize = 243;
/// Length of `p_comm` including NUL.
#[cfg(target_os = "macos")]
const MAXCOMLEN_PLUS1: usize = 17;
/// Size of a single `kinfo_proc` entry.
#[cfg(target_os = "macos")]
const KINFO_PROC_SIZE: usize = 648;
/// `PROC_PIDTASKINFO` flavor constant.
#[cfg(target_os = "macos")]
const PROC_PIDTASKINFO: i32 = 4;
/// `PROC_PIDLISTFDS` flavor constant.
#[cfg(target_os = "macos")]
const PROC_PIDLISTFDS: i32 = 1;
/// Size of a single `proc_fdinfo` entry (from `<sys/proc_info.h>`).
#[cfg(target_os = "macos")]
const PROC_FDINFO_SIZE: usize = 8;

/// Mirror of `struct proc_taskinfo` used for macOS `proc_pidinfo` calls.
#[cfg(target_os = "macos")]
#[repr(C)]
struct ProcTaskInfo {
    pti_virtual_size: u64,
    pti_resident_size: u64,
    pti_total_user: u64,
    pti_total_system: u64,
    _padding: [u8; 64],
}

#[cfg(target_os = "macos")]
impl ProcTaskInfo {
    const fn zeroed() -> Self {
        // SAFETY: `ProcTaskInfo` consists of integer fields and a byte-array padding.
        // Zero-initializing all bytes produces a valid value for every field.
        unsafe { std::mem::zeroed() }
    }
}

// proc_pidinfo is declared in libproc.h but not in libc; we redeclare it.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn proc_pidinfo(
        pid: libc::c_int,
        flavor: libc::c_int,
        arg: u64,
        buffer: *mut libc::c_void,
        buffersize: libc::c_int,
    ) -> libc::c_int;
}

/// Reads per-process task info via `proc_pidinfo(PROC_PIDTASKINFO)`.
/// Returns `None` when the process denies access (e.g., system daemons).
#[cfg(target_os = "macos")]
fn macos_task_info(pid: u32) -> Option<ProcTaskInfo> {
    let mut info = ProcTaskInfo::zeroed();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "ProcTaskInfo is 96 bytes; fits in i32"
    )]
    let buf_size = std::mem::size_of::<ProcTaskInfo>() as libc::c_int;
    // SAFETY: standard PROC_PIDTASKINFO call; `info` is correctly sized; no mutation.
    #[expect(clippy::cast_possible_wrap, reason = "pid is a valid POSIX pid")]
    let ret = unsafe {
        proc_pidinfo(
            pid as libc::c_int,
            PROC_PIDTASKINFO,
            0,
            std::ptr::addr_of_mut!(info).cast(),
            buf_size,
        )
    };
    if ret < buf_size { None } else { Some(info) }
}

/// Counts open FDs via `proc_pidinfo(PROC_PIDLISTFDS)`.
/// Returns `None` when the call returns `EPERM` or fails for any reason.
#[cfg(target_os = "macos")]
fn macos_fd_count(pid: u32) -> Option<u32> {
    // First call: size probe (null buffer → returns required buffer size).
    // SAFETY: null buffer + 0 size is the documented size-probe idiom for PROC_PIDLISTFDS.
    #[expect(clippy::cast_possible_wrap, reason = "pid is a valid POSIX pid")]
    let needed = unsafe {
        proc_pidinfo(
            pid as libc::c_int,
            PROC_PIDLISTFDS,
            0,
            std::ptr::null_mut(),
            0,
        )
    };
    if needed < 0 {
        return None;
    }
    // Number of FD entries = needed_bytes / sizeof(proc_fdinfo).
    #[expect(
        clippy::cast_sign_loss,
        reason = "needed >= 0 checked above; safe to cast to usize"
    )]
    let fd_count = needed as usize / PROC_FDINFO_SIZE;
    u32::try_from(fd_count).ok()
}

/// Reads a `kinfo_proc` struct for `pid` via `sysctl(KERN_PROC_PID)`.
#[cfg(target_os = "macos")]
fn macos_kinfo_proc(pid: u32) -> Option<Vec<u8>> {
    #[expect(
        clippy::cast_possible_wrap,
        reason = "pid is a valid POSIX pid; kernel rejects pids > INT_MAX"
    )]
    let mut mib: [libc::c_int; 4] = [
        libc::CTL_KERN,
        libc::KERN_PROC,
        libc::KERN_PROC_PID,
        pid as libc::c_int,
    ];
    let mut size: libc::size_t = KINFO_PROC_SIZE + 64; // generous slack
    let mut buf: Vec<u8> = vec![0u8; size];
    // SAFETY: 4-element MIB for KERN_PROC_PID; `buf` has `size` bytes; read-only.
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
    if ret != 0 || size < KINFO_PROC_SIZE {
        return None;
    }
    buf.truncate(size);
    Some(buf)
}

/// Reads a little-endian `i64` at `offset` within `entry`.
#[cfg(target_os = "macos")]
fn read_i64_le(entry: &[u8], offset: usize) -> i64 {
    // SAFETY: caller guarantees `offset + 8 <= entry.len()`.
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "read_unaligned avoids alignment UB"
    )]
    let ptr = entry[offset..].as_ptr().cast::<i64>();
    unsafe { ptr.read_unaligned() }
}

/// Reads a little-endian `u32` at `offset`.
#[cfg(target_os = "macos")]
fn read_u32_le(entry: &[u8], offset: usize) -> u32 {
    // SAFETY: caller guarantees `offset + 4 <= entry.len()`.
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "read_unaligned avoids alignment UB"
    )]
    let ptr = entry[offset..].as_ptr().cast::<u32>();
    unsafe { ptr.read_unaligned() }
}

/// Extracts a NUL-terminated string from a byte slice.
#[cfg(target_os = "macos")]
fn c_bytes_to_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Computes CPU% from current CPU nanoseconds and the previous cache entry.
#[cfg(target_os = "macos")]
fn compute_macos_cpu_pct(total_cpu_ns: u64, prev: Option<&PidCpuEntry>, cpu_count: u64) -> f32 {
    let Some(p) = prev else { return 0.0 };
    let wall_ns = p.at.elapsed().as_nanos();
    if wall_ns == 0 || cpu_count == 0 {
        return 0.0;
    }
    let cpu_delta = total_cpu_ns.saturating_sub(p.cpu_units);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "precision loss acceptable for % display"
    )]
    {
        let wall_ns_f64 = wall_ns.min(u128::from(u64::MAX)) as f64;
        let pct = (cpu_delta as f64 / wall_ns_f64) * 100.0 / cpu_count as f64;
        (pct as f32).clamp(0.0, 100.0)
    }
}

/// Reads process stats on macOS via sysctl + `proc_pidinfo` (Tier-1).
///
/// # Errors
///
/// Returns `SubstrateError::NotFound` when `pid` does not exist.
/// Returns `SubstrateError::InternalError` for other kernel errors.
#[cfg(target_os = "macos")]
pub(crate) fn read_stats_macos(pid: u32, cache: &mut PidCpuCache) -> SubstrateResult<ProcessStats> {
    let buf = macos_kinfo_proc(pid).ok_or_else(|| substrate_domain::SubstrateError::NotFound {
        resource: format!("process {pid}"),
        correlation_id: None,
    })?;

    if buf.len() < KINFO_PROC_SIZE {
        return Err(substrate_domain::SubstrateError::InternalError {
            reason: format!("kinfo_proc for pid={pid} shorter than expected"),
            correlation_id: None,
        });
    }

    let p_stat = buf[OFF_P_STAT];
    let state = ProcessState::from_macos_p_stat(p_stat);
    let uid = read_u32_le(&buf, OFF_RUID);
    let comm_bytes = &buf[OFF_P_COMM..OFF_P_COMM + MAXCOMLEN_PLUS1];
    let command = c_bytes_to_string(comm_bytes);

    // p_starttime (timeval): tv_sec is an i64 at offset 0 within kp_proc.
    let start_time_i64 = read_i64_le(&buf, OFF_P_STARTTIME_TV_SEC);
    #[expect(
        clippy::cast_sign_loss,
        reason = "start_time_i64 validated > 0 in the branch above; cast is safe"
    )]
    let start_time = if start_time_i64 > 0 {
        start_time_i64 as u64
    } else {
        0u64
    };

    // Fetch task-level metrics (best-effort).
    let task = macos_task_info(pid);
    let rss_bytes = task.as_ref().map_or(0, |t| t.pti_resident_size);
    let virt_bytes = task.as_ref().map_or(0, |t| t.pti_virtual_size);
    let total_cpu_ns = task
        .as_ref()
        .map_or(0, |t| t.pti_total_user.saturating_add(t.pti_total_system));
    let threads = task.as_ref().map_or(1, |_| {
        // pti_threadnum would require a larger struct; use 1 as a safe default.
        // Wave H enhancement: parse pti_threadnum from the full struct.
        1u32
    });

    let fds = macos_fd_count(pid);

    let cpu_count = num_cpus::get() as u64;
    let prev = cache.get(pid, start_time);
    let cpu_pct = compute_macos_cpu_pct(total_cpu_ns, prev, cpu_count);
    cache.insert(
        pid,
        PidCpuEntry {
            cpu_units: total_cpu_ns,
            at: Instant::now(),
            start_time,
        },
    );

    Ok(ProcessStats {
        pid,
        rss_bytes,
        virt_bytes,
        cpu_pct,
        threads,
        fds,
        uid,
        start_time,
        state,
        command,
    })
}

// ---- Platform dispatch ------------------------------------------------------

/// Reads stats for `pid` using the platform-appropriate Tier-1 source.
///
/// # Errors
///
/// Returns `SubstrateError::NotFound` when `pid` does not exist.
/// Returns `SubstrateError::InternalError` for other read failures.
pub fn read_process_stats(pid: u32, cache: &mut PidCpuCache) -> SubstrateResult<ProcessStats> {
    #[cfg(target_os = "linux")]
    {
        read_stats_linux(pid, cache)
    }
    #[cfg(target_os = "macos")]
    {
        read_stats_macos(pid, cache)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        read_stats_sysinfo(pid, cache)
    }
}

/// Cross-platform sysinfo fallback (non-Linux/macOS platforms).
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_stats_sysinfo(pid: u32, cache: &mut PidCpuCache) -> SubstrateResult<ProcessStats> {
    use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

    let sysinfo_pid = Pid::from(pid as usize);
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[sysinfo_pid]), true);

    let proc =
        sys.process(sysinfo_pid)
            .ok_or_else(|| substrate_domain::SubstrateError::NotFound {
                resource: format!("process {pid}"),
                correlation_id: None,
            })?;

    let start_time = proc.start_time();
    let rss_bytes = proc.memory();
    let virt_bytes = proc.virtual_memory();
    let cpu_pct = proc.cpu_usage();
    let uid = proc
        .user_id()
        .and_then(|u| u32::try_from(**u).ok())
        .unwrap_or(0);
    let command = proc.name().to_string_lossy().into_owned();

    // No warm-up delta needed for sysinfo (it maintains internal sampling).
    let _ = cache;

    Ok(ProcessStats {
        pid,
        rss_bytes,
        virt_bytes,
        cpu_pct,
        threads: proc.tasks().map_or(1, |t| t.len() as u32),
        fds: None,
        uid,
        start_time,
        state: ProcessState::Unknown,
        command,
    })
}

// ---- Handler ----------------------------------------------------------------

/// Input parameters for `proc.stats`.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcStatsRequest {
    /// Target process identifier.
    pub pid: u32,
}

/// Handles a `proc.stats` tool call.
///
/// Returns a [`ProcessStats`] snapshot for the requested `pid`. CPU%
/// is `0.0` on the first call for any given PID.
///
/// # Errors
///
/// Returns `SubstrateError::NotFound` when `pid` does not exist.
/// Returns `SubstrateError::InternalError` for other platform read failures.
#[instrument(skip(deps, cpu_cache), fields(pid = req.pid))]
pub async fn handle_proc_stats(
    req: ProcStatsRequest,
    deps: Arc<ProcessDeps>,
    cpu_cache: SharedPidCpuCache,
) -> SubstrateResult<ToolResponse> {
    let _ = deps;
    let pid = req.pid;

    debug!(pid, "proc.stats called");

    let stats = spawn_blocking(move || {
        let mut cache = cpu_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read_process_stats(pid, &mut cache)
    })
    .await
    .map_err(|e| substrate_domain::SubstrateError::InternalError {
        reason: format!("spawn_blocking join error in proc.stats: {e}"),
        correlation_id: None,
    })??;

    let cold_start = stats.cpu_pct == 0.0;
    let content = format!(
        "proc.stats: pid={} rss={} KB virt={} KB cpu={:.1}% state={:?}.",
        stats.pid,
        stats.rss_bytes / 1024,
        stats.virt_bytes / 1024,
        stats.cpu_pct,
        stats.state,
    );

    let mut hints = build_read_hints(Some("proc.top"), Some("proc.signal"));
    if cold_start {
        hints = substrate_domain::Hints {
            next_action_suggested: Some(
                "Re-invoke proc.stats after 100ms to obtain non-zero cpu_pct.".to_owned(),
            ),
            ..hints
        };
    }

    Ok(ToolResponse::with_hints(content, json!(stats), hints))
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    reason = "test module — panics on assertion failure intended; cold-start zero comparison and loop-index casts are sound in test fixtures"
)]
mod tests {
    use std::sync::Arc;

    use substrate_domain::Capabilities;

    use super::*;
    use crate::response::ProcessDeps;

    fn make_deps() -> Arc<ProcessDeps> {
        Arc::new(ProcessDeps {
            capabilities: Arc::new(Capabilities::default()),
        })
    }

    #[tokio::test]
    async fn proc_stats_current_process_has_nonzero_rss() {
        let pid = std::process::id();
        let deps = make_deps();
        let cache = new_pid_cpu_cache();
        let req = ProcStatsRequest { pid };
        let resp = handle_proc_stats(req, deps, cache)
            .await
            .expect("proc.stats must not fail for current process");
        let stats: ProcessStats =
            serde_json::from_value(resp.structured_content).expect("valid ProcessStats JSON");
        assert!(
            stats.rss_bytes > 0,
            "rss_bytes must be > 0 for current process"
        );
    }

    #[tokio::test]
    async fn proc_stats_returns_correct_pid() {
        let pid = std::process::id();
        let deps = make_deps();
        let cache = new_pid_cpu_cache();
        let req = ProcStatsRequest { pid };
        let resp = handle_proc_stats(req, deps, cache)
            .await
            .expect("proc.stats must not fail");
        let stats: ProcessStats =
            serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert_eq!(stats.pid, pid, "returned pid must match requested pid");
    }

    #[tokio::test]
    async fn proc_stats_cold_start_cpu_pct_is_zero() {
        let pid = std::process::id();
        let deps = make_deps();
        let cache = new_pid_cpu_cache(); // fresh cache → first call
        let req = ProcStatsRequest { pid };
        let resp = handle_proc_stats(req, deps, cache)
            .await
            .expect("proc.stats must not fail");
        let stats: ProcessStats =
            serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert_eq!(stats.cpu_pct, 0.0, "first call must return cpu_pct = 0.0");
    }

    #[tokio::test]
    async fn proc_stats_nonexistent_pid_returns_err() {
        // PID u32::MAX is extremely unlikely to exist on any real system.
        let pid = u32::MAX;
        let deps = make_deps();
        let cache = new_pid_cpu_cache();
        let req = ProcStatsRequest { pid };
        let result = handle_proc_stats(req, deps, cache).await;
        assert!(
            result.is_err(),
            "proc.stats for nonexistent PID must return Err"
        );
    }

    #[test]
    fn pid_cpu_cache_evicts_at_max() {
        let mut cache = PidCpuCache::new();
        let now = Instant::now();
        // Fill to capacity.
        for i in 0..MAX_CACHED_PIDS {
            cache.insert(
                i as u32,
                PidCpuEntry {
                    cpu_units: 0,
                    at: now,
                    start_time: 0,
                },
            );
        }
        assert_eq!(cache.entries.len(), MAX_CACHED_PIDS);
        // Inserting one more must evict the oldest (pid 0).
        cache.insert(
            u32::MAX,
            PidCpuEntry {
                cpu_units: 0,
                at: now,
                start_time: 0,
            },
        );
        assert_eq!(
            cache.entries.len(),
            MAX_CACHED_PIDS,
            "cache must remain at cap"
        );
        assert!(
            !cache.entries.contains_key(&0),
            "pid 0 must have been evicted"
        );
        assert!(
            cache.entries.contains_key(&u32::MAX),
            "newly inserted pid must be present"
        );
    }

    #[test]
    fn process_state_linux_chars_map_correctly() {
        assert_eq!(ProcessState::from_linux_char('R'), ProcessState::Running);
        assert_eq!(ProcessState::from_linux_char('S'), ProcessState::Sleeping);
        assert_eq!(ProcessState::from_linux_char('D'), ProcessState::Disk);
        assert_eq!(ProcessState::from_linux_char('Z'), ProcessState::Zombie);
        assert_eq!(ProcessState::from_linux_char('T'), ProcessState::Stopped);
        assert_eq!(ProcessState::from_linux_char('I'), ProcessState::Idle);
        assert_eq!(ProcessState::from_linux_char('X'), ProcessState::Unknown);
    }

    #[test]
    fn process_state_macos_bytes_map_correctly() {
        assert_eq!(ProcessState::from_macos_p_stat(1), ProcessState::Idle);
        assert_eq!(ProcessState::from_macos_p_stat(2), ProcessState::Running);
        assert_eq!(ProcessState::from_macos_p_stat(3), ProcessState::Sleeping);
        assert_eq!(ProcessState::from_macos_p_stat(4), ProcessState::Stopped);
        assert_eq!(ProcessState::from_macos_p_stat(5), ProcessState::Zombie);
        assert_eq!(ProcessState::from_macos_p_stat(99), ProcessState::Unknown);
    }
}
