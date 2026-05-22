//! `sys.info` handler — Zone A (sync inline).
// macOS sysctl + mach FFI — module-level allow per ADR-0042 + ADR-0044 carve-out.
#![cfg_attr(
    target_os = "macos",
    allow(
        unsafe_code,
        reason = "libc sysctl(HW_MEMSIZE, VM_SWAPUSAGE) + sysconf + mach host_statistics64(HOST_VM_INFO64) FFI on macOS; read-only syscalls. ADR-0042 + ADR-0044 sysctl/mach carve-out."
    )
)]
//!
//! Returns a composite `SystemSnapshot` combining kernel version, hostname,
//! uptime, memory statistics, and load averages in a single call.
//!
//! # Memory statistics platform strategy
//!
//! - **Linux**: `nix::sys::sysinfo::sysinfo()` provides `totalram`, `freeram`,
//!   and `totalswap` fields. Used RAM is derived as `totalram - freeram`.
//! - **macOS**: total RAM via `sysctl(HW_MEMSIZE)`; free/used via
//!   `host_statistics64(HOST_VM_INFO64)` where "available" counts immediately
//!   free plus reclaimable inactive pages; swap via `sysctl(VM_SWAPUSAGE)`. All
//!   are read-only syscalls behind a narrow `unsafe` carve-out
//!   (ADR-0042 + ADR-0044), mirroring the `uptime.rs` / `df.rs` pattern.

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use tracing::instrument;

use crate::{
    hints_helpers::build_info_hints,
    load_average::LoadAverage,
    response::{SystemInfoDeps, ToolResponse},
    uptime::Uptime,
};
use substrate_domain::SubstrateResult;

/// Memory statistics sub-record within `SystemSnapshot`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct MemoryStats {
    /// Total physical RAM in bytes.
    pub total_bytes: u64,
    /// RAM currently in use (total minus free), in bytes.
    pub used_bytes: u64,
    /// RAM available to new allocations, in bytes.
    pub free_bytes: u64,
    /// Total swap space in bytes.
    pub swap_total_bytes: u64,
    /// Swap currently in use, in bytes.
    pub swap_used_bytes: u64,
}

/// Composite OS and hardware snapshot returned by `sys.info`.
///
/// This is the aggregate root for the system-info bounded context (BC README).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct SystemSnapshot {
    /// OS kernel name, release, version, and machine architecture.
    pub kernel: KernelSummary,
    /// Short hostname.
    pub hostname: String,
    /// System uptime.
    pub uptime: Uptime,
    /// CPU load averages.
    pub load_average: LoadAverage,
    /// Physical memory statistics.
    ///
    /// Serialised as `"mem"` (not `"memory"`) to match the structured-content
    /// field name expected by the cucumber assertion steps (`system_info.rs`).
    #[serde(rename = "mem")]
    pub memory: MemoryStats,
}

/// Inline kernel summary (subset of `KernelVersion` for the composite view).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct KernelSummary {
    /// OS kernel name (e.g., `"Linux"` or `"Darwin"`).
    pub sysname: String,
    /// Kernel release string.
    pub release: String,
    /// Machine architecture.
    pub machine: String,
}

// ---- Linux memory read ------------------------------------------------------

#[cfg(target_os = "linux")]
fn read_memory() -> SubstrateResult<MemoryStats> {
    let info = nix::sys::sysinfo::sysinfo().map_err(|e| {
        substrate_domain::SubstrateError::InternalError {
            reason: format!("sysinfo(2) failed in sys.info: {e}"),
            correlation_id: None,
        }
    })?;
    let total_bytes = info.ram_total();
    let free_bytes = info.ram_unused();
    let used_bytes = total_bytes.saturating_sub(free_bytes);
    let swap_total_bytes = info.swap_total();
    let swap_free = info.swap_free();
    let swap_used_bytes = swap_total_bytes.saturating_sub(swap_free);
    Ok(MemoryStats {
        total_bytes,
        used_bytes,
        free_bytes,
        swap_total_bytes,
        swap_used_bytes,
    })
}

// ---- macOS memory read ------------------------------------------------------
//
// Safety justification (ADR-0042 + ADR-0044 sysctl/mach FFI carve-out): every
// syscall below is a standard read-only macOS query. `sysctl`/`sysconf` write
// only into the stack buffers we provide; `host_statistics64` fills a fixed-size
// `vm_statistics64` whose element count we declare via the libc-provided
// `HOST_VM_INFO64_COUNT`. No raw pointer escapes its call frame.

#[cfg(target_os = "macos")]
fn read_memory() -> SubstrateResult<MemoryStats> {
    let total_bytes = macos_total_ram()?;

    // SAFETY: `sysconf` with a static name returns the page size or -1 on error.
    let page_size_raw = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = u64::try_from(page_size_raw).unwrap_or(4096);

    let vm = macos_vm_stats()?;
    // "Available to new allocations" = immediately-free plus reclaimable inactive
    // pages, mirroring the intent of the Linux `ram_unused()` reading.
    let free_pages = u64::from(vm.free_count).saturating_add(u64::from(vm.inactive_count));
    let free_bytes = free_pages.saturating_mul(page_size);
    let used_bytes = total_bytes.saturating_sub(free_bytes);

    let (swap_total_bytes, swap_used_bytes) = macos_swap().unwrap_or((0, 0));

    Ok(MemoryStats {
        total_bytes,
        used_bytes,
        free_bytes,
        swap_total_bytes,
        swap_used_bytes,
    })
}

/// Total physical RAM in bytes via `sysctl(CTL_HW, HW_MEMSIZE)`.
#[cfg(target_os = "macos")]
fn macos_total_ram() -> SubstrateResult<u64> {
    let mut mib: [libc::c_int; 2] = [libc::CTL_HW, libc::HW_MEMSIZE];
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: 2-element MIB for HW_MEMSIZE; `value` is a correctly-sized u64
    // buffer; `size` is updated by the kernel; read-only query (null newp).
    let ret = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut value).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 {
        Ok(value)
    } else {
        Err(substrate_domain::SubstrateError::InternalError {
            reason: format!(
                "sysctl(HW_MEMSIZE) failed in sys.info: {}",
                std::io::Error::last_os_error()
            ),
            correlation_id: None,
        })
    }
}

/// Virtual-memory page statistics via `host_statistics64(HOST_VM_INFO64)`.
#[cfg(target_os = "macos")]
#[expect(
    deprecated,
    reason = "libc deprecates mach_host_self in favour of the mach2 crate; we stay libc-only per ADR-0042/0044 to avoid an extra dependency for one read-only host query"
)]
fn macos_vm_stats() -> SubstrateResult<libc::vm_statistics64> {
    // SAFETY: an all-zero `vm_statistics64` is a valid POD value of integers.
    let mut vm: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count: libc::mach_msg_type_number_t = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: `mach_host_self()` returns the host port; `host_statistics64`
    // writes exactly `count` integers into `vm` (sized via the libc constant).
    // Read-only kernel query; no pointer escapes.
    let kr = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            (&raw mut vm).cast::<libc::integer_t>(),
            &raw mut count,
        )
    };
    if kr == libc::KERN_SUCCESS {
        Ok(vm)
    } else {
        Err(substrate_domain::SubstrateError::InternalError {
            reason: format!(
                "host_statistics64(HOST_VM_INFO64) failed in sys.info: kern_return={kr}"
            ),
            correlation_id: None,
        })
    }
}

/// Swap usage `(total_bytes, used_bytes)` via `sysctl(CTL_VM, VM_SWAPUSAGE)`.
///
/// Returns `None` on failure; swap is best-effort and never fails `sys.info`.
#[cfg(target_os = "macos")]
fn macos_swap() -> Option<(u64, u64)> {
    let mut mib: [libc::c_int; 2] = [libc::CTL_VM, libc::VM_SWAPUSAGE];
    // SAFETY: an all-zero `xsw_usage` is a valid POD value.
    let mut xsw: libc::xsw_usage = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::xsw_usage>();
    // SAFETY: 2-element MIB for VM_SWAPUSAGE; `xsw` is correctly sized; read-only.
    let ret = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut xsw).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 {
        Some((xsw.xsu_total, xsw.xsu_used))
    } else {
        None
    }
}

/// Handles a `sys.info` tool call.
///
/// # Errors
///
/// Propagates errors from any sub-reader (`uname`, `gethostname`, uptime,
/// load-average, or memory).
#[instrument(skip(deps))]
pub async fn handle_sys_info(deps: Arc<SystemInfoDeps>) -> SubstrateResult<ToolResponse> {
    let _ = deps;

    // Kernel version (uname).
    let uts = nix::sys::utsname::uname().map_err(|e| {
        substrate_domain::SubstrateError::InternalError {
            reason: format!("uname(2) failed in sys.info: {e}"),
            correlation_id: None,
        }
    })?;
    let kernel = KernelSummary {
        sysname: uts.sysname().to_string_lossy().into_owned(),
        release: uts.release().to_string_lossy().into_owned(),
        machine: uts.machine().to_string_lossy().into_owned(),
    };

    // Hostname.
    let raw_host = nix::unistd::gethostname().map_err(|e| {
        substrate_domain::SubstrateError::InternalError {
            reason: format!("gethostname(2) failed in sys.info: {e}"),
            correlation_id: None,
        }
    })?;
    let hostname =
        raw_host
            .into_string()
            .map_err(|_| substrate_domain::SubstrateError::EncodingError {
                detail: "hostname non-UTF-8 in sys.info".to_owned(),
                correlation_id: None,
            })?;

    // Uptime (re-uses the platform function from uptime.rs via shared helper).
    let uptime_secs = crate::uptime::read_uptime_secs_pub()?;
    let uptime = Uptime {
        seconds: uptime_secs,
        human: Uptime::humanize(uptime_secs),
    };

    // Load averages.
    let load_average = crate::load_average::read_load_average_pub()?;

    // Memory.
    let memory = read_memory()?;

    let snapshot = SystemSnapshot {
        kernel,
        hostname,
        uptime,
        load_average,
        memory,
    };

    let content = format!(
        "sys.info: {} {} on {}, up {}, load {:.2}/{:.2}/{:.2}.",
        snapshot.kernel.sysname,
        snapshot.kernel.release,
        snapshot.hostname,
        snapshot.uptime.human,
        snapshot.load_average.load_1,
        snapshot.load_average.load_5,
        snapshot.load_average.load_15,
    );

    let hints = build_info_hints(Some("sys.df"), None);

    Ok(ToolResponse::with_hints(content, json!(snapshot), hints))
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test code: panicking assertions are idiomatic in unit tests"
)]
mod tests {
    use std::sync::Arc;

    use substrate_domain::Capabilities;

    use super::*;
    use crate::response::SystemInfoDeps;

    #[tokio::test]
    async fn sys_info_snapshot_kernel_and_host() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_info(deps).await.expect("sys.info must not fail");
        let snap: SystemSnapshot =
            serde_json::from_value(resp.structured_content).expect("valid SystemSnapshot JSON");
        assert!(!snap.kernel.sysname.is_empty(), "sysname must not be empty");
        assert!(!snap.hostname.is_empty(), "hostname must not be empty");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn sys_info_uptime_positive_linux() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_info(deps).await.expect("sys.info must not fail");
        let snap: SystemSnapshot =
            serde_json::from_value(resp.structured_content).expect("valid SystemSnapshot JSON");
        assert!(snap.uptime.seconds > 0, "uptime must be > 0 on Linux");
    }

    #[tokio::test]
    async fn sys_info_content_starts_correctly() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_info(deps).await.expect("sys.info must not fail");
        assert!(resp.content.starts_with("sys.info:"));
    }

    // Regression: macOS memory stats must be populated, not the previous
    // all-zeroes Wave-F stub. Asserts total RAM is real and the
    // used = total - free invariant holds.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn sys_info_memory_populated_macos() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_info(deps).await.expect("sys.info must not fail");
        let snap: SystemSnapshot =
            serde_json::from_value(resp.structured_content).expect("valid SystemSnapshot JSON");
        assert!(
            snap.memory.total_bytes > 0,
            "macOS total RAM must be reported, got 0 (stub regression)"
        );
        assert!(
            snap.memory.free_bytes <= snap.memory.total_bytes,
            "free must not exceed total"
        );
        assert_eq!(
            snap.memory.used_bytes,
            snap.memory.total_bytes - snap.memory.free_bytes,
            "used must equal total - free"
        );
    }
}
