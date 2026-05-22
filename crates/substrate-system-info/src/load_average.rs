//! `sys.load_average` handler — Zone A (sync inline).
//!
//! The macOS implementation calls `libc::getloadavg` directly; see the
//! `read_load_average` function for full safety justification (ADR-0042 + ADR-0044).
// macOS getloadavg FFI — module-level allow per ADR-0042 + ADR-0044 carve-out.
#![cfg_attr(
    target_os = "macos",
    allow(
        unsafe_code,
        reason = "libc::getloadavg FFI on macOS; standard POSIX read call. ADR-0042 + ADR-0044 sysctl carve-out."
    )
)]
//!
//! Returns 1-, 5-, and 15-minute CPU load averages.
//!
//! # Platform strategy
//!
//! - **Linux**: `nix::sys::sysinfo::sysinfo()` returns a `Sysinfo` struct
//!   whose `load_average()` method yields `(f64, f64, f64)`. Pure safe Rust.
//! - **macOS**: `libc::getloadavg(buf: *mut f64, nelem: c_int)` fills a
//!   3-element array. Requires a narrow `unsafe` block scoped to this module
//!   (ADR-0042 + ADR-0044 carve-out).

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use tracing::instrument;

use crate::{
    hints_helpers::build_info_hints,
    response::{SystemInfoDeps, ToolResponse},
};
use substrate_domain::SubstrateResult;

/// Load average triplet returned by `sys.load_average`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct LoadAverage {
    /// 1-minute exponential moving average of runnable processes.
    pub load_1: f64,
    /// 5-minute exponential moving average of runnable processes.
    pub load_5: f64,
    /// 15-minute exponential moving average of runnable processes.
    pub load_15: f64,
}

// ---- Linux implementation ---------------------------------------------------

/// Platform-internal load reader, also re-used by `info.rs`.
#[cfg(target_os = "linux")]
pub(crate) fn read_load_average_pub() -> SubstrateResult<LoadAverage> {
    read_load_average()
}

#[cfg(target_os = "linux")]
fn read_load_average() -> SubstrateResult<LoadAverage> {
    let info = nix::sys::sysinfo::sysinfo().map_err(|e| {
        substrate_domain::SubstrateError::InternalError {
            reason: format!("sysinfo(2) failed: {e}"),
            correlation_id: None,
        }
    })?;
    let (load_1, load_5, load_15) = info.load_average();
    Ok(LoadAverage {
        load_1,
        load_5,
        load_15,
    })
}

// ---- macOS implementation ---------------------------------------------------
//
// Uses libc::getloadavg(buf, 3) — a single POSIX function that fills a
// 3-element f64 array with 1-, 5-, and 15-minute load averages.
//
// Safety justification (ADR-0042 + ADR-0044 sysctl FFI carve-out):
// `libc::getloadavg` is a standard POSIX function available on macOS.
// The call writes into a stack-allocated array; no pointer escapes the frame.

/// Platform-internal load reader, also re-used by `info.rs`.
#[cfg(target_os = "macos")]
pub(crate) fn read_load_average_pub() -> SubstrateResult<LoadAverage> {
    read_load_average()
}

#[cfg(target_os = "macos")]
fn read_load_average() -> SubstrateResult<LoadAverage> {
    let mut buf = [0.0_f64; 3];

    // SAFETY: `buf` is a stack-allocated 3-element f64 array.
    // `getloadavg` writes exactly `nelem` doubles into the buffer.
    // The return value is the number of samples written (3 on success, -1
    // on failure). No pointer escapes this call.
    let ret = unsafe { libc::getloadavg(buf.as_mut_ptr(), 3) };

    if ret < 0 {
        return Err(substrate_domain::SubstrateError::InternalError {
            reason: format!("getloadavg(3) failed: {}", std::io::Error::last_os_error()),
            correlation_id: None,
        });
    }

    Ok(LoadAverage {
        load_1: buf[0],
        load_5: buf[1],
        load_15: buf[2],
    })
}

/// Handles a `sys.load_average` tool call.
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` if the platform load read fails.
#[instrument(skip(deps))]
pub async fn handle_sys_load_average(deps: Arc<SystemInfoDeps>) -> SubstrateResult<ToolResponse> {
    let _ = deps;
    let avg = read_load_average()?;
    let content = format!(
        "sys.load_average: 1m={:.2} 5m={:.2} 15m={:.2}.",
        avg.load_1, avg.load_5, avg.load_15
    );
    let hints = build_info_hints(Some("sys.info"), None);
    Ok(ToolResponse::with_hints(content, json!(avg), hints))
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
    async fn load_average_returns_three_values() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_load_average(deps)
            .await
            .expect("sys.load_average must not fail");
        let avg: LoadAverage = serde_json::from_value(resp.structured_content).expect("valid JSON");
        // Load averages are non-negative on any real system.
        assert!(avg.load_1 >= 0.0, "load_1 must be non-negative");
        assert!(avg.load_5 >= 0.0, "load_5 must be non-negative");
        assert!(avg.load_15 >= 0.0, "load_15 must be non-negative");
    }

    #[tokio::test]
    async fn load_average_content_format() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_load_average(deps)
            .await
            .expect("sys.load_average must not fail");
        assert!(resp.content.starts_with("sys.load_average:"));
    }

    #[tokio::test]
    async fn load_average_values_are_non_nan() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_load_average(deps)
            .await
            .expect("sys.load_average must not fail");
        let avg: LoadAverage = serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert!(!avg.load_1.is_nan(), "load_1 must not be NaN");
        assert!(!avg.load_5.is_nan(), "load_5 must not be NaN");
        assert!(!avg.load_15.is_nan(), "load_15 must not be NaN");
    }

    #[tokio::test]
    async fn load_average_values_are_finite() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_load_average(deps)
            .await
            .expect("sys.load_average must not fail");
        let avg: LoadAverage = serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert!(avg.load_1.is_finite(), "load_1 must be finite");
        assert!(avg.load_5.is_finite(), "load_5 must be finite");
        assert!(avg.load_15.is_finite(), "load_15 must be finite");
    }
}
