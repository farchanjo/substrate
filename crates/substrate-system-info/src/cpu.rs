//! `sys.cpu` handler — Zone B (`spawn_blocking`; file I/O on Linux, sysctl on macOS).
//!
//! Returns a CPU topology and utilization snapshot including logical/physical core
//! counts, per-core load percentages, frequency, and temperature. Implements the
//! tier-cascade pattern from ADR-0042.
//!
//! # Per-core load sampling
//!
//! CPU utilization requires two consecutive readings. The first call after process
//! startup returns zeros for all `per_core_load` entries and sets `cold_start: true`
//! in the structured hints. On every subsequent call the delta between the cached
//! snapshot and the current kernel counters is used to compute load percentages.
//!
//! The snapshot is protected by an `Arc<Mutex<Option<CpuSnapshot>>>` shared across
//! calls. This is initialized lazily on the first `sys.cpu` call.
//!
//! # Platform strategy
//!
//! - **Linux (Tier 1)**: logical cores via `nix::unistd::sysconf(_NPROCESSORS_ONLN)`;
//!   physical cores from `/proc/cpuinfo` `cpu cores` field; per-core load via
//!   `/proc/stat` delta; frequency via `/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq`;
//!   temperature via `/sys/class/thermal/thermal_zone0/temp` when present.
//! - **macOS (Tier 1)**: logical cores via `sysctl hw.logicalcpu`; physical cores via
//!   `sysctl hw.physicalcpu`; per-core load via `host_processor_info(PROCESSOR_CPU_LOAD_INFO)`;
//!   frequency via `sysctl hw.cpufrequency` (not available on Apple Silicon — returns `None`);
//!   temperature: not available through public macOS APIs — always `None`.
//!
//! # See also
//!
//! [ADR-0050](../../../docs/arch/adr/0050-system-resource-monitoring.md)
// macOS sysctl + mach processor_info FFI — module-level allow per ADR-0042 + ADR-0044 carve-out.
#![cfg_attr(
    target_os = "macos",
    allow(
        unsafe_code,
        reason = "sysctl(hw.logicalcpu, hw.physicalcpu, hw.cpufrequency) + \
                  mach host_processor_info(PROCESSOR_CPU_LOAD_INFO) FFI on macOS; \
                  read-only syscalls. ADR-0042 + ADR-0044 sysctl/mach carve-out. ADR-0050 sys.cpu."
    )
)]

use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::task::spawn_blocking;
use tracing::{debug, instrument};

use crate::{
    hints_helpers::build_info_hints,
    response::{SystemInfoDeps, ToolResponse},
};
use substrate_domain::{Hints, SubstrateResult};

// ---- Value objects -----------------------------------------------------------

/// Per-core CPU utilization snapshot, used internally for delta computation.
///
/// Protected by `Arc<Mutex<Option<CpuSnapshot>>>` to allow sharing across
/// async `sys.cpu` calls without locking the executor.
#[derive(Debug, Clone)]
pub struct CpuSnapshot {
    /// Sum of (user + nice + system) ticks per logical core at the sample time.
    pub user_nice_sys: Vec<u64>,
    /// Sum of (user + nice + system + idle + iowait) ticks per core at sample time.
    pub total: Vec<u64>,
    /// Wall-clock instant of the snapshot.
    pub at: Instant,
}

/// CPU topology and utilization snapshot returned by `sys.cpu`.
///
/// See [ADR-0050](../../../docs/arch/adr/0050-system-resource-monitoring.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuStats {
    /// Number of logical CPUs (hardware threads) visible to the OS scheduler.
    pub logical_cores: u32,
    /// Number of physical cores; `None` when unavailable on this platform.
    pub physical_cores: Option<u32>,
    /// Current frequency in MHz of the first logical core, or aggregate average.
    /// `None` when unavailable (e.g., Apple Silicon).
    pub freq_mhz: Option<u64>,
    /// Per-logical-core utilization in the range 0.0–100.0.
    ///
    /// Contains exactly `logical_cores` entries. All entries are `0.0` on the
    /// first call after process startup (`cold_start: true` in hints).
    pub per_core_load: Vec<f32>,
    /// CPU package temperature in Celsius where available; `None` otherwise.
    ///
    /// Always `None` on macOS (not available through public APIs).
    pub temperature_c: Option<f32>,
    /// Data-source tier tag per ADR-0050.
    ///
    /// Values: `"linux-proc"`, `"macos-sysctl"`, `"sysinfo"`.
    pub platform_tier: String,
}

// ---- Linux Tier-1 -----------------------------------------------------------

/// Reads a single integer from a sysfs file, returning `None` on any error.
#[cfg(target_os = "linux")]
fn read_sysfs_u64(path: &str) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Parses `/proc/stat` to extract per-core tick counters.
///
/// Returns `(user_nice_sys, total)` as two parallel vectors indexed by logical
/// core number. Skips the aggregate `cpu` line; only `cpu0`, `cpu1`, … entries
/// are used.
///
/// # Errors
///
/// Returns `Err` when `/proc/stat` is unreadable or contains no per-core lines.
#[cfg(target_os = "linux")]
fn parse_proc_stat() -> SubstrateResult<(Vec<u64>, Vec<u64>)> {
    let content = std::fs::read_to_string("/proc/stat").map_err(|e| {
        substrate_domain::SubstrateError::InternalError {
            reason: format!("read /proc/stat failed: {e}"),
            correlation_id: None,
        }
    })?;

    let mut user_nice_sys: Vec<u64> = Vec::new();
    let mut total: Vec<u64> = Vec::new();

    for line in content.lines() {
        // Match per-core lines: "cpu0 <user> <nice> <sys> <idle> <iowait> ..."
        if !line.starts_with("cpu") || line.starts_with("cpu ") {
            continue;
        }
        let fields: Vec<u64> = line
            .split_whitespace()
            .skip(1) // skip the "cpuN" label
            .map(|s| s.parse().unwrap_or(0))
            .collect();

        // fields: [user, nice, sys, idle, iowait, irq, softirq, steal, guest, guest_nice]
        let u = fields.first().copied().unwrap_or(0);
        let n = fields.get(1).copied().unwrap_or(0);
        let s = fields.get(2).copied().unwrap_or(0);
        let idle = fields.get(3).copied().unwrap_or(0);
        let iowait = fields.get(4).copied().unwrap_or(0);

        user_nice_sys.push(u.saturating_add(n).saturating_add(s));
        total.push(
            u.saturating_add(n)
                .saturating_add(s)
                .saturating_add(idle)
                .saturating_add(iowait),
        );
    }

    if total.is_empty() {
        return Err(substrate_domain::SubstrateError::InternalError {
            reason: "no per-core cpu lines found in /proc/stat".to_owned(),
            correlation_id: None,
        });
    }

    Ok((user_nice_sys, total))
}

/// Reads logical core count via `nix::unistd::sysconf(_NPROCESSORS_ONLN)`.
#[cfg(target_os = "linux")]
fn linux_logical_cores() -> u32 {
    nix::unistd::sysconf(nix::unistd::SysconfVar::_NPROCESSORS_ONLN)
        .ok()
        .flatten()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(1)
}

/// Reads physical core count from `/proc/cpuinfo` field `cpu cores`.
#[cfg(target_os = "linux")]
fn linux_physical_cores() -> Option<u32> {
    let content = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("cpu cores") {
            let val: u32 = rest.trim_start_matches([' ', '\t', ':']).parse().ok()?;
            return Some(val);
        }
    }
    None
}

/// Reads CPU frequency in MHz from sysfs `scaling_cur_freq` (kHz → MHz).
#[cfg(target_os = "linux")]
fn linux_freq_mhz() -> Option<u64> {
    // scaling_cur_freq reports kHz
    read_sysfs_u64("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq")
        .map(|khz| khz / 1000)
        .or_else(|| {
            // cpuinfo_cur_freq is an alternative on some kernels
            read_sysfs_u64("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_cur_freq")
                .map(|khz| khz / 1000)
        })
}

/// Reads CPU temperature from `/sys/class/thermal/thermal_zone0/temp`.
///
/// The value is in millidegrees Celsius; divided by 1000 to yield degrees.
/// Returns `None` when the path does not exist or the read fails.
#[cfg(target_os = "linux")]
fn linux_temperature_c() -> Option<f32> {
    let milli = read_sysfs_u64("/sys/class/thermal/thermal_zone0/temp")?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "millidegree integer converted to f32; precision loss < 0.001 C is irrelevant"
    )]
    let celsius = milli as f32 / 1000.0;
    Some(celsius)
}

/// Reads CPU stats on Linux via procfs + sysfs (Tier-1).
///
/// `prev` is the snapshot from the previous call; if `None` (first call),
/// `per_core_load` will be all zeros.
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` when `/proc/stat` is unreadable.
#[cfg(target_os = "linux")]
pub(crate) fn read_cpu_linux(
    prev: Option<&CpuSnapshot>,
) -> SubstrateResult<(CpuStats, CpuSnapshot)> {
    let logical_cores = linux_logical_cores();
    let physical_cores = linux_physical_cores();
    let freq_mhz = linux_freq_mhz();
    let temperature_c = linux_temperature_c();

    let (user_nice_sys, total) = parse_proc_stat()?;
    let now = Instant::now();

    let per_core_load = prev.map_or_else(
        || vec![0.0_f32; user_nice_sys.len()],
        |p| {
            user_nice_sys
                .iter()
                .zip(total.iter())
                .zip(p.user_nice_sys.iter().zip(p.total.iter()))
                .map(|((cur_busy, cur_total), (prev_busy, prev_total))| {
                    let delta_total = cur_total.saturating_sub(*prev_total);
                    let delta_busy = cur_busy.saturating_sub(*prev_busy);
                    if delta_total == 0 {
                        0.0_f32
                    } else {
                        #[expect(
                            clippy::cast_precision_loss,
                            clippy::cast_possible_truncation,
                            reason = "tick counts fit in f64 mantissa at any realistic core count; \
                                      result is clamped to 0.0..=100.0 so f32 truncation is inconsequential"
                        )]
                        let pct = (delta_busy as f64 / delta_total as f64 * 100.0) as f32;
                        pct.clamp(0.0, 100.0)
                    }
                })
                .collect::<Vec<f32>>()
        },
    );

    let snapshot = CpuSnapshot {
        user_nice_sys,
        total,
        at: now,
    };

    Ok((
        CpuStats {
            logical_cores,
            physical_cores,
            freq_mhz,
            per_core_load,
            temperature_c,
            platform_tier: "linux-proc".to_owned(),
        },
        snapshot,
    ))
}

// ---- macOS Tier-1 -----------------------------------------------------------

/// Reads a `u32` sysctl value by name string.
#[cfg(target_os = "macos")]
fn macos_sysctl_u32(name: &std::ffi::CStr) -> Option<u32> {
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>();
    // SAFETY: `name` is a valid NUL-terminated C string; `value` is correctly
    // sized; `size` updated by kernel; read-only query (null newp/newlen).
    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&raw mut value).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 { Some(value) } else { None }
}

/// Reads a `u64` sysctl value by name string.
#[cfg(target_os = "macos")]
fn macos_sysctl_u64(name: &std::ffi::CStr) -> Option<u64> {
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: same invariants as `macos_sysctl_u32`.
    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&raw mut value).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 { Some(value) } else { None }
}

// CPU state array layout per `<mach/processor_info.h>`:
// Each logical CPU contributes CPU_STATE_MAX consecutive `integer_t` values.

/// Number of CPU-state counters per logical CPU in the `host_processor_info` buffer.
#[cfg(target_os = "macos")]
const CPU_STATE_MAX: usize = 4;
/// Index of the user-time counter within a per-CPU block.
#[cfg(target_os = "macos")]
const CPU_STATE_USER: usize = 0;
/// Index of the system-time counter within a per-CPU block.
#[cfg(target_os = "macos")]
const CPU_STATE_SYSTEM: usize = 1;
/// Index of the idle counter within a per-CPU block.
#[cfg(target_os = "macos")]
const CPU_STATE_IDLE: usize = 2;
/// Index of the nice-time counter within a per-CPU block.
#[cfg(target_os = "macos")]
const CPU_STATE_NICE: usize = 3;
/// `PROCESSOR_CPU_LOAD_INFO = 2` from `<mach/processor_info.h>`.
#[cfg(target_os = "macos")]
const PROCESSOR_CPU_LOAD_INFO: libc::processor_flavor_t = 2;

/// Reads per-core CPU load info via `host_processor_info(PROCESSOR_CPU_LOAD_INFO)`.
///
/// Returns `(user_nice_sys, total)` as parallel vectors indexed by logical core.
/// The sum `user + nice + sys` approximates "busy" ticks; `total` includes idle.
///
/// Returns `(vec![], vec![])` when the mach call fails (e.g., under restrictive
/// sandbox — fall through to zero-load vector construction by caller).
#[cfg(target_os = "macos")]
#[expect(
    deprecated,
    reason = "libc mach_host_self deprecated; staying libc-only per ADR-0042/ADR-0044"
)]
fn macos_processor_load_info() -> (Vec<u64>, Vec<u64>) {
    let mut info_array: libc::processor_info_array_t = std::ptr::null_mut();
    let mut info_count: libc::mach_msg_type_number_t = 0;
    let mut ncpus: libc::natural_t = 0;

    // SAFETY: `host_processor_info` writes into `info_array` (a newly allocated
    // Mach memory region), `info_count`, and `ncpus`. The caller must free the
    // returned buffer via `vm_deallocate`. Null flavor argument is valid for
    // PROCESSOR_CPU_LOAD_INFO. No pointer escapes this function.
    let kr = unsafe {
        libc::host_processor_info(
            libc::mach_host_self(),
            PROCESSOR_CPU_LOAD_INFO,
            &raw mut ncpus,
            &raw mut info_array,
            &raw mut info_count,
        )
    };

    if kr != libc::KERN_SUCCESS || info_array.is_null() || ncpus == 0 {
        return (Vec::new(), Vec::new());
    }

    let ncpu = ncpus as usize;

    let mut user_nice_sys = Vec::with_capacity(ncpu);
    let mut total = Vec::with_capacity(ncpu);

    for i in 0..ncpu {
        let base = i * CPU_STATE_MAX;
        // SAFETY: `info_array` points to `info_count` `integer_t` values allocated
        // by the kernel. Each CPU occupies `CPU_STATE_MAX` consecutive integers.
        // We verify the array is non-null above and access only within
        // `ncpu * CPU_STATE_MAX` elements which is guaranteed by the kernel.
        // `integer_t` is `i32` on macOS; we cast via `i32::unsigned_abs()` to avoid
        // sign-loss — tick counters are always non-negative in practice.
        #[expect(
            clippy::cast_sign_loss,
            reason = "processor tick counters are always non-negative; \
                      casting i32 kernel integer to u64 is safe for accumulation"
        )]
        let user = unsafe { *info_array.add(base + CPU_STATE_USER) } as u64;
        #[expect(
            clippy::cast_sign_loss,
            reason = "same as user — non-negative tick counter"
        )]
        let sys = unsafe { *info_array.add(base + CPU_STATE_SYSTEM) } as u64;
        #[expect(
            clippy::cast_sign_loss,
            reason = "same as user — non-negative tick counter"
        )]
        let idle = unsafe { *info_array.add(base + CPU_STATE_IDLE) } as u64;
        #[expect(
            clippy::cast_sign_loss,
            reason = "same as user — non-negative tick counter"
        )]
        let nice = unsafe { *info_array.add(base + CPU_STATE_NICE) } as u64;

        let busy = user.saturating_add(sys).saturating_add(nice);
        let ttl = busy.saturating_add(idle);
        user_nice_sys.push(busy);
        total.push(ttl);
    }

    // Free the Mach-allocated buffer.
    // SAFETY: `info_array` was allocated by the kernel via `host_processor_info`;
    // `info_count` is the exact element count. `vm_deallocate` is the correct
    // deallocation call for Mach memory regions.
    unsafe {
        libc::vm_deallocate(
            libc::mach_task_self(),
            info_array as libc::vm_address_t,
            (info_count as usize * std::mem::size_of::<libc::integer_t>()) as libc::vm_size_t,
        );
    }

    (user_nice_sys, total)
}

/// Reads CPU stats on macOS via sysctl + mach (Tier-1).
///
/// Never returns an error — all failure paths fall back to zero/None values.
#[cfg(target_os = "macos")]
pub(crate) fn read_cpu_macos(prev: Option<&CpuSnapshot>) -> (CpuStats, CpuSnapshot) {
    let logical_cores = macos_sysctl_u32(c"hw.logicalcpu").unwrap_or(1);
    let physical_cores = macos_sysctl_u32(c"hw.physicalcpu");
    // hw.cpufrequency is absent on Apple Silicon; returns None gracefully.
    let freq_mhz = macos_sysctl_u64(c"hw.cpufrequency").map(|hz| hz / 1_000_000);
    // Temperature not available via public macOS APIs.
    let temperature_c: Option<f32> = None;

    let (user_nice_sys, total) = macos_processor_load_info();
    let now = Instant::now();

    let n = if user_nice_sys.is_empty() {
        logical_cores as usize
    } else {
        user_nice_sys.len()
    };

    let per_core_load = if user_nice_sys.is_empty() {
        // host_processor_info failed; return zeros
        vec![0.0_f32; n]
    } else {
        prev.map_or_else(
            || vec![0.0_f32; n],
            |p| {
                user_nice_sys
                    .iter()
                    .zip(total.iter())
                    .zip(p.user_nice_sys.iter().zip(p.total.iter()))
                    .map(|((cur_busy, cur_total), (prev_busy, prev_total))| {
                        let delta_total = cur_total.saturating_sub(*prev_total);
                        let delta_busy = cur_busy.saturating_sub(*prev_busy);
                        if delta_total == 0 {
                            0.0_f32
                        } else {
                            #[expect(
                                clippy::cast_precision_loss,
                                clippy::cast_possible_truncation,
                                reason = "tick counts fit in f64; f64→f32 truncation is \
                                          acceptable for per-core % display (1 decimal place)"
                            )]
                            let pct = (delta_busy as f64 / delta_total as f64 * 100.0) as f32;
                            pct.clamp(0.0, 100.0)
                        }
                    })
                    .collect()
            },
        )
    };

    let snapshot = CpuSnapshot {
        user_nice_sys: if user_nice_sys.is_empty() {
            vec![0u64; n]
        } else {
            user_nice_sys
        },
        total: if total.is_empty() {
            vec![0u64; n]
        } else {
            total
        },
        at: now,
    };

    (
        CpuStats {
            logical_cores,
            physical_cores,
            freq_mhz,
            per_core_load,
            temperature_c,
            platform_tier: "macos-sysctl".to_owned(),
        },
        snapshot,
    )
}

// ---- Shared state for CPU delta snapshots -----------------------------------

/// Thread-safe shared CPU snapshot used across sequential `sys.cpu` calls.
///
/// `Arc<Mutex<Option<CpuSnapshot>>>`:
/// - `None` on first call → returns zeros + `cold_start` hint.
/// - `Some(prev)` on subsequent calls → computes delta load percentages.
pub type SharedCpuState = Arc<Mutex<Option<CpuSnapshot>>>;

/// Constructs a new `SharedCpuState` initialized to `None` (cold start).
#[must_use]
pub fn new_cpu_state() -> SharedCpuState {
    Arc::new(Mutex::new(None))
}

// ---- Platform dispatch ------------------------------------------------------

/// Reads CPU stats using the best available tier, returning `(CpuStats, CpuSnapshot)`.
///
/// `prev` is the previously cached snapshot for delta computation.
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` when the active platform read fails.
#[cfg(target_os = "linux")]
fn read_cpu_stats_inner(prev: Option<&CpuSnapshot>) -> SubstrateResult<(CpuStats, CpuSnapshot)> {
    read_cpu_linux(prev)
}

#[cfg(target_os = "macos")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "signature must match the Linux/fallback variants that CAN return Err"
)]
fn read_cpu_stats_inner(prev: Option<&CpuSnapshot>) -> SubstrateResult<(CpuStats, CpuSnapshot)> {
    Ok(read_cpu_macos(prev))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_cpu_stats_inner(_prev: Option<&CpuSnapshot>) -> SubstrateResult<(CpuStats, CpuSnapshot)> {
    use sysinfo::{CpuRefreshKind, RefreshKind, System};
    let mut sys =
        System::new_with_specifics(RefreshKind::nothing().with_cpu(CpuRefreshKind::everything()));
    sys.refresh_cpu_all();

    let cpus = sys.cpus();
    let logical_cores = cpus.len() as u32;
    let per_core_load: Vec<f32> = cpus.iter().map(|c| c.cpu_usage()).collect();
    let freq_mhz = cpus.first().map(|c| c.frequency());

    let snap = CpuSnapshot {
        user_nice_sys: vec![0u64; logical_cores as usize],
        total: vec![0u64; logical_cores as usize],
        at: Instant::now(),
    };

    Ok((
        CpuStats {
            logical_cores,
            physical_cores: None,
            freq_mhz,
            per_core_load,
            temperature_c: None,
            platform_tier: "sysinfo".to_owned(),
        },
        snap,
    ))
}

// ---- Handler ----------------------------------------------------------------

/// Handles a `sys.cpu` tool call.
///
/// Returns a [`CpuStats`] snapshot of CPU topology and utilization.
/// On the first call after startup, `per_core_load` contains all zeros and
/// `cold_start: true` is set in the response hints per ADR-0050.
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` when the platform read fails.
#[instrument(skip(deps, cpu_state))]
pub async fn handle_sys_cpu(
    deps: Arc<SystemInfoDeps>,
    cpu_state: SharedCpuState,
) -> SubstrateResult<ToolResponse> {
    let _ = deps;

    let result = spawn_blocking(move || {
        // Take the previous snapshot under lock, then release before I/O.
        let prev_snapshot: Option<CpuSnapshot> = {
            cpu_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };

        let (stats, new_snap) = read_cpu_stats_inner(prev_snapshot.as_ref())?;

        // Update the shared state with the new snapshot.
        let mut guard = cpu_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(new_snap);
        drop(guard); // release lock promptly; avoids holding it beyond this point

        Ok::<CpuStats, substrate_domain::SubstrateError>(stats)
    })
    .await
    .map_err(|e| substrate_domain::SubstrateError::InternalError {
        reason: format!("spawn_blocking join error in sys.cpu: {e}"),
        correlation_id: None,
    })??;

    let cold_start = result.per_core_load.iter().all(|&v| v == 0.0);
    if cold_start {
        debug!("sys.cpu cold_start: first call returns zero per-core load");
    }

    let avg_load = if result.per_core_load.is_empty() {
        0.0_f32
    } else {
        #[expect(
            clippy::cast_precision_loss,
            reason = "core count fits in f32 mantissa (≤ 1024 cores)"
        )]
        {
            result.per_core_load.iter().copied().sum::<f32>() / result.per_core_load.len() as f32
        }
    };

    let content = format!(
        "sys.cpu: {} logical cores, avg load {:.1}%, freq {:?} MHz tier={}.",
        result.logical_cores, avg_load, result.freq_mhz, result.platform_tier,
    );

    let mut hints = build_info_hints(Some("sys.mem"), Some("proc.top"));
    if cold_start {
        hints = Hints {
            next_action_suggested: Some(
                "Re-invoke sys.cpu after 100ms to obtain non-zero per-core load values.".to_owned(),
            ),
            ..hints
        };
    }

    Ok(ToolResponse::with_hints(content, json!(result), hints))
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::similar_names,
    reason = "test module — panics on assertion failure are the intended behavior; state/stats name collision is unavoidable in CpuStats tests"
)]
mod tests {
    use std::sync::Arc;

    use substrate_domain::Capabilities;

    use super::*;
    use crate::response::SystemInfoDeps;

    fn make_deps() -> Arc<SystemInfoDeps> {
        Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        })
    }

    #[tokio::test]
    async fn sys_cpu_logical_cores_positive() {
        let deps = make_deps();
        let state = new_cpu_state();
        let resp = handle_sys_cpu(deps, state)
            .await
            .expect("sys.cpu must not fail");
        let stats: CpuStats =
            serde_json::from_value(resp.structured_content).expect("valid CpuStats JSON");
        assert!(stats.logical_cores > 0, "logical_cores must be > 0");
    }

    #[tokio::test]
    async fn sys_cpu_per_core_load_length_matches_logical_cores() {
        let deps = make_deps();
        let state = new_cpu_state();
        let resp = handle_sys_cpu(deps, state)
            .await
            .expect("sys.cpu must not fail");
        let stats: CpuStats =
            serde_json::from_value(resp.structured_content).expect("valid CpuStats JSON");
        assert_eq!(
            stats.per_core_load.len(),
            stats.logical_cores as usize,
            "per_core_load length must equal logical_cores"
        );
    }

    #[tokio::test]
    async fn sys_cpu_first_call_is_cold_start() {
        let deps = make_deps();
        let state = new_cpu_state();
        let resp = handle_sys_cpu(deps, state)
            .await
            .expect("sys.cpu must not fail");
        let stats: CpuStats =
            serde_json::from_value(resp.structured_content).expect("valid CpuStats JSON");
        // First call must return all-zero per_core_load (cold start semantics).
        for (i, &v) in stats.per_core_load.iter().enumerate() {
            assert!(
                v.abs() < f32::EPSILON,
                "per_core_load[{i}] must be 0.0 on first call (got {v})"
            );
        }
    }

    #[tokio::test]
    async fn sys_cpu_content_format() {
        let deps = make_deps();
        let state = new_cpu_state();
        let resp = handle_sys_cpu(deps, state)
            .await
            .expect("sys.cpu must not fail");
        assert!(
            resp.content.starts_with("sys.cpu:"),
            "content must start with 'sys.cpu:'"
        );
    }

    #[tokio::test]
    async fn sys_cpu_platform_tier_nonempty() {
        let deps = make_deps();
        let state = new_cpu_state();
        let resp = handle_sys_cpu(deps, state)
            .await
            .expect("sys.cpu must not fail");
        let stats: CpuStats = serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert!(
            !stats.platform_tier.is_empty(),
            "platform_tier must not be empty"
        );
    }

    #[tokio::test]
    async fn sys_cpu_second_call_uses_same_state() {
        let deps = make_deps();
        let state = new_cpu_state();
        // First call primes the state.
        let _ = handle_sys_cpu(Arc::clone(&deps), Arc::clone(&state))
            .await
            .expect("first sys.cpu must not fail");
        // Second call uses the cached snapshot.
        let resp = handle_sys_cpu(deps, state)
            .await
            .expect("second sys.cpu must not fail");
        let stats: CpuStats = serde_json::from_value(resp.structured_content).expect("valid JSON");
        // Entries may still be 0.0 if not enough CPU time elapsed between calls;
        // just verify they are all finite and non-negative.
        for (i, &v) in stats.per_core_load.iter().enumerate() {
            assert!(
                v.is_finite() && v >= 0.0,
                "per_core_load[{i}] must be finite >= 0"
            );
        }
    }
}
