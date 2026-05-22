//! `sys.uname` handler — Zone A (sync inline).
//!
//! Returns kernel name, release, version, and machine architecture via
//! `nix::sys::utsname::uname()`, which maps to the POSIX `uname(2)` syscall.
//! Available on both Linux and macOS without any unsafe code.

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use tracing::instrument;

use crate::{
    hints_helpers::build_info_hints,
    response::{SystemInfoDeps, ToolResponse},
};
use substrate_domain::SubstrateResult;

/// Structured kernel version record returned by `sys.uname`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct KernelVersion {
    /// OS kernel name (e.g., `"Linux"` or `"Darwin"`).
    pub sysname: String,
    /// Short hostname (same as `sys.hostname`; included for context).
    pub nodename: String,
    /// Kernel release string (e.g., `"6.8.0-50-generic"`).
    pub release: String,
    /// Kernel version/build string.
    pub version: String,
    /// Machine hardware architecture (e.g., `"x86_64"` or `"arm64"`).
    pub machine: String,
}

/// Handles a `sys.uname` tool call.
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` if `uname(2)` fails (extremely
/// unlikely; the syscall has no failure mode on a running kernel).
#[instrument(skip(deps))]
pub async fn handle_sys_uname(deps: Arc<SystemInfoDeps>) -> SubstrateResult<ToolResponse> {
    let _ = deps;

    let uts = nix::sys::utsname::uname().map_err(|e| {
        substrate_domain::SubstrateError::InternalError {
            reason: format!("uname(2) failed: {e}"),
            correlation_id: None,
        }
    })?;

    let kv = KernelVersion {
        sysname: uts.sysname().to_string_lossy().into_owned(),
        nodename: uts.nodename().to_string_lossy().into_owned(),
        release: uts.release().to_string_lossy().into_owned(),
        version: uts.version().to_string_lossy().into_owned(),
        machine: uts.machine().to_string_lossy().into_owned(),
    };

    let content = format!(
        "sys.uname: {} {} {} {}.",
        kv.sysname, kv.release, kv.version, kv.machine
    );

    let hints = build_info_hints(Some("sys.info"), None);

    Ok(ToolResponse::with_hints(content, json!(kv), hints))
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
    async fn uname_returns_non_empty_sysname() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_uname(deps)
            .await
            .expect("sys.uname must not fail");
        let kv: KernelVersion = serde_json::from_value(resp.structured_content)
            .expect("structured content must be valid KernelVersion JSON");
        assert!(
            !kv.sysname.is_empty(),
            "sysname must not be empty on any platform"
        );
    }

    #[tokio::test]
    async fn uname_machine_non_empty() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_uname(deps)
            .await
            .expect("sys.uname must not fail");
        let kv: KernelVersion =
            serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert!(!kv.machine.is_empty(), "machine must not be empty");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn uname_sysname_is_linux() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_uname(deps)
            .await
            .expect("sys.uname must not fail");
        let kv: KernelVersion =
            serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert_eq!(kv.sysname, "Linux");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn uname_sysname_is_darwin() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_uname(deps)
            .await
            .expect("sys.uname must not fail");
        let kv: KernelVersion =
            serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert_eq!(kv.sysname, "Darwin");
    }
}
