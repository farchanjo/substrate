//! `proc.top` handler — Zone B (`spawn_blocking`).
//!
//! Returns a paginated, sorted, optionally filtered list of per-process resource
//! snapshots. Builds on `proc.stats` per-process reads and applies the sort +
//! filter + pagination contract from ADR-0051.
//!
//! # Sort orders
//!
//! | `sort_by` | Field sorted |
//! |-----------|--------------|
//! | `mem`     | `rss_bytes` descending (default) |
//! | `cpu`     | `cpu_pct` descending |
//! | `pid`     | `pid` ascending |
//! | `fds`     | `fds` (None treated as 0) descending |
//!
//! # Pagination
//!
//! Uses a simple integer offset cursor encoded as base64 to remain opaque to
//! callers per ADR-0008. The cursor encodes the start index into the sorted,
//! filtered list.
//!
//! # See also
//!
//! [ADR-0051](../../../docs/arch/adr/0051-per-process-resource-stats.md)

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::task::spawn_blocking;
use tracing::instrument;

use crate::{
    hints_helpers::build_read_hints,
    response::{ProcessDeps, ToolResponse},
    scanner::ProcessScannerPort,
    stats::{PidCpuCache, ProcessStats, SharedPidCpuCache, read_process_stats},
};
use substrate_domain::SubstrateResult;

// ---- Request types ----------------------------------------------------------

/// Sort key for `proc.top`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TopSortBy {
    /// Sort by resident set size, highest first (default).
    #[default]
    Mem,
    /// Sort by CPU utilization, highest first.
    Cpu,
    /// Sort by PID, lowest first.
    Pid,
    /// Sort by open file descriptor count, highest first.
    Fds,
}

/// Filter criteria for `proc.top`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TopFilter {
    /// Restrict to processes owned by this UID.
    pub uid: Option<u32>,
    /// Case-insensitive substring match against the `command` field.
    pub command_substring: Option<String>,
    /// Minimum `rss_bytes` threshold.
    pub min_rss_bytes: Option<u64>,
    /// Minimum `cpu_pct` threshold.
    pub min_cpu_pct: Option<f32>,
}

/// Input parameters for `proc.top`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcTopRequest {
    /// Sort key (default: `mem`).
    #[serde(default)]
    pub sort_by: TopSortBy,

    /// Maximum entries to return (default 20, max 200).
    #[serde(default)]
    pub limit: Option<u32>,

    /// Optional filter criteria.
    #[serde(default)]
    pub filter: TopFilter,

    /// Opaque base64 pagination cursor from a previous `proc.top` response.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Result envelope for `proc.top`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcTopResult {
    /// Sorted, filtered, paginated process list.
    pub processes: Vec<ProcessStats>,
    /// Opaque cursor for the next page; `None` when this is the last page.
    pub next_cursor: Option<String>,
    /// Total number of processes matching the filter (before pagination).
    pub total_matching: usize,
}

// ---- Constants --------------------------------------------------------------

/// Default page size per ADR-0051 `proc.top` definition.
const DEFAULT_LIMIT: u32 = 20;
/// Hard cap per ADR-0051.
const MAX_LIMIT: u32 = 200;

/// Base64 alphabet (RFC 4648 standard).
const B64_ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Decode table: 0xFF = invalid byte; otherwise the 6-bit value.
const B64_DECODE: [u8; 256] = {
    let mut t = [0xFFu8; 256];
    let mut i = 0usize;
    while i < 64 {
        #[expect(clippy::cast_possible_truncation, reason = "i < 64 always fits in u8")]
        {
            t[B64_ALPHA[i] as usize] = i as u8;
        }
        i += 1;
    }
    t
};

// ---- Cursor encode/decode ---------------------------------------------------

/// Encodes a start-offset integer as a base64-opaque cursor string.
///
/// Uses a lightweight hand-rolled base64 encoder to avoid pulling in the
/// `base64_simd` workspace dep into this crate. The cursor is opaque to callers
/// per ADR-0008; only the structure (base64url-encoded decimal offset) matters.
fn encode_cursor(offset: usize) -> String {
    // Encode the ASCII decimal representation of `offset` in standard base64.
    let s = offset.to_string();
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let combined = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_ALPHA[((combined >> 18) & 0x3F) as usize]);
        out.push(B64_ALPHA[((combined >> 12) & 0x3F) as usize]);
        if chunk.len() >= 2 {
            out.push(B64_ALPHA[((combined >> 6) & 0x3F) as usize]);
        }
        if chunk.len() == 3 {
            out.push(B64_ALPHA[(combined & 0x3F) as usize]);
        }
    }
    // SAFETY: all pushed bytes are ASCII from B64_ALPHA; UTF-8 validity guaranteed.
    String::from_utf8(out).unwrap_or_default()
}

/// Decodes a base64-opaque cursor string into a start-offset integer.
/// Returns `0` (start of list) on any decode error.
fn decode_cursor(cursor: &str) -> usize {
    let bytes = cursor.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4 + 1);
    let mut i = 0;
    while i + 1 < bytes.len() {
        let c0 = B64_DECODE[bytes[i] as usize];
        let c1 = B64_DECODE[bytes[i + 1] as usize];
        if c0 == 0xFF || c1 == 0xFF {
            return 0;
        }
        out.push((c0 << 2) | (c1 >> 4));
        if i + 2 < bytes.len() {
            let c2 = B64_DECODE[bytes[i + 2] as usize];
            if c2 == 0xFF {
                break;
            }
            out.push((c1 << 4) | (c2 >> 2));
        }
        if i + 3 < bytes.len() {
            let c3 = B64_DECODE[bytes[i + 3] as usize];
            if c3 == 0xFF {
                break;
            }
            out.push((B64_DECODE[bytes[i + 2] as usize] << 6) | c3);
        }
        i += 4;
    }
    std::str::from_utf8(&out)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

// ---- Sort comparators -------------------------------------------------------

fn compare_by(a: &ProcessStats, b: &ProcessStats, sort_by: TopSortBy) -> std::cmp::Ordering {
    match sort_by {
        TopSortBy::Mem => b.rss_bytes.cmp(&a.rss_bytes),
        TopSortBy::Cpu => b
            .cpu_pct
            .partial_cmp(&a.cpu_pct)
            .unwrap_or(std::cmp::Ordering::Equal),
        TopSortBy::Pid => a.pid.cmp(&b.pid),
        TopSortBy::Fds => {
            let fa = a.fds.unwrap_or(0);
            let fb = b.fds.unwrap_or(0);
            fb.cmp(&fa)
        },
    }
}

// ---- Filter -----------------------------------------------------------------

fn apply_filter(stats: &ProcessStats, filter: &TopFilter) -> bool {
    if let Some(uid) = filter.uid
        && stats.uid != uid
    {
        return false;
    }
    if let Some(ref substr) = filter.command_substring
        && !stats
            .command
            .to_lowercase()
            .contains(&substr.to_lowercase())
    {
        return false;
    }
    if let Some(min_rss) = filter.min_rss_bytes
        && stats.rss_bytes < min_rss
    {
        return false;
    }
    if let Some(min_cpu) = filter.min_cpu_pct
        && stats.cpu_pct < min_cpu
    {
        return false;
    }
    true
}

// ---- Core implementation ----------------------------------------------------

/// Enumerates all visible PIDs on the current platform.
///
/// Returns a `Vec<u32>` of live PIDs. Uses the platform scanner's `scan_all`
/// to get the PID list, then reads detailed stats for each via `read_process_stats`.
fn enumerate_all_pids(scanner: &dyn ProcessScannerPort) -> SubstrateResult<Vec<u32>> {
    let all = scanner.scan_all()?;
    Ok(all.iter().map(|p| p.pid).collect())
}

/// Reads `proc.top` synchronously: enumerate PIDs, read per-process stats,
/// filter, sort, paginate.
fn read_top_sync(
    sort_by: TopSortBy,
    limit: u32,
    filter: &TopFilter,
    start_offset: usize,
    scanner: &dyn ProcessScannerPort,
    cache: &mut PidCpuCache,
) -> SubstrateResult<ProcTopResult> {
    let pids = enumerate_all_pids(scanner)?;

    let mut stats: Vec<ProcessStats> = pids
        .into_iter()
        .filter_map(|pid| {
            // Process may have exited between enumeration and stats read;
            // silently skip it (matches ADR-0051 scan-level error policy).
            read_process_stats(pid, cache).ok()
        })
        .filter(|s| apply_filter(s, filter))
        .collect();

    // Sort.
    stats.sort_unstable_by(|a, b| compare_by(a, b, sort_by));

    let total_matching = stats.len();
    let start = start_offset.min(total_matching);
    let end = start.saturating_add(limit as usize).min(total_matching);
    let page = stats[start..end].to_vec();
    let next_cursor = if end < total_matching {
        Some(encode_cursor(end))
    } else {
        None
    };

    Ok(ProcTopResult {
        processes: page,
        next_cursor,
        total_matching,
    })
}

// ---- Handler ----------------------------------------------------------------

/// Handles a `proc.top` tool call.
///
/// Returns a paginated, sorted, filtered list of per-process resource snapshots
/// per ADR-0051. CPU% is `0.0` for any PID on its first appearance in the cache.
///
/// # Errors
///
/// Returns `SubstrateError::InternalError` when the platform process enumeration
/// fails (e.g., `/proc` unmounted on Linux).
#[instrument(skip(deps, scanner, cpu_cache), fields(sort_by = ?req.sort_by, limit = ?req.limit))]
pub async fn handle_proc_top(
    req: ProcTopRequest,
    deps: Arc<ProcessDeps>,
    scanner: Arc<dyn ProcessScannerPort>,
    cpu_cache: SharedPidCpuCache,
) -> SubstrateResult<ToolResponse> {
    let _ = deps;

    let limit = req.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let sort_by = req.sort_by;
    let filter = req.filter.clone();
    let start_offset = req.cursor.as_deref().map_or(0, decode_cursor);

    let result = spawn_blocking(move || {
        let mut cache = cpu_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read_top_sync(
            sort_by,
            limit,
            &filter,
            start_offset,
            scanner.as_ref(),
            &mut cache,
        )
    })
    .await
    .map_err(|e| substrate_domain::SubstrateError::InternalError {
        reason: format!("spawn_blocking join error in proc.top: {e}"),
        correlation_id: None,
    })??;

    let content = format!(
        "proc.top: {} processes (sort_by={sort_by:?}, limit={limit}, total_matching={}).",
        result.processes.len(),
        result.total_matching,
    );

    let hints = build_read_hints(Some("proc.stats"), Some("proc.signal"));

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
    reason = "test module — panics and unwraps on assertion failure are the intended behavior"
)]
mod tests {
    use std::sync::Arc;

    use substrate_domain::Capabilities;

    use super::*;
    use crate::response::ProcessDeps;
    use crate::scanner::default_scanner;
    use crate::stats::new_pid_cpu_cache;

    fn make_deps() -> Arc<ProcessDeps> {
        Arc::new(ProcessDeps {
            capabilities: Arc::new(Capabilities::default()),
        })
    }

    #[tokio::test]
    async fn proc_top_returns_at_least_one_process() {
        let deps = make_deps();
        let scanner = default_scanner();
        let cache = new_pid_cpu_cache();
        let req = ProcTopRequest {
            sort_by: TopSortBy::Mem,
            limit: None,
            filter: TopFilter::default(),
            cursor: None,
        };
        let resp = handle_proc_top(req, deps, scanner, cache)
            .await
            .expect("proc.top must not fail");
        let result: ProcTopResult =
            serde_json::from_value(resp.structured_content).expect("valid ProcTopResult JSON");
        assert!(
            result.total_matching > 0,
            "proc.top must return at least one process"
        );
    }

    #[tokio::test]
    async fn proc_top_limit_is_respected() {
        let deps = make_deps();
        let scanner = default_scanner();
        let cache = new_pid_cpu_cache();
        let req = ProcTopRequest {
            sort_by: TopSortBy::Pid,
            limit: Some(3),
            filter: TopFilter::default(),
            cursor: None,
        };
        let resp = handle_proc_top(req, deps, scanner, cache)
            .await
            .expect("proc.top must not fail");
        let result: ProcTopResult =
            serde_json::from_value(resp.structured_content).expect("valid JSON");
        assert!(
            result.processes.len() <= 3,
            "returned processes must not exceed limit=3"
        );
    }

    #[tokio::test]
    async fn proc_top_mem_sort_is_descending() {
        let deps = make_deps();
        let scanner = default_scanner();
        let cache = new_pid_cpu_cache();
        let req = ProcTopRequest {
            sort_by: TopSortBy::Mem,
            limit: Some(10),
            filter: TopFilter::default(),
            cursor: None,
        };
        let resp = handle_proc_top(req, deps, scanner, cache)
            .await
            .expect("proc.top must not fail");
        let result: ProcTopResult =
            serde_json::from_value(resp.structured_content).expect("valid JSON");
        let rss: Vec<u64> = result.processes.iter().map(|p| p.rss_bytes).collect();
        let sorted = rss.windows(2).all(|w| w[0] >= w[1]);
        assert!(sorted, "processes must be sorted by rss_bytes descending");
    }

    #[tokio::test]
    async fn proc_top_pid_sort_is_ascending() {
        let deps = make_deps();
        let scanner = default_scanner();
        let cache = new_pid_cpu_cache();
        let req = ProcTopRequest {
            sort_by: TopSortBy::Pid,
            limit: Some(10),
            filter: TopFilter::default(),
            cursor: None,
        };
        let resp = handle_proc_top(req, deps, scanner, cache)
            .await
            .expect("proc.top must not fail");
        let result: ProcTopResult =
            serde_json::from_value(resp.structured_content).expect("valid JSON");
        let pids: Vec<u32> = result.processes.iter().map(|p| p.pid).collect();
        let sorted = pids.windows(2).all(|w| w[0] <= w[1]);
        assert!(sorted, "processes must be sorted by pid ascending");
    }

    #[tokio::test]
    async fn proc_top_pagination_cursor_advances() {
        let deps = make_deps();
        let scanner = default_scanner();
        let cache = new_pid_cpu_cache();
        let req = ProcTopRequest {
            sort_by: TopSortBy::Pid,
            limit: Some(2),
            filter: TopFilter::default(),
            cursor: None,
        };
        let resp = handle_proc_top(
            req,
            Arc::clone(&deps),
            Arc::clone(&scanner),
            Arc::clone(&cache),
        )
        .await
        .expect("first proc.top page must not fail");
        let result: ProcTopResult =
            serde_json::from_value(resp.structured_content).expect("valid JSON");

        if result.total_matching <= 2 {
            // Not enough processes to paginate; just assert cursor is None.
            assert!(
                result.next_cursor.is_none(),
                "single page must have no cursor"
            );
            return;
        }

        let cursor = result
            .next_cursor
            .expect("must have a next cursor with >2 processes");
        let req2 = ProcTopRequest {
            sort_by: TopSortBy::Pid,
            limit: Some(2),
            filter: TopFilter::default(),
            cursor: Some(cursor),
        };
        let resp2 = handle_proc_top(req2, deps, scanner, cache)
            .await
            .expect("second proc.top page must not fail");
        let result2: ProcTopResult =
            serde_json::from_value(resp2.structured_content).expect("valid JSON");
        // The first PID on page 2 must be different from page 1.
        if let Some(p2_first) = result2.processes.first() {
            assert!(
                !result.processes.iter().any(|p| p.pid == p2_first.pid),
                "page 2 must start after page 1"
            );
        }
    }

    #[test]
    fn cursor_encode_decode_roundtrip() {
        for offset in [0usize, 1, 42, 200, 4095, 99_999] {
            let encoded = encode_cursor(offset);
            let decoded = decode_cursor(&encoded);
            assert_eq!(
                decoded, offset,
                "cursor roundtrip failed for offset {offset}"
            );
        }
    }

    #[test]
    fn cursor_decode_garbage_returns_zero() {
        assert_eq!(decode_cursor("!!!invalid!!!"), 0);
        assert_eq!(decode_cursor(""), 0);
    }

    #[test]
    fn filter_uid_excludes_wrong_uid() {
        let stats = ProcessStats {
            pid: 1,
            rss_bytes: 0,
            virt_bytes: 0,
            cpu_pct: 0.0,
            threads: 1,
            fds: None,
            uid: 500,
            start_time: 0,
            state: crate::stats::ProcessState::Running,
            command: "test".to_owned(),
        };
        let filter = TopFilter {
            uid: Some(999),
            ..TopFilter::default()
        };
        assert!(
            !apply_filter(&stats, &filter),
            "uid=999 filter must exclude uid=500 process"
        );
    }

    #[test]
    fn filter_command_substring_is_case_insensitive() {
        let stats = ProcessStats {
            pid: 1,
            rss_bytes: 0,
            virt_bytes: 0,
            cpu_pct: 0.0,
            threads: 1,
            fds: None,
            uid: 0,
            start_time: 0,
            state: crate::stats::ProcessState::Running,
            command: "MyServiceWorker".to_owned(),
        };
        let filter = TopFilter {
            command_substring: Some("serviceworker".to_owned()),
            ..TopFilter::default()
        };
        assert!(
            apply_filter(&stats, &filter),
            "case-insensitive match must include the process"
        );
    }
}
