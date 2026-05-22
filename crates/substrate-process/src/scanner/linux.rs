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

use std::collections::HashMap;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use procfs::process::{Process, all_processes};
use tracing::warn;

use super::ProcessScannerPort;
use crate::process_info::ProcessInfo;
use substrate_domain::{SubstrateError, SubstrateResult};

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

        for entry in all {
            if let Ok(proc) = entry {
                if let Ok(stat) = proc.stat() {
                    let current_ticks = stat.utime.saturating_add(stat.stime);
                    new_sample.insert(
                        proc.pid() as u32,
                        TickSnapshot {
                            ticks: current_ticks,
                            at: now,
                        },
                    );
                }
            }
        }

        let mut guard = self
            .last_sample
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = new_sample;
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

    let rss_kb = stat.rss_bytes().ok().map(|b| b / 1024).unwrap_or(0) as u64;
    let vm_kb = stat.vsize / 1024;

    // start_time: ticks since boot → not yet converted to epoch seconds in MVP.
    // TODO Wave G: convert stat.starttime (jiffies) via sysconf(_SC_CLK_TCK)
    // and /proc/stat btime to absolute Unix epoch.
    let start_time_unix: Option<i64> = None;

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
            let tick_hz = procfs::ticks_per_second() as f32;
            let tick_delta = current_ticks.saturating_sub(p.ticks) as f32;
            // Clamp to [0, 100 * num_cpus] to guard against spurious spikes.
            (tick_delta / tick_hz / wall_secs * 100.0).clamp(0.0, 100.0 * num_cpus)
        }
    });

    let pid = proc.pid() as u32;
    let info = ProcessInfo {
        pid,
        ppid: stat.ppid as u32,
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
            .map(|n| n.get() as f32)
            .unwrap_or(1.0)
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
                    let pid = proc.pid() as u32;
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

        Ok(result)
    }
}
