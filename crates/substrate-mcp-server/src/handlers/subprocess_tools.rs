//! MCP tool handlers for subprocess.* tools per ADR-0052.
//!
//! All five handlers are compiled only when the `subprocess` Cargo feature is
//! active. They delegate to `Arc<dyn SubprocessPort>` wired by the composition
//! root and convert domain results to `DispatchedResponse` envelopes.
//!
//! Tool descriptions are thin one-liners per the ADR-0007 2026-05-22 amendment;
//! full narrative arc lives in the companion `subprocess.md` tool-card document.

#![cfg(feature = "subprocess")]
#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::instrument;

use substrate_domain::{
    SubstrateError, SubstrateResult,
    ports::subprocess::{CancelSignal, SignalTarget, SubprocessSignalName},
    subprocess::errors::SubprocessError,
    subprocess::pagination::{Pagination, SubprocessSearchRequest},
    subprocess::request::SubprocessRequest,
    subprocess::state::SubprocessState,
    value_objects::JobId,
};

use crate::handlers::dispatcher::DispatchedResponse;

// ---- CancelSignal shim for the composition layer ----------------------------

/// A trivial always-not-cancelled `CancelSignal` implementation used at the
/// MCP handler layer. Callers that need real cancellation should pass a
/// `tokio_util::sync::CancellationToken`-backed implementation; this shim is
/// adequate for the composition root which drives cooperative cancellation via
/// `SubprocessRegistry`'s own `root_cancel` token.
struct NoCancel;

#[async_trait]
impl CancelSignal for NoCancel {
    fn is_cancelled(&self) -> bool {
        false
    }

    async fn cancelled(&self) {
        // Never resolves — NoCancel is never cancelled.
        std::future::pending::<()>().await;
    }
}

// ---- Error helpers ----------------------------------------------------------

/// Maps a [`SubprocessError`] to a [`SubstrateError`] carrying the
/// `SUBSTRATE_INTERNAL_ERROR` code and a recovery hint per ADR-0010.
fn subprocess_err(e: &SubprocessError) -> SubstrateError {
    SubstrateError::InternalError {
        reason: e.to_string(),
        correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
    }
}

// ---- subprocess_spawn -------------------------------------------------------

/// Dispatches `subprocess.spawn` — spawn a supervised child process.
///
/// Delegates to the wired `SubprocessPort::spawn`. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_subprocess_spawn(
    args: Value,
    port: Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: SubprocessRequest =
        serde_json::from_value(args.clone()).map_err(|e| SubstrateError::InvalidArgument {
            offending_field: "arguments".to_owned(),
            reason: e.to_string(),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        })?;

    let handle = port
        .spawn(req, &NoCancel)
        .await
        .map_err(|e| subprocess_err(&e))?;

    let content = format!(
        "subprocess.spawn: job_id={} pgid={} state={:?}.",
        handle.job_id,
        handle.process_group.pgid(),
        handle.state,
    );
    let structured = json!({
        "job_id": handle.job_id,
        "process_group": {
            "pid": handle.process_group.pid(),
            "pgid": handle.process_group.pgid(),
        },
        "state": handle.state,
        "started_at": handle.started_at,
    });
    let hints = substrate_domain::Hints {
        next_action_suggested: Some("subprocess.result".to_owned()),
        ..substrate_domain::Hints::default()
    };
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- subprocess_list --------------------------------------------------------

/// Request type for `subprocess.list`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct SubprocessListRequest {
    /// Restrict results to these states; `None` returns all states.
    pub(crate) state_filter: Option<Vec<SubprocessState>>,
    /// Opaque pagination cursor from a previous response.
    pub(crate) page_cursor: Option<String>,
    /// Maximum entries to return (default 50).
    #[serde(default = "default_page_size")]
    pub(crate) page_size: u32,
    /// Caller client identifier for cross-client scoping.
    pub(crate) client_id: Option<String>,
}

impl Default for SubprocessListRequest {
    fn default() -> Self {
        Self {
            state_filter: None,
            page_cursor: None,
            page_size: default_page_size(),
            client_id: None,
        }
    }
}

const fn default_page_size() -> u32 {
    50
}

/// Dispatches `subprocess.list` — list live subprocess handles. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_subprocess_list(
    args: Value,
    port: Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>,
    client_id: substrate_domain::value_objects::ClientId,
) -> SubstrateResult<DispatchedResponse> {
    let req: SubprocessListRequest =
        if args.is_null() || args == Value::Object(serde_json::Map::default()) {
            SubprocessListRequest::default()
        } else {
            serde_json::from_value(args).map_err(|e| SubstrateError::InvalidArgument {
                offending_field: "arguments".to_owned(),
                reason: e.to_string(),
                correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
            })?
        };

    let state_filter_ref: Option<Vec<SubprocessState>> = req.state_filter;
    let state_slice: Option<&[SubprocessState]> = state_filter_ref.as_deref();
    let (handles, next_cursor) = port
        .list(
            &client_id,
            state_slice,
            req.page_cursor.as_deref(),
            req.page_size,
        )
        .await?;

    let content = format!("subprocess.list: {} handle(s).", handles.len());
    let structured = json!({
        "handles": handles.iter().map(|h| json!({
            "job_id": h.job_id,
            "state": h.state,
            "process_group": {
                "pid": h.process_group.pid(),
                "pgid": h.process_group.pgid(),
            },
            "started_at": h.started_at,
            "exit_code": h.exit_code,
        })).collect::<Vec<_>>(),
        "next_cursor": next_cursor,
    });
    let hints = substrate_domain::Hints {
        next_action_suggested: Some("subprocess.result".to_owned()),
        ..substrate_domain::Hints::default()
    };
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- subprocess_cancel ------------------------------------------------------

/// Request type for `subprocess.cancel`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubprocessCancelRequest {
    /// Job identifier of the subprocess to cancel.
    pub(crate) job_id: JobId,
    /// When `true`, skip SIGTERM drain and send SIGKILL immediately.
    #[serde(default)]
    pub(crate) force: bool,
}

/// Dispatches `subprocess.cancel` — cancel a running subprocess. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_subprocess_cancel(
    args: Value,
    port: Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: SubprocessCancelRequest =
        serde_json::from_value(args).map_err(|e| SubstrateError::InvalidArgument {
            offending_field: "arguments".to_owned(),
            reason: e.to_string(),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        })?;

    let state = port.cancel(&req.job_id, req.force).await?;

    let content = format!(
        "subprocess.cancel: job_id={} terminal_state={state:?}.",
        req.job_id
    );
    let structured = json!({
        "job_id": req.job_id,
        "terminal_state": state,
    });
    let hints = substrate_domain::Hints {
        next_action_suggested: Some("subprocess.list".to_owned()),
        ..substrate_domain::Hints::default()
    };
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- subprocess_result ------------------------------------------------------

/// Request type for `subprocess.result`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubprocessResultRequest {
    /// Job identifier of the subprocess.
    pub(crate) job_id: JobId,
    /// Long-poll timeout in milliseconds. Per ADR-0059, absent field substitutes
    /// the configured `jobs.quotas.result_default_wait_ms`; explicit `0` opts
    /// out of long-poll (fast-return).
    #[serde(default)]
    pub(crate) wait_ms: Option<u32>,
    /// Include aggregated stdout/stderr in the response.
    #[serde(default = "default_true")]
    pub(crate) include_aggregates: bool,
    /// Optional line-based pagination for stdout/stderr output (ADR-0057).
    ///
    /// When `Some`, the response includes `stdout_lines`, `stdout_total_lines`,
    /// `stdout_next_offset`, and the stderr equivalents in addition to the raw
    /// base64 aggregates (which remain populated for backward compatibility).
    /// When `None`, the six pagination fields are absent from the response.
    #[serde(default)]
    pub(crate) pagination: Option<Pagination>,
}

const fn default_true() -> bool {
    true
}

/// Dispatches `subprocess.result` — retrieve terminal result and output. See substrate skill.
///
/// `default_wait_ms` is sourced from `jobs.quotas.result_default_wait_ms` per ADR-0059
/// and substituted when the caller omits the `wait_ms` field. An explicit `wait_ms = 0`
/// in the payload is honored unchanged (fast-return).
#[instrument(skip(port, args))]
pub(crate) async fn handle_subprocess_result(
    args: Value,
    port: Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>,
    default_wait_ms: u32,
) -> SubstrateResult<DispatchedResponse> {
    let req: SubprocessResultRequest =
        serde_json::from_value(args).map_err(|e| SubstrateError::InvalidArgument {
            offending_field: "arguments".to_owned(),
            reason: e.to_string(),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        })?;

    let effective_wait_ms = req.wait_ms.unwrap_or(default_wait_ms);
    let result = port
        .result(&req.job_id, effective_wait_ms, req.include_aggregates)
        .await?;

    // Base64-encode binary stdout/stderr aggregates for safe JSON transport.
    let stdout_b64 = base64_encode(&result.stdout_aggregate);
    let stderr_b64 = base64_encode(&result.stderr_aggregate);

    // Build the LLM-facing content text.
    // When TmpFile captures are present, append a one-line note so the model
    // can correlate the structured payload paths with the human-readable summary.
    let tmp_note = match (&result.stdout_tmp_path, &result.stderr_tmp_path) {
        (Some(p), _) => format!(
            " Captures persisted to {}.",
            p.parent()
                .map_or_else(|| p.display().to_string(), |d| d.display().to_string())
        ),
        (None, Some(p)) => format!(
            " Captures persisted to {}.",
            p.parent()
                .map_or_else(|| p.display().to_string(), |d| d.display().to_string())
        ),
        (None, None) => String::new(),
    };
    let content = format!(
        "subprocess.result: job_id={} state={:?} exit_code={:?} stdout={}B stderr={}B.{tmp_note}",
        req.job_id,
        result.terminal_state,
        result.exit_code,
        result.stdout_bytes_total,
        result.stderr_bytes_total,
    );

    // Serialize optional tmp paths as string or null for JSON consumers.
    let stdout_tmp_path_str: Option<String> = result
        .stdout_tmp_path
        .as_ref()
        .map(|p| p.display().to_string());
    let stderr_tmp_path_str: Option<String> = result
        .stderr_tmp_path
        .as_ref()
        .map(|p| p.display().to_string());

    // Apply line-based pagination when the caller supplied a `pagination` cursor
    // (ADR-0057). The raw aggregate bytes are decoded to UTF-8 lossily and split
    // into lines; the paginated slice is returned alongside the existing base64
    // aggregate fields (backward-compatible — old callers that do not supply
    // `pagination` see `null` for all six optional fields).
    let (stdout_lines, stdout_total_lines, stdout_next_offset) =
        if let Some(ref pag) = req.pagination {
            let stdout_text = String::from_utf8_lossy(&result.stdout_aggregate).into_owned();
            let (lines, total, next) =
                substrate_subprocess::registry::paginate_lines(&stdout_text, pag);
            (Some(lines), Some(total), next)
        } else {
            (
                result.stdout_lines,
                result.stdout_total_lines,
                result.stdout_next_offset,
            )
        };

    let (stderr_lines, stderr_total_lines, stderr_next_offset) =
        if let Some(ref pag) = req.pagination {
            let stderr_text = String::from_utf8_lossy(&result.stderr_aggregate).into_owned();
            let (lines, total, next) =
                substrate_subprocess::registry::paginate_lines(&stderr_text, pag);
            (Some(lines), Some(total), next)
        } else {
            (
                result.stderr_lines,
                result.stderr_total_lines,
                result.stderr_next_offset,
            )
        };

    // Derive a pagination-aware next_action hint: suggest subprocess.result with
    // a follow-up offset when more output pages remain.
    let has_more = stdout_next_offset.is_some() || stderr_next_offset.is_some();

    let structured = json!({
        "job_id": req.job_id,
        "terminal_state": result.terminal_state,
        "exit_code": result.exit_code,
        "stdout_aggregate_b64": stdout_b64,
        "stderr_aggregate_b64": stderr_b64,
        "stdout_aggregate_truncated": result.stdout_aggregate_truncated,
        "stderr_aggregate_truncated": result.stderr_aggregate_truncated,
        "stdout_tmp_path": stdout_tmp_path_str,
        "stderr_tmp_path": stderr_tmp_path_str,
        "stream_chunks_dropped": result.stream_chunks_dropped,
        "duration_ms": result.duration_ms,
        "stdout_bytes_total": result.stdout_bytes_total,
        "stderr_bytes_total": result.stderr_bytes_total,
        "terminal_at": result.terminal_at,
        "stdout_lines": stdout_lines,
        "stdout_total_lines": stdout_total_lines,
        "stdout_next_offset": stdout_next_offset,
        "stderr_lines": stderr_lines,
        "stderr_total_lines": stderr_total_lines,
        "stderr_next_offset": stderr_next_offset,
    });
    let hints = substrate_domain::Hints {
        next_action_suggested: if has_more {
            Some("subprocess.result".to_owned())
        } else {
            Some("subprocess.list".to_owned())
        },
        ..substrate_domain::Hints::default()
    };
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- subprocess_search ------------------------------------------------------

/// Dispatches `subprocess.search` — regex search over captured subprocess output.
///
/// Compiles the pattern, scans the stdout/stderr ring buffers, and returns
/// paginated `SearchMatch` results per ADR-0057. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_subprocess_search(
    args: Value,
    port: Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: SubprocessSearchRequest =
        serde_json::from_value(args).map_err(|e| SubstrateError::InvalidArgument {
            offending_field: "arguments".to_owned(),
            reason: e.to_string(),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        })?;

    let result = port.search(req).await.map_err(|e| subprocess_err(&e))?;

    let n = result.matches.len();
    let total = result.total_matches;
    let next = result.next_offset;

    let content = format!("subprocess.search: matches={n} total={total} next_offset={next:?}.",);

    let structured = json!({
        "matches": result.matches.iter().map(|m| json!({
            "stream": m.stream,
            "line_number": m.line_number,
            "line_text": m.line_text,
        })).collect::<Vec<_>>(),
        "total_matches": total,
        "next_offset": next,
    });

    let hints = substrate_domain::Hints {
        next_action_suggested: if next.is_some() {
            Some("subprocess.search".to_owned())
        } else {
            Some("subprocess.result".to_owned())
        },
        ..substrate_domain::Hints::default()
    };

    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- subprocess_signal ------------------------------------------------------

/// Request type for `subprocess.signal`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubprocessSignalRequest {
    /// Job identifier of the subprocess.
    pub(crate) job_id: JobId,
    /// POSIX signal to send.
    pub(crate) signal: SubprocessSignalName,
    /// Whether to target the process or the entire process group.
    #[serde(default = "default_signal_target")]
    pub(crate) target: SignalTarget,
    /// Confirmation token required for destructive signals (SIGKILL, SIGTERM, SIGSTOP).
    #[serde(default)]
    pub(crate) elicitation_confirmed: bool,
}

const fn default_signal_target() -> SignalTarget {
    SignalTarget::ProcessGroup
}

/// Dispatches `subprocess.signal` — send a POSIX signal to a subprocess. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_subprocess_signal(
    args: Value,
    port: Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: SubprocessSignalRequest =
        serde_json::from_value(args).map_err(|e| SubstrateError::InvalidArgument {
            offending_field: "arguments".to_owned(),
            reason: e.to_string(),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        })?;

    // Destructive signals require elicitation confirmation per ADR-0052.
    if matches!(
        req.signal,
        SubprocessSignalName::Sigkill
            | SubprocessSignalName::Sigterm
            | SubprocessSignalName::Sigstop
    ) && !req.elicitation_confirmed
    {
        return Err(SubstrateError::InvalidArgument {
            offending_field: "elicitation_confirmed".to_owned(),
            reason: format!(
                "destructive signal {} requires elicitation_confirmed=true per ADR-0052",
                req.signal
            ),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        });
    }

    port.signal(&req.job_id, req.signal, req.target).await?;

    let content = format!(
        "subprocess.signal: job_id={} signal={} target={:?} delivered.",
        req.job_id, req.signal, req.target,
    );
    let structured = json!({
        "job_id": req.job_id,
        "signal": req.signal,
        "target": req.target,
        "delivered": true,
    });
    let hints = substrate_domain::Hints {
        next_action_suggested: Some("subprocess.result".to_owned()),
        ..substrate_domain::Hints::default()
    };
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- Unit tests -------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use time::OffsetDateTime;

    use substrate_domain::{
        SubstrateError, SubstrateResult,
        ports::subprocess::{
            CancelSignal, SignalTarget, SubprocessPort, SubprocessResult, SubprocessSignalName,
        },
        subprocess::{
            errors::SubprocessError,
            handle::SubprocessHandle,
            pagination::{SubprocessSearchRequest, SubprocessSearchResult},
            request::SubprocessRequest,
            state::SubprocessState,
        },
        value_objects::{ClientId, JobId, ProcessGroup},
    };

    use super::*;

    // ---- Change 3: Default impl unit tests ----------------------------------

    #[test]
    fn subprocess_list_request_default_uses_page_size_fn() {
        let req = SubprocessListRequest::default();
        assert_eq!(
            req.page_size, 50,
            "Default::default() must honor default_page_size()"
        );
        assert!(req.state_filter.is_none());
        assert!(req.page_cursor.is_none());
        assert!(req.client_id.is_none());
    }

    /// Deserialising from `{}` (empty JSON object) must also produce page_size=50.
    ///
    /// This confirms that `#[serde(deny_unknown_fields, default)]` on the struct
    /// falls back to the manual `Default` impl (and therefore `default_page_size()`)
    /// for absent fields, rather than to `0u32::default()`.
    #[test]
    fn subprocess_list_request_serde_empty_object_uses_page_size_fn() {
        let req: SubprocessListRequest =
            serde_json::from_str("{}").expect("deserialise empty object");
        assert_eq!(
            req.page_size, 50,
            "serde default for empty {{}} must honour default_page_size()"
        );
    }

    /// Deserialising from `null` must also produce page_size=50 (handler fast-path).
    #[test]
    fn subprocess_list_request_handler_null_uses_page_size_fn() {
        let args = serde_json::Value::Null;
        let req = if args.is_null() || args == serde_json::Value::Object(serde_json::Map::default())
        {
            SubprocessListRequest::default()
        } else {
            serde_json::from_value(args).expect("should not reach")
        };
        assert_eq!(
            req.page_size, 50,
            "handler null-path must produce page_size=50"
        );
    }

    // ---- Change 4: handler integration tests (stub SubprocessPort) ----------

    /// Minimal `SubprocessPort` stub that returns a fixed one-element list from
    /// `list()`. All other methods are unreachable — they are not exercised here.
    struct OneHandlePort {
        handle: SubprocessHandle,
    }

    impl OneHandlePort {
        fn new() -> Self {
            let job_id = JobId::now_v7();
            let process_group =
                ProcessGroup::new(100, 100).expect("valid ProcessGroup for test stub");
            Self {
                handle: SubprocessHandle {
                    job_id,
                    process_group,
                    state: SubprocessState::Running,
                    started_at: OffsetDateTime::now_utc(),
                    exit_code: None,
                    stream_chunks_dropped: 0,
                    tmp_files: Vec::new(),
                },
            }
        }
    }

    #[async_trait]
    impl SubprocessPort for OneHandlePort {
        async fn spawn(
            &self,
            _req: SubprocessRequest,
            _cancel: &dyn CancelSignal,
        ) -> Result<SubprocessHandle, SubprocessError> {
            unreachable!("spawn not exercised by this test")
        }

        async fn list(
            &self,
            _client_id: &ClientId,
            _state_filter: Option<&[SubprocessState]>,
            _page_cursor: Option<&str>,
            page_size: u32,
        ) -> SubstrateResult<(Vec<SubprocessHandle>, Option<String>)> {
            // Fail loudly in tests if the caller passes page_size=0 so regressions
            // produce a clear, actionable error rather than a silent empty Vec.
            if page_size == 0 {
                return Err(SubstrateError::InternalError {
                    reason: "page_size=0 reached stub port — \
                             SubprocessListRequest Default bug (page_size must be 50)"
                        .to_owned(),
                    correlation_id: None,
                });
            }
            let handles: Vec<SubprocessHandle> = vec![self.handle.clone()]
                .into_iter()
                .take(page_size as usize)
                .collect();
            Ok((handles, None))
        }

        async fn cancel(&self, _job_id: &JobId, _force: bool) -> SubstrateResult<SubprocessState> {
            unreachable!("cancel not exercised by this test")
        }

        async fn result(
            &self,
            _job_id: &JobId,
            _wait_ms: u32,
            _include_aggregates: bool,
        ) -> SubstrateResult<SubprocessResult> {
            unreachable!("result not exercised by this test")
        }

        async fn signal(
            &self,
            _job_id: &JobId,
            _signal_name: SubprocessSignalName,
            _target: SignalTarget,
        ) -> SubstrateResult<()> {
            unreachable!("signal not exercised by this test")
        }

        async fn search(
            &self,
            _req: SubprocessSearchRequest,
        ) -> Result<SubprocessSearchResult, SubprocessError> {
            unreachable!("search not exercised by this test")
        }
    }

    fn test_client_id() -> ClientId {
        ClientId::parse("test-client").expect("valid test ClientId")
    }

    /// `Value::Null` input must produce the one registered handle.
    ///
    /// Regression guard for the `page_size=0` bug: before the fix,
    /// `Default::default()` produced `page_size=0`, causing `iter().take(0)` to
    /// yield an empty Vec even when handles existed.
    #[tokio::test]
    async fn handle_subprocess_list_null_args_returns_existing_handle() {
        let port: Arc<dyn SubprocessPort> = Arc::new(OneHandlePort::new());
        let client_id = test_client_id();

        let resp = handle_subprocess_list(Value::Null, port, client_id)
            .await
            .expect("handle_subprocess_list must not Err for valid stub");

        let handles = resp
            .structured_content
            .get("handles")
            .and_then(|v| v.as_array())
            .expect("structured_content must contain a handles array");

        assert_eq!(
            handles.len(),
            1,
            "Value::Null input must return 1 handle; got {}. \
             (page_size=0 regression: Default::default() must use \
             default_page_size()=50, not u32::default()=0)",
            handles.len()
        );
    }

    /// `Value::Object(Map::default())` (empty JSON object `{}`) must also return
    /// the registered handle. This is the second shape the handler fast-path
    /// recognises as "no arguments provided".
    #[tokio::test]
    async fn handle_subprocess_list_empty_object_args_returns_existing_handle() {
        let port: Arc<dyn SubprocessPort> = Arc::new(OneHandlePort::new());
        let client_id = test_client_id();

        let args = Value::Object(serde_json::Map::default());
        let resp = handle_subprocess_list(args, port, client_id)
            .await
            .expect("handle_subprocess_list must not Err for empty-object args");

        let handles = resp
            .structured_content
            .get("handles")
            .and_then(|v| v.as_array())
            .expect("structured_content must contain a handles array");

        assert_eq!(
            handles.len(),
            1,
            "empty-object input must return 1 handle; got {}. \
             (page_size=0 regression: serde default for missing field must use \
             default_page_size()=50, not u32::default()=0)",
            handles.len()
        );
    }

    /// Explicit `page_size` in the request must be honoured — guards that the fix
    /// did not break the explicit-override path.
    #[tokio::test]
    async fn handle_subprocess_list_explicit_page_size_is_honoured() {
        let port: Arc<dyn SubprocessPort> = Arc::new(OneHandlePort::new());
        let client_id = test_client_id();

        let args = json!({ "page_size": 500 });
        let resp = handle_subprocess_list(args, port, client_id)
            .await
            .expect("handle_subprocess_list must not Err for explicit page_size");

        let handles = resp
            .structured_content
            .get("handles")
            .and_then(|v| v.as_array())
            .expect("structured_content must contain a handles array");

        // Stub holds 1 handle total; page_size=500 is above that so we get 1.
        assert_eq!(
            handles.len(),
            1,
            "explicit page_size=500 must return all 1 available handle; got {}",
            handles.len()
        );
    }
}

// ---- Base64 helper ----------------------------------------------------------

/// Minimal RFC 4648 base64 encoder (standard alphabet, no padding stripping).
///
/// Avoids pulling a runtime dep into the handler layer; output is valid
/// standard base64 readable by any JSON consumer.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let combined = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHA[((combined >> 18) & 0x3F) as usize]);
        out.push(ALPHA[((combined >> 12) & 0x3F) as usize]);
        out.push(if chunk.len() >= 2 {
            ALPHA[((combined >> 6) & 0x3F) as usize]
        } else {
            b'='
        });
        out.push(if chunk.len() == 3 {
            ALPHA[(combined & 0x3F) as usize]
        } else {
            b'='
        });
    }
    // SAFETY: every byte pushed is ASCII from ALPHA or '='.
    String::from_utf8(out).unwrap_or_default()
}
