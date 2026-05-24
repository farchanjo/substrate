//! `sys.mem` handler — Zone B (`spawn_blocking`; file I/O on Linux, sysctl on macOS).
//!
//! Returns a detailed memory snapshot including total, used, available, free,
//! and swap statistics. Implements the tier-cascade pattern from ADR-0042:
//! Tier-1 uses native OS interfaces; Tier-2 falls back to the `sysinfo` crate.
//!
//! # Platform strategy
//!
//! - **Linux (Tier 1)**: parses `/proc/meminfo` fields `MemTotal`, `MemFree`,
//!   `MemAvailable`, `Buffers`, `Cached`, `SwapTotal`, `SwapFree`. The
//!   `used_bytes` field is derived as `MemTotal - MemAvailable`. Reads are
//!   performed via `tokio::fs::read_to_string` inside `spawn_blocking`.
//! - **macOS (Tier 1)**: reuses the `sysctl(HW_MEMSIZE)` + `host_statistics64`
//!   + `sysctl(VM_SWAPUSAGE)` calls from `info.rs`. The `available_bytes`
//!     estimate combines free and inactive VM pages, mirroring macOS Activity
//!     Monitor semantics.
//! - **Tier 2 (cross-platform fallback)**: `sysinfo::System::new_with_specifics`
//!   is called when neither native path is available.
//!
//! # See also
//!
//! [ADR-0050](../../../docs/arch/adr/0050-system-resource-monitoring.md)
// macOS sysctl + mach FFI — module-level allow per ADR-0042 + ADR-0044 carve-out.
#![cfg_attr(
    target_os = "macos",
    allow(
        unsafe_code,
        reason = "libc sysctl(HW_MEMSIZE, VM_SWAPUSAGE) + sysconf + mach host_statistics64 FFI on macOS; \
                  read-only syscalls. ADR-0042 + ADR-0044 sysctl/mach carve-out. ADR-0050 sys.mem."
    )
)]

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::task::spawn_blocking;
use tracing::instrument;

use crate::{
    hints_helpers::build_info_hints,
    response::{SystemInfoDeps, ToolResponse},
};
use substrate_domain::SubstrateResult;

// ---- Value object -------------------------------------------------------

/// Detailed physical and virtual memory snapshot returned by `sys.mem`.
///
/// All byte values are raw OS-reported integers; no unit conversion is applied.
/// The `platform_tier` field identifies the data source used for this snapshot
/// per the ADR-0050 tier-cascade contract.
///
/// See [ADR-0050](../../../docs/arch/adr/0050-system-resource-monitoring.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySnapshot {
    /// Total installed physical RAM in bytes.
    pub total_bytes: u64,
    /// Memory currently in active use (total minus available), in bytes.
    pub used_bytes: u64,
    /// Memory immediately available to new allocations without swapping, in bytes.
    ///
    /// On Linux: `MemAvailable` from `/proc/meminfo`.
    /// On macOS: free + inactive page count multiplied by page size.
    pub available_bytes: u64,
    /// Memory not in use at all (excludes cached/buffered pages), in bytes.
    ///
    /// On Linux: `MemFree` from `/proc/meminfo`.
    /// On macOS: `free_count * page_size`.
    pub free_bytes: u64,
    /// Total swap partition/file size in bytes. `0` when no swap is configured.
    pub swap_total_bytes: u64,
    /// Swap currently in use in bytes.
    pub swap_used_bytes: u64,
    /// Data-source tier tag per ADR-0050.
    ///
    /// Values: `"linux-proc"`, `"macos-sysctl"`, `"sysinfo"`.
    pub platform_tier: String,
}

// ---- Linux Tier-1 -----------------------------------------------------------

/// Reads `/proc/meminfo` and returns a `MemorySnapshot` (Tier-1 Linux path).
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` when `/proc/meminfo` is unreadable
/// or a required field is missing. Recovery hint: check container policy.
#[cfg(target_os = "linux")]
pub(crate) fn read_memory_linux() -> SubstrateResult<MemorySnapshot> {
    let content = std::fs::read_to_string("/proc/meminfo").map_err(|e| {
        substrate_domain::SubstrateError::InternalError {
            reason: format!("read /proc/meminfo failed: {e}"),
            correlation_id: None,
        }
    })?;

    let mut total: u64 = 0;
    let mut free: u64 = 0;
    let mut available: u64 = 0;
    let mut swap_total: u64 = 0;
    let mut swap_free: u64 = 0;

    for line in content.lines() {
        // Lines are of the form: "FieldName:     <value> kB"
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let val_kb: u64 = rest
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        match key.trim() {
            "MemTotal" => total = val_kb * 1024,
            "MemFree" => free = val_kb * 1024,
            "MemAvailable" => available = val_kb * 1024,
            "SwapTotal" => swap_total = val_kb * 1024,
            "SwapFree" => swap_free = val_kb * 1024,
            _ => {},
        }
    }

    if total == 0 {
        return Err(substrate_domain::SubstrateError::InternalError {
            reason: "MemTotal missing or zero in /proc/meminfo".to_owned(),
            correlation_id: None,
        });
    }

    let used = total.saturating_sub(available);
    let swap_used = swap_total.saturating_sub(swap_free);

    Ok(MemorySnapshot {
        total_bytes: total,
        used_bytes: used,
        available_bytes: available,
        free_bytes: free,
        swap_total_bytes: swap_total,
        swap_used_bytes: swap_used,
        platform_tier: "linux-proc".to_owned(),
    })
}

// ---- macOS Tier-1 -----------------------------------------------------------

/// Total physical RAM via `sysctl(CTL_HW, HW_MEMSIZE)`.
#[cfg(target_os = "macos")]
fn macos_hw_memsize() -> SubstrateResult<u64> {
    let mut mib: [libc::c_int; 2] = [libc::CTL_HW, libc::HW_MEMSIZE];
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: 2-element MIB for HW_MEMSIZE; `value` is correctly-sized u64
    // buffer; `size` updated by kernel; read-only query (null newp).
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
                "sysctl(HW_MEMSIZE) failed in sys.mem: {}",
                std::io::Error::last_os_error()
            ),
            correlation_id: None,
        })
    }
}

/// VM page statistics via `host_statistics64(HOST_VM_INFO64)`.
#[cfg(target_os = "macos")]
#[expect(
    deprecated,
    reason = "libc deprecates mach_host_self; staying libc-only per ADR-0042/ADR-0044 to avoid extra dependency"
)]
fn macos_vm_stats() -> SubstrateResult<libc::vm_statistics64> {
    // SAFETY: all-zero `vm_statistics64` is a valid POD of integers.
    let mut vm: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count: libc::mach_msg_type_number_t = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: `mach_host_self()` returns the host port; `host_statistics64`
    // writes exactly `count` integers into `vm`. Read-only kernel query.
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
                "host_statistics64(HOST_VM_INFO64) failed in sys.mem: kern_return={kr}"
            ),
            correlation_id: None,
        })
    }
}

/// Swap usage `(total_bytes, used_bytes)` via `sysctl(CTL_VM, VM_SWAPUSAGE)`.
/// Returns `(0, 0)` on failure; swap is best-effort.
#[cfg(target_os = "macos")]
fn macos_swap() -> (u64, u64) {
    let mut mib: [libc::c_int; 2] = [libc::CTL_VM, libc::VM_SWAPUSAGE];
    // SAFETY: all-zero `xsw_usage` is a valid POD value.
    let mut xsw: libc::xsw_usage = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::xsw_usage>();
    // SAFETY: 2-element MIB for VM_SWAPUSAGE; `xsw` correctly sized; read-only.
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
        (xsw.xsu_total, xsw.xsu_used)
    } else {
        (0, 0)
    }
}

/// Reads macOS memory statistics via sysctl + mach (Tier-1 macOS path).
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` when `sysctl(HW_MEMSIZE)` or
/// `host_statistics64` fails.
#[cfg(target_os = "macos")]
pub(crate) fn read_memory_macos() -> SubstrateResult<MemorySnapshot> {
    let total_bytes = macos_hw_memsize()?;

    // SAFETY: `sysconf` with a static name returns the page size or -1 on error.
    let page_size_raw = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = u64::try_from(page_size_raw).unwrap_or(4096);

    let vm = macos_vm_stats()?;

    let free_bytes = u64::from(vm.free_count).saturating_mul(page_size);
    // available = free + inactive (reclaimable without I/O)
    let available_bytes = u64::from(vm.free_count)
        .saturating_add(u64::from(vm.inactive_count))
        .saturating_mul(page_size);
    let used_bytes = total_bytes.saturating_sub(available_bytes);

    let (swap_total_bytes, swap_used_bytes) = macos_swap();

    Ok(MemorySnapshot {
        total_bytes,
        used_bytes,
        available_bytes,
        free_bytes,
        swap_total_bytes,
        swap_used_bytes,
        platform_tier: "macos-sysctl".to_owned(),
    })
}

// ---- Tier-2 fallback (sysinfo crate) ----------------------------------------

/// Reads memory statistics via the `sysinfo` crate (cross-platform Tier-2).
///
/// Used when neither the Linux nor macOS native tier is available, or when the
/// native tier probe fails at runtime. Marked `#[allow(dead_code)]` because the
/// function is only reachable on non-Linux/macOS targets via `cfg_select!`.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn read_memory_sysinfo() -> SubstrateResult<MemorySnapshot> {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
    );
    sys.refresh_memory();

    let total_bytes = sys.total_memory();
    let free_bytes = sys.free_memory();
    let available_bytes = sys.available_memory();
    let used_bytes = sys.used_memory();
    let swap_total_bytes = sys.total_swap();
    let swap_used_bytes = sys.used_swap();

    Ok(MemorySnapshot {
        total_bytes,
        used_bytes,
        available_bytes,
        free_bytes,
        swap_total_bytes,
        swap_used_bytes,
        platform_tier: "sysinfo".to_owned(),
    })
}

// ---- Dispatch ---------------------------------------------------------------

/// Reads memory statistics using the best available tier on the current platform.
///
/// On Linux: Tier-1 `/proc/meminfo`; on macOS: Tier-1 sysctl + mach.
/// Falls back to `sysinfo` crate on other platforms.
///
/// # Errors
///
/// Propagates errors from the active platform tier.
pub fn read_memory_stats() -> SubstrateResult<MemorySnapshot> {
    #[cfg(target_os = "linux")]
    {
        read_memory_linux()
    }
    #[cfg(target_os = "macos")]
    {
        read_memory_macos()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        read_memory_sysinfo()
    }
}

// ---- Handler ----------------------------------------------------------------

/// Handles a `sys.mem` tool call.
///
/// Returns a [`MemorySnapshot`] containing total, used, available, free, and
/// swap memory statistics via the platform-appropriate tier per ADR-0050.
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` when the platform memory read fails.
/// Recovery hint: check container policy for read access to `/proc/meminfo` on
/// Linux or verify sysctl access on macOS.
#[instrument(skip(deps))]
pub async fn handle_sys_mem(deps: Arc<SystemInfoDeps>) -> SubstrateResult<ToolResponse> {
    let _ = deps;
    let snapshot = spawn_blocking(read_memory_stats).await.map_err(|e| {
        substrate_domain::SubstrateError::InternalError {
            reason: format!("spawn_blocking join error in sys.mem: {e}"),
            correlation_id: None,
        }
    })??;

    let content = format!(
        "sys.mem: total={} MB used={} MB available={} MB swap={}/{} MB tier={}.",
        snapshot.total_bytes / 1_048_576,
        snapshot.used_bytes / 1_048_576,
        snapshot.available_bytes / 1_048_576,
        snapshot.swap_used_bytes / 1_048_576,
        snapshot.swap_total_bytes / 1_048_576,
        snapshot.platform_tier,
    );

    let hints = build_info_hints(Some("sys.cpu"), Some("sys.info"));
    Ok(ToolResponse::with_hints(content, json!(snapshot), hints))
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    reason = "test module — panics and unwraps on assertion failure are the intended behavior"
)]
mod tests {
    use std::sync::Arc;

    use substrate_domain::Capabilities;

    use super::*;
    use crate::response::SystemInfoDeps;

    #[tokio::test]
    async fn sys_mem_returns_nonzero_total() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_mem(deps).await.expect("sys.mem must not fail");
        let snap: MemorySnapshot =
            serde_json::from_value(resp.structured_content).expect("valid MemorySnapshot JSON");
        assert!(snap.total_bytes > 0, "total_bytes must be > 0");
    }

    #[tokio::test]
    async fn sys_mem_available_le_total() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_mem(deps).await.expect("sys.mem must not fail");
        let snap: MemorySnapshot =
            serde_json::from_value(resp.structured_content).expect("valid MemorySnapshot JSON");
        assert!(
            snap.available_bytes <= snap.total_bytes,
            "available must not exceed total"
        );
    }

    #[tokio::test]
    async fn sys_mem_free_le_total() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_mem(deps).await.expect("sys.mem must not fail");
        let snap: MemorySnapshot =
            serde_json::from_value(resp.structured_content).expect("valid MemorySnapshot JSON");
        assert!(
            snap.free_bytes <= snap.total_bytes,
            "free must not exceed total"
        );
    }

    #[tokio::test]
    async fn sys_mem_content_format() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_mem(deps).await.expect("sys.mem must not fail");
        assert!(
            resp.content.starts_with("sys.mem:"),
            "content must start with 'sys.mem:'"
        );
    }

    #[tokio::test]
    async fn sys_mem_platform_tier_nonempty() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_mem(deps).await.expect("sys.mem must not fail");
        let snap: MemorySnapshot =
            serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert!(
            !snap.platform_tier.is_empty(),
            "platform_tier must not be empty"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_tier_tag_is_linux_proc() {
        let snap = read_memory_linux().expect("linux mem read must succeed");
        assert_eq!(snap.platform_tier, "linux-proc");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_tier_tag_is_macos_sysctl() {
        let snap = read_memory_macos().expect("macos mem read must succeed");
        assert_eq!(snap.platform_tier, "macos-sysctl");
    }
}
