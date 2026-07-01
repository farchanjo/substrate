//! Linux process scanner — reads `/proc/<pid>/stat`, `/proc/<pid>/status`,
//! and `/proc/<pid>/cmdline` via the `procfs` crate (no `unsafe` required).
//!
//! This module is compiled only on Linux (`#[cfg(target_os = "linux")]`).
//! No `unsafe` code is used; `procfs` wraps the kernel interface safely.
//!
//! # CPU% delta strategy
//!
//! `scan_all` is a synchronous interface so we cannot sleep between two samples
//! in the same call without blocking the thread. Instead we use **persistent
//! state**: the scanner remembers `(utime + stime, Instant)` per PID from the
//! previous call and computes the delta on the next call. The first call always
//! returns `0.0` for every process; subsequent calls return a meaningful value.
//!
//! The state is protected by a `Mutex` — contention is negligible because
//! `proc.list` is a human-paced tool call, not a tight hot loop.
//!
//! ## Prime mode (opt-in)
//!
//! Callers that need a non-zero CPU% on the very first call can construct the
//! scanner with [`LinuxProcessScanner::with_prime`]. When prime mode is enabled,
//! the first call to [`scan_all`] automatically takes two samples 100 ms apart
//! inside `spawn_blocking` so the async executor is not stalled. This adds
//! ~100 ms of latency to the first call only; every subsequent call is
//! zero-sleep as normal.
//!
//! The composition root (dispatcher) uses the default constructor (no prime) to
//! keep first-call latency low. Tests or callers that specifically need accurate
//! first-call CPU% should use `with_prime()`.
//!
//! # Process start time
//!
//! `/proc/<pid>/stat` field `starttime` is in clock ticks (jiffies) since boot,
//! NOT a Unix timestamp. To convert to epoch seconds:
//!
//! ```text
//! start_time_unix = boot_time_unix + (starttime / CLK_TCK)
//! ```
//!
//! `boot_time_unix` is derived from `/proc/uptime` (first field = seconds since
//! boot as a float) and `SystemTime::now()`. Both values are cached in
//! module-scoped `OnceLock`s to avoid repeated syscalls on every process entry.
//!
//! ## Container / PID-namespace caveat
//!
//! Inside a container, `/proc/uptime` reflects the container's uptime (from
//! the kernel's perspective of the container's PID namespace), not the host
//! uptime. `start_time_unix` for processes started before the container will
//! therefore be incorrect. This is a known limitation; no workaround is
//! implemented (out of scope per ADR-0003 / ADR-0044).

use std::collections::HashMap;
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use procfs::WithCurrentSystemInfo;
use procfs::process::{Process, all_processes};
use tracing::warn;

use super::ProcessScannerPort;
use crate::process_info::ProcessInfo;
use substrate_domain::{SubstrateError, SubstrateResult};

/// Cached boot time in seconds since the Unix epoch.
///
/// Populated on first call to [`boot_time_unix`]; subsequent calls return the
/// cached value without re-reading `/proc/uptime`.
static BOOT_TIME_UNIX: OnceLock<u64> = OnceLock::new();

/// Cached clock-tick frequency (`CLK_TCK`) in ticks per second.
///
/// Populated on first call to [`clk_tck`]; subsequent calls return the cached
/// value without calling into `procfs::ticks_per_second()` again.
static CLK_TCK: OnceLock<u64> = OnceLock::new();

/// Returns the Unix timestamp (seconds) of the system boot, cached after the
/// first successful read.
///
/// Reads `/proc/uptime` (first field = seconds since boot as a float) and
/// subtracts from `SystemTime::now()`. On read failure, logs a warning and
/// returns `0` (equivalent to the previous behaviour of leaving `start_time_unix`
/// as `None`).
fn boot_time_unix() -> u64 {
    *BOOT_TIME_UNIX.get_or_init(|| {
        read_boot_time_unix().unwrap_or_else(|e| {
            warn!(error = %e, "failed to read /proc/uptime; start_time_unix will be inaccurate");
            0
        })
    })
}

/// Reads `/proc/uptime` and computes the boot-epoch timestamp.
///
/// Returns an error string on any parse or I/O failure so the caller can log
/// it and fall back gracefully.
fn read_boot_time_unix() -> Result<u64, String> {
    let content =
        std::fs::read_to_string("/proc/uptime").map_err(|e| format!("read /proc/uptime: {e}"))?;
    let uptime_secs_str = content
        .split_whitespace()
        .next()
        .ok_or_else(|| "empty /proc/uptime".to_owned())?;
    let uptime_secs: f64 = uptime_secs_str
        .parse()
        .map_err(|e| format!("parse uptime float '{uptime_secs_str}': {e}"))?;
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("SystemTime before UNIX_EPOCH: {e}"))?
        .as_secs();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "/proc/uptime is always non-negative; floor() before the cast leaves no \
                  fractional part, and system uptime never approaches u64::MAX seconds"
    )]
    let uptime_secs_floor = uptime_secs.floor() as u64;
    Ok(now_secs.saturating_sub(uptime_secs_floor))
}

/// Returns the clock-tick frequency (`CLK_TCK`) in ticks per second, cached
/// after the first call.
///
/// Delegates to `procfs::ticks_per_second()` which reads `_SC_CLK_TCK` via
/// `sysconf(2)` internally. Guaranteed to return at least 1 to avoid
/// divide-by-zero if the kernel returns an unexpected value.
fn clk_tck() -> u64 {
    *CLK_TCK.get_or_init(|| procfs::ticks_per_second().max(1))
}

/// Converts a `starttime` field (ticks since boot) from `/proc/<pid>/stat` to
/// a Unix epoch timestamp in seconds.
///
/// Returns `None` when `boot_time_unix()` returned `0` (i.e., the boot-time
/// read failed) to preserve the previous undefined semantics.
fn starttime_to_unix(starttime_ticks: u64) -> Option<i64> {
    let boot = boot_time_unix();
    if boot == 0 {
        return None;
    }
    let hz = clk_tck();
    let start_unix = boot.saturating_add(starttime_ticks / hz);
    i64::try_from(start_unix).ok()
}

/// Per-process tick snapshot for CPU% delta calculation.
#[derive(Debug, Clone, Copy)]
struct TickSnapshot {
    /// Sum of `utime + stime` in scheduler ticks.
    ticks: u64,
    /// Wall-clock instant when the snapshot was taken.
    at: Instant,
}

/// Linux-specific process scanner backed by the `procfs` crate.
///
/// Maintains a snapshot from the previous `scan_all` call so subsequent calls
/// can compute a meaningful CPU% delta.
///
/// ## Prime mode
///
/// See the module-level documentation for the prime-mode trade-off. Construct
/// with [`LinuxProcessScanner::with_prime`] to opt in. The `Default` impl and
/// [`LinuxProcessScanner::new`] both produce a scanner with prime mode **off**.
#[derive(Debug)]
pub struct LinuxProcessScanner {
    /// Previous tick snapshot keyed by PID.
    ///
    /// Protected by a `Mutex` because `ProcessScannerPort::scan_all` takes
    /// `&self` (shared reference). Contention is negligible for this use case.
    last_sample: Mutex<HashMap<u32, TickSnapshot>>,
    /// Whether to take a two-sample warm-up on the very first `scan_all` call.
    ///
    /// When `true` the first call takes an extra ~100 ms but returns non-zero
    /// CPU% for active processes. Off by default to keep first-call latency low.
    prime_on_first_call: bool,
    /// Set to `false` after the first `scan_all` call completes.
    ///
    /// Uses `AtomicBool` so the check does not require taking the snapshot lock.
    first_call_pending: AtomicBool,
}

impl Default for LinuxProcessScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxProcessScanner {
    /// Constructs a new scanner instance with prime mode **disabled**.
    ///
    /// The first `scan_all` call returns `0.0` for all CPU% values; subsequent
    /// calls compute a meaningful delta. This is the default for the dispatcher.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_sample: Mutex::new(HashMap::new()),
            prime_on_first_call: false,
            first_call_pending: AtomicBool::new(true),
        }
    }

    /// Constructs a scanner with prime mode **enabled**.
    ///
    /// The first `scan_all` call takes two samples ~100 ms apart and returns a
    /// meaningful CPU% delta from the very first response. Use this in contexts
    /// where the caller cannot issue two sequential calls and still needs
    /// non-zero CPU data (e.g., one-shot diagnostic scripts).
    ///
    /// # Latency note
    ///
    /// Prime mode adds approximately 100 ms to the first call only. Every
    /// subsequent call returns immediately from the persistent snapshot state.
    #[must_use]
    pub fn with_prime() -> Self {
        Self {
            last_sample: Mutex::new(HashMap::new()),
            prime_on_first_call: true,
            first_call_pending: AtomicBool::new(true),
        }
    }

    /// Low-level single-sample pass: reads all `/proc/<pid>` entries and stores
    /// tick snapshots without computing CPU%.
    ///
    /// Called during prime mode to populate the baseline before the second
    /// sample. Returns an error only for systemic `/proc` failures.
    fn populate_baseline_sample(&self) -> SubstrateResult<()> {
        let all = all_processes().map_err(|e| SubstrateError::InternalError {
            reason: format!("failed to open /proc: {e}"),
            correlation_id: None,
        })?;

        let now = Instant::now();
        let mut new_sample: HashMap<u32, TickSnapshot> = HashMap::new();

        for proc in all.flatten() {
            if let Ok(stat) = proc.stat() {
                let current_ticks = stat.utime.saturating_add(stat.stime);
                new_sample.insert(
                    proc.pid().cast_unsigned(),
                    TickSnapshot {
                        ticks: current_ticks,
                        at: now,
                    },
                );
            }
        }

        let mut guard = self
            .last_sample
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = new_sample;
        drop(guard);
        Ok(())
    }
}

/// Converts a `procfs::process::Stat` state character to a string.
fn stat_state(state: char) -> String {
    String::from(state)
}

/// Reads a single `/proc/<pid>` entry into a `ProcessInfo`.
///
/// `prev` is the tick snapshot from the previous scan for this PID. When
/// `Some`, CPU% is computed as `(tick_delta / wall_delta_secs) / num_cpus * 100`.
/// When `None` (first call), CPU% is `0.0`.
///
/// Returns `None` if the process has exited by the time we read it — this is
/// normal and must be tolerated gracefully.
fn read_process(
    proc: &Process,
    prev: Option<TickSnapshot>,
    now: Instant,
    num_cpus: f32,
) -> Option<(ProcessInfo, TickSnapshot)> {
    let stat = proc.stat().ok()?;
    let status = proc.status().ok()?;

    // cmdline is best-effort; kernels truncate or restrict it for some processes.
    let command = proc
        .cmdline()
        .ok()
        .map(|args| args.join(" "))
        .unwrap_or_default();

    let rss_kb = stat.rss_bytes().get() / 1024;
    let vm_kb = stat.vsize / 1024;

    // Convert starttime (jiffies since boot) to Unix epoch seconds.
    // boot_time_unix() + (starttime / CLK_TCK) gives the absolute epoch.
    // Returns None only when /proc/uptime was unreadable (boot_time == 0).
    let start_time_unix: Option<i64> = starttime_to_unix(stat.starttime);

    // Total scheduler ticks consumed by this process (user + kernel).
    let current_ticks = stat.utime.saturating_add(stat.stime);
    let snapshot = TickSnapshot {
        ticks: current_ticks,
        at: now,
    };

    // CPU%: (tick_delta_secs / wall_secs) * 100, normalised across all CPUs.
    // First call always yields 0.0 because there is no baseline to compare.
    let cpu_pct = prev.map_or(0.0_f32, |p| {
        let wall_secs = now.duration_since(p.at).as_secs_f32();
        if wall_secs < f32::EPSILON {
            0.0
        } else {
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
            let tick_delta = current_ticks.saturating_sub(p.ticks) as f32;
            // Clamp to [0, 100 * num_cpus] to guard against spurious spikes.
            (tick_delta / tick_hz / wall_secs * 100.0).clamp(0.0, 100.0 * num_cpus)
        }
    });

    let pid = proc.pid().cast_unsigned();
    let info = ProcessInfo {
        pid,
        ppid: stat.ppid.cast_unsigned(),
        name: stat.comm.clone(),
        command,
        uid: status.ruid,
        gid: status.rgid,
        cpu_pct,
        rss_kb,
        vm_kb,
        start_time_unix,
        state: stat_state(stat.state),
    };
    Some((info, snapshot))
}

impl ProcessScannerPort for LinuxProcessScanner {
    fn scan_all(&self) -> SubstrateResult<Vec<ProcessInfo>> {
        // Prime mode: on the very first call, populate a baseline snapshot then
        // sleep 100 ms so the second `/proc` pass can compute a real CPU delta.
        // After the first call completes this branch is never entered again.
        //
        // Note: `scan_all` is called from `spawn_blocking` by the async adapter
        // (ADR-0003 Zone B), so the `thread::sleep` here does not block the
        // Tokio executor thread pool.
        if self.prime_on_first_call
            && self
                .first_call_pending
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            self.populate_baseline_sample()?;
            // TODO(perf): the 100 ms sleep here produces a meaningful CPU delta on
            // the very first scan but burns a `spawn_blocking` thread slot.
            // A better approach is to cache a startup-time baseline via
            // `std::sync::LazyLock` keyed to the boot-tick and diff against it,
            // eliminating the sleep entirely. Deferred because it requires
            // refactoring `populate_baseline_sample` to write into a shared
            // `LazyLock<Mutex<Sample>>` instead of `self.last_sample`, which is
            // a non-trivial change to scanner state ownership.
            std::thread::sleep(Duration::from_millis(100));
            // Fall through to the normal scan path; last_sample is now populated
            // so the delta computation will yield non-zero results.
        }

        let all = all_processes().map_err(|e| SubstrateError::InternalError {
            reason: format!("failed to open /proc: {e}"),
            correlation_id: None,
        })?;

        let now = Instant::now();
        // Logical CPU count; capped at 1 as a minimum to avoid divide-by-zero.
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

        // Take a snapshot of the previous sample under lock, then release
        // before doing the (potentially slow) /proc scan to minimise hold time.
        let prev_sample = {
            let guard = self
                .last_sample
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.clone()
        };

        let mut result = Vec::new();
        let mut new_sample: HashMap<u32, TickSnapshot> = HashMap::new();

        for entry in all {
            match entry {
                Ok(proc) => {
                    let pid = proc.pid().cast_unsigned();
                    let prev = prev_sample.get(&pid).copied();
                    if let Some((info, snap)) = read_process(&proc, prev, now, num_cpus) {
                        new_sample.insert(pid, snap);
                        result.push(info);
                    }
                },
                Err(e) => {
                    // A single process vanishing mid-scan is normal; log at
                    // warn so operators can spot unusual patterns without
                    // failing the entire scan.
                    warn!(error = %e, "skipping unreadable /proc entry");
                },
            }
        }

        // Store new snapshot for the next call.
        let mut guard = self
            .last_sample
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = new_sample;
        drop(guard);

        Ok(result)
    }
}

#[cfg(all(test, target_os = "linux"))]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_wrap,
    reason = "test module — panics and unwraps on assertion failure are the intended behavior"
)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{boot_time_unix, clk_tck, starttime_to_unix};
    use crate::scanner::ProcessScannerPort;
    use crate::scanner::linux::LinuxProcessScanner;

    /// Calling `boot_time_unix()` twice must return the identical cached value
    /// (the `OnceLock` must not recompute on the second call).
    #[test]
    fn boot_time_cached_once() {
        let first = boot_time_unix();
        let second = boot_time_unix();
        assert_eq!(
            first, second,
            "boot_time_unix must return a stable cached value"
        );
        // Must also be non-zero on a real Linux host with a readable /proc/uptime.
        assert!(
            first > 0,
            "boot_time_unix must be non-zero on a live Linux host"
        );
    }

    /// `clk_tck()` must return a positive value (the kernel always reports >= 1).
    #[test]
    fn clk_tck_returns_positive() {
        let hz = clk_tck();
        assert!(hz > 0, "CLK_TCK must be positive; got {hz}");
    }

    /// Given two synthetic starttime values `t1 < t2` (both in ticks), the
    /// resulting Unix timestamps must satisfy `ts2 >= ts1` (monotonic ordering
    /// is preserved through the linear conversion).
    #[test]
    fn starttime_conversion_monotonic() {
        // Force cache warm-up so both calls see the same boot_time / clk_tck.
        let _ = boot_time_unix();
        let _ = clk_tck();

        let t1_ticks: u64 = 1_000;
        let t2_ticks: u64 = 2_000;

        let ts1 = starttime_to_unix(t1_ticks).expect("t1 conversion must succeed");
        let ts2 = starttime_to_unix(t2_ticks).expect("t2 conversion must succeed");

        assert!(
            ts2 >= ts1,
            "later starttime ({t2_ticks} ticks) must produce a >= Unix ts ({ts2}) than earlier ({t1_ticks} ticks → {ts1})"
        );
    }

    /// `proc.list` on the live system must return the current process with a
    /// `start_time_unix` within ±60 seconds of `SystemTime::now()` (generous
    /// margin for slow CI environments) and strictly greater than zero.
    ///
    /// We look up the current PID in the scan result and inspect its
    /// `start_time_unix`. The current process was started very recently relative
    /// to the test run, so its start time must be close to `now`.
    #[test]
    fn proc_list_populates_start_time() {
        let scanner = LinuxProcessScanner::new();
        let processes = scanner.scan_all().expect("scan_all must succeed on Linux");

        let our_pid = std::process::id();
        let entry = processes
            .iter()
            .find(|p| p.pid == our_pid)
            .unwrap_or_else(|| panic!("current PID {our_pid} must appear in proc.list output"));

        let start_time = entry
            .start_time_unix
            .unwrap_or_else(|| panic!("start_time_unix must be Some for PID {our_pid}"));

        assert!(
            start_time > 0,
            "start_time_unix must be positive; got {start_time}"
        );

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after UNIX_EPOCH")
            .as_secs() as i64;

        // The test process was started at most 60 seconds ago (generous for CI).
        let delta = now_secs - start_time;
        assert!(
            (0..=60).contains(&delta),
            "start_time_unix delta from now should be 0..=60 s; got {delta} s \
             (start={start_time}, now={now_secs})"
        );
    }
}
