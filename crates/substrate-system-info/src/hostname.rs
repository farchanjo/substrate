//! `sys.hostname` handler — Zone A (sync inline).
//!
//! Returns the system hostname via `nix::unistd::gethostname()`, which wraps
//! the POSIX `gethostname(2)` syscall. Available on both Linux and macOS
//! without any unsafe code.

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use tracing::instrument;

use crate::{
    hints_helpers::build_info_hints,
    response::{SystemInfoDeps, ToolResponse},
};
use substrate_domain::SubstrateResult;

/// Hostname record returned by `sys.hostname`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct HostName {
    /// Short hostname as reported by the OS (e.g., `"myhost"` or `"myhost.example.com"`).
    pub hostname: String,
}

/// Handles a `sys.hostname` tool call.
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` if `gethostname(2)` fails or the
/// hostname bytes are not valid UTF-8.
#[instrument(skip(deps))]
pub async fn handle_sys_hostname(deps: Arc<SystemInfoDeps>) -> SubstrateResult<ToolResponse> {
    let _ = deps;

    let raw = nix::unistd::gethostname().map_err(|e| {
        substrate_domain::SubstrateError::InternalError {
            reason: format!("gethostname(2) failed: {e}"),
            correlation_id: None,
        }
    })?;

    let hostname =
        raw.into_string()
            .map_err(|_| substrate_domain::SubstrateError::EncodingError {
                detail: "hostname contains non-UTF-8 bytes".to_owned(),
                correlation_id: None,
            })?;

    let record = HostName {
        hostname: hostname.clone(),
    };
    let content = format!("sys.hostname: {hostname}.");
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

    #[tokio::test]
    async fn hostname_is_non_empty() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_hostname(deps)
            .await
            .expect("sys.hostname must not fail");
        let record: HostName = serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert!(!record.hostname.is_empty(), "hostname must not be empty");
    }

    #[tokio::test]
    async fn hostname_content_mentions_hostname() {
        let deps = Arc::new(SystemInfoDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let resp = handle_sys_hostname(deps)
            .await
            .expect("sys.hostname must not fail");
        assert!(
            resp.content.starts_with("sys.hostname:"),
            "content prefix must match tool name"
        );
    }
}
