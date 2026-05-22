//! `proc.list` handler — Zone B (sync I/O via `spawn_blocking`).
//!
//! Returns a paginated, optionally filtered snapshot of running processes.
//! The platform scanner is called inside `tokio::task::spawn_blocking`
//! because it performs synchronous `/proc` or `sysctl` reads.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::task::spawn_blocking;
use tracing::instrument;

use crate::{
    hints_helpers::build_read_hints,
    process_info::ProcessInfo,
    response::{ProcessDeps, ToolResponse},
    scanner::ProcessScannerPort,
};
use substrate_domain::{SubstrateError, SubstrateResult};

/// Hard cap on processes returned in a single response (defense in depth).
const MAX_PROCESSES: usize = 10_000;

/// Default page size per ADR-0008.
const DEFAULT_PAGE_SIZE: usize = 100;

/// Maximum page size per ADR-0008.
const MAX_PAGE_SIZE: usize = 1_000;

/// Input parameters for `proc.list`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ProcListRequest {
    /// Filter: only return processes whose `name` matches this glob pattern.
    /// Uses simple substring containment (case-insensitive) in the MVP;
    /// full glob matching is a Wave G enhancement.
    #[serde(default)]
    pub name_filter: Option<String>,

    /// Filter: only return processes owned by this UID.
    #[serde(default)]
    pub uid_filter: Option<u32>,

    /// Filter: only return direct children of this PPID.
    #[serde(default)]
    pub parent_pid_filter: Option<u32>,

    /// Page size (default 100, max 1000).
    #[serde(default)]
    pub page_size: Option<usize>,

    /// Zero-based page index (default 0).
    #[serde(default)]
    pub page: Option<usize>,
}

/// Paginated result envelope for `proc.list`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ProcListResult {
    /// Processes on this page.
    pub processes: Vec<ProcessInfo>,
    /// Total number of processes matching the filter (before pagination).
    pub total_matching: usize,
    /// Current zero-based page index.
    pub page: usize,
    /// Effective page size applied.
    pub page_size: usize,
    /// `true` when more pages are available.
    pub has_next_page: bool,
}

/// Handles a `proc.list` tool call.
///
/// # Errors
///
/// Returns `SubstrateError` if the platform scanner fails to open the process
/// table (e.g., `/proc` unmounted on Linux).
#[instrument(skip(deps, scanner), fields(name_filter = ?req.name_filter, uid = ?req.uid_filter))]
pub async fn handle_proc_list(
    req: ProcListRequest,
    deps: Arc<ProcessDeps>,
    scanner: Arc<dyn ProcessScannerPort>,
) -> SubstrateResult<ToolResponse> {
    let _ = deps; // capabilities available for future tier annotation

    let scanner_clone = Arc::clone(&scanner);
    let mut all: Vec<ProcessInfo> = spawn_blocking(move || scanner_clone.scan_all())
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error in proc.list: {e}"),
            correlation_id: None,
        })??;

    // Apply filters.
    if let Some(ref name_pat) = req.name_filter {
        let pat_lower = name_pat.to_lowercase();
        all.retain(|p| p.name.to_lowercase().contains(&pat_lower));
    }
    if let Some(uid) = req.uid_filter {
        all.retain(|p| p.uid == uid);
    }
    if let Some(ppid) = req.parent_pid_filter {
        all.retain(|p| p.ppid == ppid);
    }

    // Apply hard cap before pagination.
    all.truncate(MAX_PROCESSES);

    let total_matching = all.len();
    let page_size = req
        .page_size
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .min(MAX_PAGE_SIZE);
    let page = req.page.unwrap_or(0);

    let start = page.saturating_mul(page_size);
    let end = start.saturating_add(page_size).min(total_matching);
    let page_slice = if start < total_matching {
        all[start..end].to_vec()
    } else {
        Vec::new()
    };
    let has_next_page = end < total_matching;

    let result = ProcListResult {
        processes: page_slice,
        total_matching,
        page,
        page_size,
        has_next_page,
    };

    let content = format!(
        "proc.list: {} processes (page {}/{}).",
        result.processes.len(),
        page,
        total_matching.div_ceil(page_size.max(1))
    );

    let hints = build_read_hints(Some("proc.signal"), Some("proc.tree"));

    Ok(ToolResponse::with_hints(content, json!(result), hints))
}

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
    use crate::response::ProcessDeps;
    use crate::scanner::default_scanner;

    #[tokio::test]
    async fn proc_list_returns_at_least_one_process() {
        let deps = Arc::new(ProcessDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let scanner = default_scanner();
        let req = ProcListRequest {
            name_filter: None,
            uid_filter: None,
            parent_pid_filter: None,
            page_size: None,
            page: None,
        };
        let resp = handle_proc_list(req, deps, scanner).await;
        let resp = resp.expect("proc.list must not fail");
        let result: ProcListResult =
            serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert!(
            result.total_matching > 0,
            "expected at least the test runner process in the list"
        );
    }

    #[tokio::test]
    async fn proc_list_pagination_caps_page_size() {
        let deps = Arc::new(ProcessDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let scanner = default_scanner();
        let req = ProcListRequest {
            name_filter: None,
            uid_filter: None,
            parent_pid_filter: None,
            page_size: Some(2),
            page: Some(0),
        };
        let resp = handle_proc_list(req, deps, scanner)
            .await
            .expect("proc.list must not fail");
        let result: ProcListResult =
            serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert!(
            result.processes.len() <= 2,
            "page_size=2 must not return more than 2 entries"
        );
    }
}
