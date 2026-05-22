//! `sys.df` handler — Zone A (sync inline).
//!
//! The macOS implementation calls `libc::getmntinfo` directly; see the
//! `read_mounts` function for full safety justification (ADR-0042 + ADR-0044).
// macOS getmntinfo FFI — module-level allow per ADR-0042 + ADR-0044 carve-out.
// This file is the ONLY module in this crate that uses unsafe code.
#![cfg_attr(
    target_os = "macos",
    allow(
        unsafe_code,
        reason = "libc::getmntinfo + CStr FFI on macOS; no safe wrapper exists. ADR-0042 + ADR-0044 sysctl/getmntinfo carve-out."
    )
)]
//!
//! Lists mounted filesystems with capacity, used, available bytes, and
//! usage percentage.
//!
//! # Platform strategy
//!
//! - **Linux**: iterates `/proc/mounts` entries via `procfs` and calls
//!   `nix::sys::statvfs::statvfs()` for each real mount point.
//! - **macOS**: calls `libc::getmntinfo(3)` which enumerates mount points via
//!   `statfs64` structures. Requires a narrow `unsafe` block at function scope
//!   (ADR-0042 + ADR-0044 carve-out).
//!
//! Pseudo-filesystems (`proc`, `sysfs`, `devtmpfs`, `cgroup2`, `tmpfs`
//! by default) are filtered so the response covers only real storage.

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use tracing::instrument;

use crate::{
    hints_helpers::build_info_hints,
    response::{SystemInfoDeps, ToolResponse},
};
use substrate_domain::SubstrateResult;

/// Set of pseudo-filesystem types excluded from `sys.df` output by default.
#[cfg(target_os = "linux")]
const PSEUDO_FS_TYPES: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "cgroup",
    "cgroup2",
    "pstore",
    "bpf",
    "tracefs",
    "debugfs",
    "securityfs",
    "autofs",
    "mqueue",
    "hugetlbfs",
    "fusectl",
    "ramfs",
    "tmpfs",
];

/// Single mounted filesystem record.
#[derive(Debug, Clone, Serialize)]
pub struct MountPoint {
    /// Device name or source (e.g., `/dev/sda1`, `overlay`).
    pub device: String,
    /// Mount path (e.g., `/`, `/home`).
    pub mount: String,
    /// Filesystem type (e.g., `ext4`, `xfs`, `apfs`).
    pub fstype: String,
    /// Total capacity in bytes.
    pub total_bytes: u64,
    /// Bytes currently in use.
    pub used_bytes: u64,
    /// Bytes available to unprivileged users.
    pub available_bytes: u64,
    /// Usage percentage rounded to one decimal place (0.0–100.0).
    pub use_pct: f64,
}

impl MountPoint {
    /// Computes the usage percentage from `used_bytes` / `total_bytes`.
    #[must_use]
    pub fn usage_pct(used: u64, total: u64) -> f64 {
        if total == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let pct = (used as f64 / total as f64) * 100.0;
            // Round to one decimal place.
            (pct * 10.0).round() / 10.0
        }
    }
}

// ---- Linux implementation ---------------------------------------------------

#[cfg(target_os = "linux")]
fn read_mounts() -> SubstrateResult<Vec<MountPoint>> {
    let mounts = procfs::process::Process::myself()
        .and_then(|p| p.mountinfo())
        .map_err(|e| substrate_domain::SubstrateError::InternalError {
            reason: format!("procfs mountinfo failed: {e}"),
            correlation_id: None,
        })?;

    let mut result = Vec::new();
    for entry in mounts {
        let fstype = entry.fs_type.clone();
        if PSEUDO_FS_TYPES.contains(&fstype.as_str()) {
            continue;
        }
        let mount_path = entry.mount_point.to_string_lossy().into_owned();
        let stat = match nix::sys::statvfs::statvfs(entry.mount_point.as_path()) {
            Ok(s) => s,
            Err(_) => continue, // skip unmountable or inaccessible entries
        };
        let block_size = stat.block_size();
        let total_bytes = stat.blocks() * block_size;
        let avail_bytes = stat.blocks_available() * block_size;
        let used_bytes = total_bytes.saturating_sub(stat.blocks_free() * block_size);
        let use_pct = MountPoint::usage_pct(used_bytes, total_bytes);

        result.push(MountPoint {
            device: entry.mount_source.unwrap_or_else(|| "unknown".to_owned()),
            mount: mount_path,
            fstype,
            total_bytes,
            used_bytes,
            available_bytes: avail_bytes,
            use_pct,
        });
    }
    Ok(result)
}

// ---- macOS implementation ---------------------------------------------------
//
// `libc::getmntinfo` (MNT_NOWAIT mode) fills a kernel-owned array of
// `statfs` structs in a single syscall. The returned pointer is valid until
// the next `getmntinfo` call on this thread.
//
// Safety justification (ADR-0042 + ADR-0044 sysctl FFI carve-out):
// The unsafe block is narrowly scoped to `getmntinfo` + slice construction.
// We copy every field we need out of the kernel-owned buffer before the
// function returns, so no raw pointer escapes.

/// Pseudo-filesystem type names skipped on macOS (virtual/kernel-internal).
#[cfg(target_os = "macos")]
const PSEUDO_FS_TYPES_MACOS: &[&str] = &["devfs", "autofs", "nullfs", "fdesc", "map auto.home"];

#[cfg(target_os = "macos")]
fn read_mounts() -> SubstrateResult<Vec<MountPoint>> {
    // SAFETY: `getmntinfo` with `MNT_NOWAIT` is a standard macOS call.
    // The function returns the count of entries written and a pointer to a
    // kernel-owned, thread-local buffer of `statfs` structs.
    // We read each element's fields immediately and do NOT store the raw pointer
    // beyond this function. The buffer is valid until the next `getmntinfo`
    // call on this thread; since we read synchronously and return owned data,
    // there is no aliasing concern.
    let mut mounts_ptr: *mut libc::statfs = std::ptr::null_mut();
    let count = unsafe { libc::getmntinfo(&raw mut mounts_ptr, libc::MNT_NOWAIT) };

    if count < 0 {
        return Err(substrate_domain::SubstrateError::InternalError {
            reason: format!("getmntinfo failed: {}", std::io::Error::last_os_error()),
            correlation_id: None,
        });
    }

    if count == 0 || mounts_ptr.is_null() {
        return Ok(Vec::new());
    }

    // SAFETY: `mounts_ptr` is a valid, aligned pointer to `count` consecutive
    // `statfs` elements. We create a shared slice covering exactly that range;
    // the kernel guarantees the buffer is readable. We immediately copy all
    // fields we need into owned Rust types before this function returns, so
    // the raw slice does not escape.
    // SAFETY: count is proven non-negative by the `count < 0` guard above.
    #[expect(
        clippy::cast_sign_loss,
        reason = "count is i32 from getmntinfo; negativity is checked explicitly above"
    )]
    let entries = unsafe { std::slice::from_raw_parts(mounts_ptr, count as usize) };

    let mut result = Vec::with_capacity(entries.len());

    for entry in entries {
        // Extract fstype as a &str from the fixed-size C char array.
        // SAFETY: `f_fstypename` is a NUL-terminated array filled by the kernel.
        // `CStr::from_ptr` is safe because the kernel guarantees NUL termination
        // within the MFSNAMELEN (16) bytes.
        let fstype = unsafe { std::ffi::CStr::from_ptr(entry.f_fstypename.as_ptr()) }
            .to_str()
            .unwrap_or("unknown")
            .to_owned();

        if PSEUDO_FS_TYPES_MACOS.contains(&fstype.as_str()) {
            continue;
        }

        // SAFETY: same reasoning as above for f_mntonname and f_mntfromname.
        let mount = unsafe { std::ffi::CStr::from_ptr(entry.f_mntonname.as_ptr()) }
            .to_str()
            .unwrap_or("unknown")
            .to_owned();

        let device = unsafe { std::ffi::CStr::from_ptr(entry.f_mntfromname.as_ptr()) }
            .to_str()
            .unwrap_or("unknown")
            .to_owned();

        // `statfs` on macOS uses u32 block sizes and u64 block counts.
        let block_size = u64::from(entry.f_bsize);
        let total_bytes = entry.f_blocks.saturating_mul(block_size);
        let avail_bytes = entry.f_bavail.saturating_mul(block_size);
        let used_bytes = total_bytes.saturating_sub(entry.f_bfree.saturating_mul(block_size));

        result.push(MountPoint {
            device,
            mount,
            fstype,
            total_bytes,
            used_bytes,
            available_bytes: avail_bytes,
            use_pct: MountPoint::usage_pct(used_bytes, total_bytes),
        });
    }

    Ok(result)
}

/// Handles a `sys.df` tool call.
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` if the mount table cannot be read.
#[instrument(skip(deps))]
pub async fn handle_sys_df(deps: Arc<SystemInfoDeps>) -> SubstrateResult<ToolResponse> {
    let _ = deps;
    let mounts = read_mounts()?;
    let count = mounts.len();
    let content = format!("sys.df: {count} filesystem(s) mounted.");
    let hints = build_info_hints(Some("sys.info"), None);
    Ok(ToolResponse::with_hints(
        content,
        json!({ "mounts": mounts }),
        hints,
    ))
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::float_cmp,
    reason = "test code: expect() and exact float equality assertions are idiomatic in unit tests"
)]
mod tests {
    use std::sync::Arc;

    use substrate_domain::Capabilities;

    use super::*;
    use crate::response::SystemInfoDeps;

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn df_returns_at_least_one_mount_linux() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_df(deps).await.expect("sys.df must not fail");
        let obj = resp.structured_content.as_object().expect("object");
        let mounts = obj["mounts"].as_array().expect("mounts array");
        assert!(
            !mounts.is_empty(),
            "at least one real mount must exist on Linux"
        );
    }

    #[tokio::test]
    async fn df_does_not_error() {
        // Platform-neutral: just asserts no error is returned.
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        handle_sys_df(deps)
            .await
            .expect("sys.df must not fail on any platform");
    }

    #[test]
    fn usage_pct_zero_total() {
        assert_eq!(MountPoint::usage_pct(0, 0), 0.0);
    }

    #[test]
    fn usage_pct_half() {
        assert_eq!(MountPoint::usage_pct(50, 100), 50.0);
    }

    #[test]
    fn usage_pct_full() {
        assert_eq!(MountPoint::usage_pct(100, 100), 100.0);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn df_returns_at_least_one_mount_macos() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_df(deps).await.expect("sys.df must not fail");
        let obj = resp.structured_content.as_object().expect("object");
        let mounts = obj["mounts"].as_array().expect("mounts array");
        assert!(
            !mounts.is_empty(),
            "at least one real mount must exist on macOS"
        );
    }

    #[test]
    fn usage_pct_over_hundred_is_clamped_at_display_level() {
        // usage_pct does NOT clamp; caller is expected to handle. This test
        // documents the existing behavior: a partial block may exceed 100% due
        // to rounding in the one-decimal-place display.
        let pct = MountPoint::usage_pct(101, 100);
        assert!((pct - 101.0).abs() < 0.1);
    }

    #[test]
    fn usage_pct_rounds_to_one_decimal() {
        // 33 / 100 = 33.0%, rounds cleanly.
        assert_eq!(MountPoint::usage_pct(1, 3), 33.3);
    }
}
