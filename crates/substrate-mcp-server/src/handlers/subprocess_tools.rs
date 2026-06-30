//! MCP tool handlers for subprocess.* tools per ADR-0052.
//!
//! All five handlers are compiled only when the `subprocess` Cargo feature is
//! active. They delegate to `Arc<dyn SubprocessPort>` wired by the composition
//! root and convert domain results to `DispatchedResponse` envelopes.
//!
//! Tool descriptions are thin one-liners per the ADR-0007 2026-05-22 amendment;
//! full narrative arc lives in the companion `subprocess.md` tool-card document.
//!
//! # Request Default Contract (ADR-0061)
//!
//! Every request struct in this module that participates in the
//! `is_null() || empty_object` shortcut MUST implement `Default` manually
//! (not via `#[derive(Default)]`). The manual impl MUST match every
//! `#[serde(default = "fn")]` field override.
//!
//! Currently: [`SubprocessListRequest`] uses this shortcut.
//! Enforced by: `docs/arch/policies/request_default_invariants.rego`

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
    value_objects::pagination::PageSize,
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

/// Generates a fresh `UUIDv7` correlation id for an outbound error envelope.
fn new_correlation_id() -> uuid::Uuid {
    uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))
}

/// Maps an ADR-0052 [`SubprocessError`] to the most specific [`SubstrateError`]
/// variant so the wire envelope carries the correct stable code instead of
/// collapsing everything to `SUBSTRATE_INTERNAL_ERROR`.
///
/// Where the base taxonomy (ADR-0010) exposes a matching code the variant maps
/// directly — `CwdOutsideAllowlist` → `PathOutsideAllowlist`, `QuotaExceeded` →
/// `QuotaExceeded`, `Timeout` → `Timeout`, `InvalidRequest` → `InvalidArgument`,
/// `ElicitationRequired` → `ConfirmationRequired`, `Killed` → `Cancelled`. For
/// subprocess-only codes that have no base-taxonomy variant
/// (`SUBSTRATE_SUBPROCESS_BINARY_NOT_ALLOWED`, `..._ENV_BANNED`, `..._SPAWN_FAILED`,
/// `SUBSTRATE_STREAM_CHUNK_DROPPED`, `SUBSTRATE_INVALID_STATE_TRANSITION`) the
/// closest base variant is used and the authoritative subprocess code string is
/// preserved verbatim in the reason via `[<code>]` so it is never lost.
fn subprocess_err(e: &SubprocessError) -> SubstrateError {
    let correlation_id = Some(new_correlation_id());
    let reason = format!("[{}] {e}", e.code());
    match e {
        SubprocessError::CwdOutsideAllowlist { path } => SubstrateError::PathOutsideAllowlist {
            path: path.clone(),
            correlation_id,
        },
        SubprocessError::QuotaExceeded { .. } => SubstrateError::QuotaExceeded {
            detail: reason,
            correlation_id,
        },
        SubprocessError::Timeout { secs } => SubstrateError::Timeout {
            elapsed_ms: u64::from(*secs).saturating_mul(1_000),
            correlation_id,
        },
        SubprocessError::Killed => SubstrateError::Cancelled { correlation_id },
        SubprocessError::ElicitationRequired { .. } => {
            SubstrateError::ConfirmationRequired { correlation_id }
        },
        SubprocessError::InvalidRequest { msg } => SubstrateError::InvalidArgument {
            offending_field: "arguments".to_owned(),
            reason: msg.clone(),
            correlation_id,
        },
        SubprocessError::BinaryNotAllowed { .. } => SubstrateError::InvalidArgument {
            offending_field: "binary_path".to_owned(),
            reason,
            correlation_id,
        },
        SubprocessError::EnvBanned { .. } => SubstrateError::InvalidArgument {
            offending_field: "env_override".to_owned(),
            reason,
            correlation_id,
        },
        SubprocessError::SpawnFailed { .. }
        | SubprocessError::StreamChunkDropped { .. }
        | SubprocessError::InvalidStateTransition { .. } => SubstrateError::InternalError {
            reason,
            correlation_id,
        },
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
///
/// `page_size` is `Option<u32>` on the wire so the handler can distinguish
/// "caller omitted the field" (apply `PageSize::default()`) from "caller sent 0"
/// (return `SUBSTRATE_INVALID_ARGUMENT`). The port boundary always receives a
/// validated [`PageSize`] per ADR-0060.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct SubprocessListRequest {
    /// Restrict results to these states; `None` returns all states.
    pub(crate) state_filter: Option<Vec<SubprocessState>>,
    /// Opaque pagination cursor from a previous response.
    pub(crate) page_cursor: Option<String>,
    /// Maximum entries to return (1..=10 000, default 50 per ADR-0060).
    ///
    /// `None` when the field is absent from the JSON — the handler substitutes
    /// `PageSize::default()`. An explicit `0` or value above 10 000 returns
    /// `SUBSTRATE_INVALID_ARGUMENT`.
    pub(crate) page_size: Option<u32>,
    /// Caller client identifier for cross-client scoping.
    pub(crate) client_id: Option<String>,
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

    // ADR-0060: convert Option<u32> → PageSize at the inbound boundary.
    // Absent field → PageSize::default() (50). Explicit 0 or > 10 000 → INVALID_ARGUMENT.
    let page_size = match req.page_size {
        Some(n) => PageSize::try_from(n)?,
        None => PageSize::default(),
    };

    let state_filter_ref: Option<Vec<SubprocessState>> = req.state_filter;
    let state_slice: Option<&[SubprocessState]> = state_filter_ref.as_deref();
    let (handles, next_cursor) = port
        .list(
            &client_id,
            state_slice,
            req.page_cursor.as_deref(),
            page_size,
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

/// Applies line pagination (ADR-0057) to a captured-output aggregate when the
/// caller supplied a `pagination` cursor, or returns the registry-provided
/// pre-paginated fallback fields otherwise.
///
/// Shared by the stdout and stderr pagination paths in
/// [`handle_subprocess_result`], which are otherwise identical apart from
/// which aggregate/fallback fields they read.
fn paginate_or_fallback(
    pagination: Option<&substrate_domain::subprocess::pagination::Pagination>,
    aggregate: &[u8],
    fallback_lines: Option<Vec<String>>,
    fallback_total_lines: Option<u64>,
    fallback_next_offset: Option<u64>,
) -> (Option<Vec<String>>, Option<u64>, Option<u64>) {
    let Some(pag) = pagination else {
        return (fallback_lines, fallback_total_lines, fallback_next_offset);
    };
    let text = String::from_utf8_lossy(aggregate).into_owned();
    let (lines, total, next) = substrate_subprocess::registry::paginate_lines(&text, pag);
    (Some(lines), Some(total), next)
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

    // Line pagination reads the stdout/stderr aggregates; requesting pagination
    // with `include_aggregates=false` would silently paginate over an empty
    // aggregate and return zero lines. Reject the contradictory combination
    // up-front so the caller gets a clear SUBSTRATE_INVALID_ARGUMENT instead.
    if req.pagination.is_some() && !req.include_aggregates {
        return Err(SubstrateError::InvalidArgument {
            offending_field: "include_aggregates".to_owned(),
            reason: "pagination requires include_aggregates=true; \
                     line pagination operates over the stdout/stderr aggregates"
                .to_owned(),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        });
    }

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
    let tmp_note = result
        .stdout_tmp_path
        .as_ref()
        .or(result.stderr_tmp_path.as_ref())
        .map_or_else(String::new, |p| {
            format!(
                " Captures persisted to {}.",
                p.parent()
                    .map_or_else(|| p.display().to_string(), |d| d.display().to_string())
            )
        });
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
    let (stdout_lines, stdout_total_lines, stdout_next_offset) = paginate_or_fallback(
        req.pagination.as_ref(),
        &result.stdout_aggregate,
        result.stdout_lines,
        result.stdout_total_lines,
        result.stdout_next_offset,
    );

    let (stderr_lines, stderr_total_lines, stderr_next_offset) = paginate_or_fallback(
        req.pagination.as_ref(),
        &result.stderr_aggregate,
        result.stderr_lines,
        result.stderr_total_lines,
        result.stderr_next_offset,
    );

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

    let content = format!("subprocess.search: matches={n} total={total} next_offset={next:?}.");

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

// ---- Unit tests -------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use std::sync::Arc;

    use time::OffsetDateTime;

    use substrate_domain::{
        SubstrateResult,
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
        value_objects::{ClientId, JobId, ProcessGroup, pagination::PageSize},
    };

    use super::*;

    // ---- ADR-0060 / ADR-0061: Default impl unit tests -----------------------

    /// `SubprocessListRequest::default()` must have `page_size: None` on the wire
    /// struct (the handler converts None → `PageSize::default()` = 50).
    #[test]
    fn subprocess_list_request_default_page_size_is_none() {
        let req = SubprocessListRequest::default();
        assert!(
            req.page_size.is_none(),
            "Default::default() must produce page_size=None so the handler applies PageSize::default()"
        );
        assert!(req.state_filter.is_none());
        assert!(req.page_cursor.is_none());
        assert!(req.client_id.is_none());
    }

    /// Deserialising from `{}` (empty JSON object) must produce `page_size: None`
    /// so the handler path applies `PageSize::default()` = 50.
    #[test]
    fn subprocess_list_request_serde_empty_object_page_size_is_none() {
        let req: SubprocessListRequest =
            serde_json::from_str("{}").expect("deserialise empty object");
        assert!(
            req.page_size.is_none(),
            "serde default for empty {{}} must produce page_size=None"
        );
    }

    /// Deserialising from `null` must produce `page_size: None` (handler fast-path).
    #[test]
    fn subprocess_list_request_handler_null_page_size_is_none() {
        let args = serde_json::Value::Null;
        let req = if args.is_null() || args == serde_json::Value::Object(serde_json::Map::default())
        {
            SubprocessListRequest::default()
        } else {
            serde_json::from_value(args).expect("should not reach")
        };
        assert!(
            req.page_size.is_none(),
            "handler null-path must produce page_size=None"
        );
    }

    /// `PageSize` conversion: None → default 50, Some(valid) → Ok, Some(0) → Err.
    #[test]
    fn page_size_conversion_from_option() {
        let default_ps =
            None::<u32>.map_or_else(PageSize::default, |n| PageSize::try_from(n).expect("valid"));
        assert_eq!(
            default_ps.get(),
            50,
            "None maps to PageSize::default() = 50"
        );

        let explicit_ps = Some(200_u32)
            .map_or_else(PageSize::default, |n| PageSize::try_from(n).expect("valid"));
        assert_eq!(explicit_ps.get(), 200);

        let zero_result: Result<PageSize, _> =
            Some(0_u32).map_or_else(|| Ok(PageSize::default()), PageSize::try_from);
        assert!(zero_result.is_err(), "page_size=0 must be rejected");
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
            page_size: PageSize,
        ) -> SubstrateResult<(Vec<SubprocessHandle>, Option<String>)> {
            // PageSize guarantees > 0 at the type level — no runtime check needed.
            let handles: Vec<SubprocessHandle> = vec![self.handle.clone()]
                .into_iter()
                .take(page_size.get() as usize)
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
    /// ADR-0060: absent `page_size` → `PageSize::default()` = 50 → port receives
    /// a valid `PageSize`, never zero.
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
            "Value::Null input must return 1 handle; got {}",
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
            "empty-object input must return 1 handle; got {}",
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

    /// ADR-0060: explicit `page_size=0` must return `SUBSTRATE_INVALID_ARGUMENT`.
    #[tokio::test]
    async fn handle_subprocess_list_page_size_zero_returns_invalid_argument() {
        let port: Arc<dyn SubprocessPort> = Arc::new(OneHandlePort::new());
        let client_id = test_client_id();

        let args = json!({ "page_size": 0 });
        let err = handle_subprocess_list(args, port, client_id)
            .await
            .expect_err("page_size=0 must return Err(InvalidArgument)");

        assert_eq!(
            err.code(),
            "SUBSTRATE_INVALID_ARGUMENT",
            "page_size=0 must produce SUBSTRATE_INVALID_ARGUMENT; got {:?}",
            err.code()
        );
    }

    /// ADR-0060: absent `page_size` field defaults to 50 and succeeds.
    #[tokio::test]
    async fn handle_subprocess_list_absent_page_size_defaults_to_fifty() {
        let port: Arc<dyn SubprocessPort> = Arc::new(OneHandlePort::new());
        let client_id = test_client_id();

        // No page_size field in the JSON.
        let args = json!({ "state_filter": null });
        let resp = handle_subprocess_list(args, port, client_id)
            .await
            .expect("absent page_size must default to 50 and succeed");

        let handles = resp
            .structured_content
            .get("handles")
            .and_then(|v| v.as_array())
            .expect("structured_content must contain a handles array");

        assert_eq!(
            handles.len(),
            1,
            "absent page_size → default 50 → 1 handle returned"
        );
    }
}
