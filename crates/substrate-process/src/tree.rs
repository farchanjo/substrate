//! `proc.tree` handler — Zone B (sync I/O via `spawn_blocking`).
//!
//! Builds a parent-child process hierarchy rooted at a requested PID.
//! A single platform scan is performed, then an in-memory adjacency map is
//! built to construct the tree without a second syscall.

use std::collections::HashMap;
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

/// Default maximum tree depth to prevent runaway recursion.
const DEFAULT_MAX_DEPTH: usize = 32;

/// Default (and hard ceiling) for the number of nodes serialized in a single
/// `proc.tree` response.
///
/// Bounds the response payload per ADR-0016 (resource limits). From a wide root
/// such as PID 1, an uncapped tree serializes the entire process forest — 100s
/// of KiB — which overruns model context windows. When the cap is reached the
/// tree is truncated and `truncated: true` is surfaced so the agent can narrow
/// `root_pid` or lower `max_depth`.
const DEFAULT_MAX_NODES: usize = 500;

/// Input parameters for `proc.tree`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ProcTreeRequest {
    /// Root PID for the tree. Defaults to `1` (init/launchd).
    #[serde(default)]
    pub root_pid: Option<u32>,

    /// Maximum traversal depth (default 32).
    #[serde(default)]
    pub max_depth: Option<usize>,

    /// Maximum number of nodes to serialize (default 500, hard ceiling 500).
    ///
    /// Bounds the response payload per ADR-0016. Values above the ceiling are
    /// clamped down; a tree larger than the cap is truncated and the response
    /// carries `truncated: true`.
    #[serde(default)]
    pub max_nodes: Option<usize>,
}

/// A single node in the process tree, carrying its own info and all children.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessTreeNode {
    /// Process information for this node.
    #[serde(flatten)]
    pub info: ProcessInfo,
    /// Direct child nodes in PID-sorted order.
    pub children: Vec<Self>,
}

/// Mutable budget threaded through [`build_node`] to bound the total node count.
struct NodeBudget {
    /// Remaining nodes that may still be added to the tree.
    remaining: usize,
    /// Set to `true` once the cap forced a subtree to be dropped.
    truncated: bool,
}

/// Recursively builds a `ProcessTreeNode` from the adjacency map.
///
/// `depth` enforces the depth cap (`max_depth`); `budget` enforces the total
/// node cap (ADR-0016). Returns `None` once either limit is reached; exhausting
/// the node budget additionally records `budget.truncated = true`.
fn build_node(
    pid: u32,
    adjacency: &HashMap<u32, Vec<ProcessInfo>>,
    by_pid: &HashMap<u32, ProcessInfo>,
    depth: usize,
    max_depth: usize,
    budget: &mut NodeBudget,
) -> Option<ProcessTreeNode> {
    if depth > max_depth {
        return None;
    }
    if budget.remaining == 0 {
        budget.truncated = true;
        return None;
    }

    let info = by_pid.get(&pid)?.clone();
    budget.remaining -= 1;

    let mut children: Vec<ProcessTreeNode> = adjacency
        .get(&pid)
        .map(|kids| {
            let mut sorted = kids.clone();
            sorted.sort_by_key(|p| p.pid);
            sorted
                .into_iter()
                .filter_map(|child| {
                    build_node(child.pid, adjacency, by_pid, depth + 1, max_depth, budget)
                })
                .collect()
        })
        .unwrap_or_default();

    children.sort_by_key(|n| n.info.pid);
    Some(ProcessTreeNode { info, children })
}

/// Handles a `proc.tree` tool call.
///
/// # Errors
///
/// Returns `SubstrateError::NotFound` when the requested root PID does not
/// exist in the scan results.
#[instrument(skip(deps, scanner), fields(root_pid = ?req.root_pid))]
pub async fn handle_proc_tree(
    req: ProcTreeRequest,
    deps: Arc<ProcessDeps>,
    scanner: Arc<dyn ProcessScannerPort>,
) -> SubstrateResult<ToolResponse> {
    let _ = deps;
    let max_depth = req
        .max_depth
        .unwrap_or(DEFAULT_MAX_DEPTH)
        .min(DEFAULT_MAX_DEPTH);
    let max_nodes = req
        .max_nodes
        .unwrap_or(DEFAULT_MAX_NODES)
        .clamp(1, DEFAULT_MAX_NODES);
    let root_pid = req.root_pid.unwrap_or(1);

    let scanner_clone = Arc::clone(&scanner);
    let all: Vec<ProcessInfo> = spawn_blocking(move || scanner_clone.scan_all())
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error in proc.tree: {e}"),
            correlation_id: None,
        })??;

    // Build a PID-keyed index and a parent→children adjacency map.
    let mut by_pid: HashMap<u32, ProcessInfo> = HashMap::with_capacity(all.len());
    let mut adjacency: HashMap<u32, Vec<ProcessInfo>> = HashMap::new();
    for info in all {
        let pid = info.pid;
        let parent_pid = info.ppid;
        by_pid.insert(pid, info.clone());
        adjacency.entry(parent_pid).or_default().push(info);
    }

    if !by_pid.contains_key(&root_pid) {
        return Err(SubstrateError::NotFound {
            resource: format!("process PID {root_pid}"),
            correlation_id: None,
        });
    }

    let mut budget = NodeBudget {
        remaining: max_nodes,
        truncated: false,
    };
    let tree =
        build_node(root_pid, &adjacency, &by_pid, 0, max_depth, &mut budget).ok_or_else(|| {
            SubstrateError::InternalError {
                reason: format!("failed to build tree rooted at PID {root_pid}"),
                correlation_id: None,
            }
        })?;

    let node_count = count_nodes(&tree);
    let truncated = budget.truncated;
    let content = if truncated {
        format!(
            "proc.tree: {node_count} nodes rooted at PID {root_pid} (depth cap {max_depth}, node cap {max_nodes} — TRUNCATED; narrow root_pid or lower max_depth)."
        )
    } else {
        format!(
            "proc.tree: {node_count} nodes rooted at PID {root_pid} (depth cap {max_depth}, node cap {max_nodes})."
        )
    };

    // Serialize the tree, then attach response-level metadata alongside the
    // flattened root node fields so clients can detect truncation.
    let mut structured = serde_json::to_value(&tree).unwrap_or_else(|_| json!({}));
    if let Some(obj) = structured.as_object_mut() {
        obj.insert("node_count".to_owned(), json!(node_count));
        obj.insert("truncated".to_owned(), json!(truncated));
        obj.insert("node_cap".to_owned(), json!(max_nodes));
    }

    let hints = build_read_hints(Some("proc.signal"), Some("proc.list"));
    Ok(ToolResponse::with_hints(content, structured, hints))
}

/// Counts total nodes in a tree recursively.
fn count_nodes(node: &ProcessTreeNode) -> usize {
    1 + node.children.iter().map(count_nodes).sum::<usize>()
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::match_same_arms,
    reason = "test module — panics and unwraps on assertion failure are the intended behavior"
)]
mod tests {
    use std::sync::Arc;

    use substrate_domain::Capabilities;

    use super::*;
    use crate::response::ProcessDeps;
    use crate::scanner::default_scanner;

    #[tokio::test]
    async fn proc_tree_default_root_returns_tree() {
        let deps = Arc::new(ProcessDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let scanner = default_scanner();
        let req = ProcTreeRequest {
            root_pid: None,
            max_depth: None,
            max_nodes: None,
        };
        // PID 1 may not be accessible in all sandbox environments; just verify
        // the call either succeeds or returns NotFound, not an InternalError.
        let result = handle_proc_tree(req, deps, scanner).await;
        match result {
            Ok(_) | Err(substrate_domain::SubstrateError::NotFound { .. }) => {},
            Err(e) => panic!("unexpected error from proc.tree: {e}"),
        }
    }

    #[tokio::test]
    async fn proc_tree_depth_cap_enforced() {
        let deps = Arc::new(ProcessDeps {
            capabilities: Arc::new(Capabilities::default()),
        });
        let scanner = default_scanner();
        let req = ProcTreeRequest {
            root_pid: None,
            max_depth: Some(999), // beyond DEFAULT_MAX_DEPTH — must be clamped
            max_nodes: None,
        };
        let result = handle_proc_tree(req, deps, scanner).await;
        match result {
            Ok(resp) => {
                // depth cap clamps at DEFAULT_MAX_DEPTH, not the requested 999
                assert!(resp.content.contains("depth cap 32"));
            },
            Err(substrate_domain::SubstrateError::NotFound { .. }) => {},
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    // Regression: a wide tree must be bounded by the node cap. Before the cap,
    // a build rooted at a busy PID serialized the entire forest (100s of KiB).
    #[test]
    fn node_cap_truncates_wide_tree() {
        let mk = |pid: u32, ppid: u32| ProcessInfo {
            pid,
            ppid,
            name: "p".to_owned(),
            command: String::new(),
            uid: 0,
            gid: 0,
            cpu_pct: 0.0,
            rss_kb: 0,
            vm_kb: 0,
            start_time_unix: None,
            state: "R".to_owned(),
        };

        let mut by_pid: HashMap<u32, ProcessInfo> = HashMap::new();
        let mut adjacency: HashMap<u32, Vec<ProcessInfo>> = HashMap::new();
        by_pid.insert(1, mk(1, 0));
        for pid in 2..=601u32 {
            let info = mk(pid, 1);
            by_pid.insert(pid, info.clone());
            adjacency.entry(1).or_default().push(info);
        }

        let mut budget = NodeBudget {
            remaining: 10,
            truncated: false,
        };
        let tree = build_node(1, &adjacency, &by_pid, 0, 32, &mut budget)
            .expect("root node must build within budget");
        assert!(
            budget.truncated,
            "a 601-node tree must truncate at a 10-node cap"
        );
        assert_eq!(count_nodes(&tree), 10, "exactly the cap is retained");
    }
}
