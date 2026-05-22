//! `sys.uptime` handler — Zone A (sync inline).
//!
//! The macOS implementation calls `libc::sysctl(KERN_BOOTTIME)` directly; the
//! module-level allow below is the narrowest scope available for a file module.
// macOS sysctl FFI — module-level allow per ADR-0042 + ADR-0044 carve-out.
#![cfg_attr(target_os = "macos", allow(unsafe_code, reason = "libc::sysctl(KERN_BOOTTIME) FFI on macOS; standard read-only syscall. ADR-0042 + ADR-0044 sysctl carve-out."))]
//!
//! Returns system uptime in seconds and as a human-readable duration string.
//!
//! # Platform strategy
//!
//! - **Linux**: reads `/proc/uptime` via `procfs::Uptime::current()` — pure
//!   safe Rust, no syscall wrappers needed.
//! - **macOS**: calls `sysctl(KERN_BOOTTIME)` to retrieve a `struct timeval`
//!   and subtracts from `clock_gettime(CLOCK_REALTIME)`. Requires a narrow
//!   `unsafe` block scoped to this module (ADR-0042 + ADR-0044 carve-out).

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use tracing::instrument;

use crate::{
    hints_helpers::build_info_hints,
    response::{SystemInfoDeps, ToolResponse},
};
use substrate_domain::SubstrateResult;

/// Uptime record returned by `sys.uptime`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Uptime {
    /// Uptime in whole seconds since last boot.
    pub seconds: u64,
    /// Human-readable duration string (e.g., `"3d 4h 21m 10s"`).
    pub human: String,
}

impl Uptime {
    /// Formats an uptime in seconds as a compact human-readable string.
    #[must_use]
    pub fn humanize(seconds: u64) -> String {
        let days = seconds / 86_400;
        let hours = (seconds % 86_400) / 3_600;
        let minutes = (seconds % 3_600) / 60;
        let secs = seconds % 60;
        if days > 0 {
            format!("{days}d {hours}h {minutes}m {secs}s")
        } else if hours > 0 {
            format!("{hours}h {minutes}m {secs}s")
        } else if minutes > 0 {
            format!("{minutes}m {secs}s")
        } else {
            format!("{secs}s")
        }
    }
}

// ---- Linux implementation ---------------------------------------------------

/// Platform-internal uptime reader, also re-used by `info.rs`.
#[cfg(target_os = "linux")]
pub(crate) fn read_uptime_secs_pub() -> SubstrateResult<u64> {
    read_uptime_secs()
}

#[cfg(target_os = "linux")]
fn read_uptime_secs() -> SubstrateResult<u64> {
    use procfs::Uptime as ProcUptime;
    let up =
        ProcUptime::current().map_err(|e| substrate_domain::SubstrateError::InternalError {
            reason: format!("procfs::Uptime::current() failed: {e}"),
            correlation_id: None,
        })?;
    // `up.uptime` is an f64 of seconds (including fractional); truncate to u64.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Ok(up.uptime as u64)
}

// ---- macOS implementation ---------------------------------------------------
//
// Uses sysctl(CTL_KERN, KERN_BOOTTIME) to retrieve the boot time as a
// struct timeval, then subtracts from SystemTime::now() to get uptime.
//
// Safety justification (ADR-0042 + ADR-0044 sysctl FFI carve-out):
// `libc::sysctl` with a KERN_BOOTTIME MIB is a standard macOS read-only
// syscall; no pointer is stored beyond the call frame. The narrow
// `#[allow(unsafe_code)]` below is the ONLY unsafe carve-out in this file.

/// Platform-internal uptime reader, also re-used by `info.rs`.
#[cfg(target_os = "macos")]
pub(crate) fn read_uptime_secs_pub() -> SubstrateResult<u64> {
    read_uptime_secs()
}

#[cfg(target_os = "macos")]
fn read_uptime_secs() -> SubstrateResult<u64> {
    let mut mib: [libc::c_int; 2] = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    let mut size = std::mem::size_of::<libc::timeval>();

    // SAFETY: `mib` is a valid 2-element MIB for KERN_BOOTTIME.
    // `&mut tv` is a correctly-sized buffer for the returned `struct timeval`.
    // `&mut size` is updated by the kernel to reflect bytes written.
    // The null pointers for `newp`/`newlen` indicate a read-only query.
    let ret = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            std::ptr::addr_of_mut!(tv).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };

    if ret != 0 {
        return Err(substrate_domain::SubstrateError::InternalError {
            reason: format!(
                "sysctl(KERN_BOOTTIME) failed: {}",
                std::io::Error::last_os_error()
            ),
            correlation_id: None,
        });
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| substrate_domain::SubstrateError::InternalError {
            reason: "system clock is before UNIX epoch".to_owned(),
            correlation_id: None,
        })?
        .as_secs();

    #[allow(
        clippy::cast_sign_loss,
        reason = "tv_sec is the boot time; it is always a positive Unix timestamp."
    )]
    let boot_secs = tv.tv_sec as u64;
    Ok(now_secs.saturating_sub(boot_secs))
}

/// Handles a `sys.uptime` tool call.
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` if the platform uptime read fails.
#[instrument(skip(deps))]
pub async fn handle_sys_uptime(deps: Arc<SystemInfoDeps>) -> SubstrateResult<ToolResponse> {
    let _ = deps;
    let seconds = read_uptime_secs()?;
    let record = Uptime {
        seconds,
        human: Uptime::humanize(seconds),
    };
    let content = format!("sys.uptime: {} ({} seconds).", record.human, record.seconds);
    let hints = build_info_hints(Some("sys.info"), None);
    Ok(ToolResponse::with_hints(content, json!(record), hints))
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

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn uptime_is_positive_linux() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_uptime(deps)
            .await
            .expect("sys.uptime must not fail");
        let up: Uptime = serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert!(up.seconds > 0, "uptime must be greater than 0 on Linux");
    }

    #[tokio::test]
    async fn uptime_does_not_error() {
        // Platform-neutral: just asserts no error is returned.
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        handle_sys_uptime(deps)
            .await
            .expect("sys.uptime must not fail on any platform");
    }

    #[test]
    fn humanize_days() {
        let s = Uptime::humanize(90061); // 1d 1h 1m 1s
        assert_eq!(s, "1d 1h 1m 1s");
    }

    #[test]
    fn humanize_hours() {
        let s = Uptime::humanize(3661); // 1h 1m 1s
        assert_eq!(s, "1h 1m 1s");
    }

    #[test]
    fn humanize_minutes() {
        let s = Uptime::humanize(61); // 1m 1s
        assert_eq!(s, "1m 1s");
    }

    #[test]
    fn humanize_seconds_only() {
        let s = Uptime::humanize(42);
        assert_eq!(s, "42s");
    }
}
