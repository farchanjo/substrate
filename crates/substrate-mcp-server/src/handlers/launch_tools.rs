//! MCP tool handlers for `launch_*` tools per ADR-0063..0069.
//!
//! All nine handlers are compiled only when the `launch` Cargo feature is active.
//! They delegate to `Arc<dyn LaunchPort>` wired by the composition root (which in
//! turn drives every managed Service through the injected `Arc<dyn SubprocessPort>`)
//! and convert domain results into [`DispatchedResponse`] envelopes.
//!
//! Tool descriptions are thin one-liners per the ADR-0007 2026-05-22 amendment;
//! the full narrative arc lives in the companion `substrate` skill.
//!
//! # Error mapping (CONFLICT FLAG #1 resolution)
//!
//! [`LaunchError`] carries stable `SUBSTRATE_LAUNCH_*` code strings. The base
//! taxonomy (ADR-0010) has no matching variants, so [`launch_err`] maps each
//! launch error to the closest base [`SubstrateError`] and preserves the
//! authoritative launch code plus its numeric JSON-RPC code (`-32044..-32056`)
//! verbatim in the surfaced reason via `[<code>] (rpc <n>)`. This mirrors the
//! `subprocess_err` strategy so the stable identifier is never lost at the edge.

#![cfg(feature = "launch")]
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
    launch::errors::LaunchError,
    launch::state::DisconnectPolicy,
    ports::launch::{CancelSignal, LaunchPort},
    value_objects::stack_id::StackId,
};

use crate::handlers::dispatcher::DispatchedResponse;

// ---- CancelSignal shim ------------------------------------------------------

/// A trivial always-not-cancelled [`CancelSignal`] used at the MCP handler layer.
///
/// The launch registry drives cooperative cancellation through the subprocess
/// port's own tokens; this shim is adequate for the composition-root edge.
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

/// Returns the numeric JSON-RPC code for a [`LaunchError`] per ADR-0063..0068.
///
/// The launch BC reserves `-32044..-32056`; `InvalidProfile` reuses the base
/// invalid-argument code (`-32009`) and `SpawnFailed` the internal code
/// (`-32099`).
const fn launch_rpc_code(e: &LaunchError) -> i32 {
    match e {
        LaunchError::ProfileNotTrusted { .. } => -32044,
        LaunchError::ConfigSymlinkRejected { .. } => -32045,
        LaunchError::ConfigUntrustedDir { .. } => -32046,
        LaunchError::TrustStoreInsecure { .. } => -32047,
        LaunchError::CycleDetected { .. } => -32048,
        LaunchError::DependencyFailed { .. } => -32049,
        LaunchError::OrphanReaped { .. } => -32050,
        LaunchError::OrphanAdopted { .. } => -32051,
        LaunchError::StackTtlExpired { .. } => -32052,
        LaunchError::SupervisorUnreachable { .. } => -32053,
        LaunchError::RegistryInsecure { .. } => -32054,
        LaunchError::FrameTooLarge { .. } => -32055,
        LaunchError::ChildPidRecycled { .. } => -32056,
        LaunchError::InvalidProfile { .. } => -32009,
        LaunchError::SpawnFailed { .. } => -32099,
    }
}

/// Maps a [`LaunchError`] to the most specific base [`SubstrateError`] so the
/// wire envelope carries a sensible stable code while the authoritative launch
/// code string and numeric JSON-RPC code are preserved verbatim in the reason.
///
/// - Trust / security gates (`ProfileNotTrusted`, `ConfigSymlinkRejected`,
///   `ConfigUntrustedDir`, `TrustStoreInsecure`, `RegistryInsecure`) →
///   `PermissionDenied`.
/// - Structural / dependency faults (`CycleDetected`, `DependencyFailed`,
///   `InvalidProfile`) → `InvalidArgument`.
/// - Supervisor / lifecycle faults (`SupervisorUnreachable`, `OrphanReaped`,
///   `OrphanAdopted`, `StackTtlExpired`, `FrameTooLarge`, `ChildPidRecycled`,
///   `SpawnFailed`) → `InternalError`.
fn launch_err(e: &LaunchError) -> SubstrateError {
    let correlation_id = Some(new_correlation_id());
    let reason = format!("[{}] (rpc {}) {e}", e.code(), launch_rpc_code(e));
    match e {
        LaunchError::ProfileNotTrusted { .. }
        | LaunchError::ConfigSymlinkRejected { .. }
        | LaunchError::ConfigUntrustedDir { .. }
        | LaunchError::TrustStoreInsecure { .. }
        | LaunchError::RegistryInsecure { .. } => SubstrateError::PermissionDenied {
            path: reason,
            correlation_id,
        },
        LaunchError::InvalidProfile { msg } => SubstrateError::InvalidArgument {
            offending_field: "profile".to_owned(),
            reason: format!("[{}] (rpc {}) {msg}", e.code(), launch_rpc_code(e)),
            correlation_id,
        },
        LaunchError::CycleDetected { .. } | LaunchError::DependencyFailed { .. } => {
            SubstrateError::InvalidArgument {
                offending_field: "depends_on".to_owned(),
                reason,
                correlation_id,
            }
        },
        LaunchError::SupervisorUnreachable { .. }
        | LaunchError::OrphanReaped { .. }
        | LaunchError::OrphanAdopted { .. }
        | LaunchError::StackTtlExpired { .. }
        | LaunchError::FrameTooLarge { .. }
        | LaunchError::ChildPidRecycled { .. }
        | LaunchError::SpawnFailed { .. } => SubstrateError::InternalError {
            reason,
            correlation_id,
        },
    }
}

/// Deserializes `args` into `T`, mapping serde failures to `SUBSTRATE_INVALID_ARGUMENT`.
fn parse_args<T: for<'de> Deserialize<'de>>(args: Value) -> SubstrateResult<T> {
    serde_json::from_value(args).map_err(|e| SubstrateError::InvalidArgument {
        offending_field: "arguments".to_owned(),
        reason: e.to_string(),
        correlation_id: Some(new_correlation_id()),
    })
}

/// Builds the destructive-confirmation hint shared by the mutating launch tools.
fn destructive_hints(next_action: &str, stack_id: Option<&str>) -> substrate_domain::Hints {
    substrate_domain::Hints {
        next_action_suggested: Some(next_action.to_owned()),
        confirm_destructive: Some(true),
        job_id: stack_id.map(ToOwned::to_owned),
        ..substrate_domain::Hints::default()
    }
}

// ---- launch_init ------------------------------------------------------------

/// Request type for `launch_init`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchInitRequest {
    /// Optional target path for the scaffolded `.substrate.toml`.
    #[serde(default)]
    profile_path: Option<String>,
    /// Optional project-type hint (e.g. `"rust"`, `"node"`) biasing the template.
    #[serde(default)]
    project_type_hint: Option<String>,
}

/// Dispatches `launch_init` — scaffold a `.substrate.toml` Profile. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_launch_init(
    args: Value,
    port: Arc<dyn LaunchPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: LaunchInitRequest = parse_args(args)?;
    let written = port
        .init(req.profile_path.as_deref(), req.project_type_hint.as_deref())
        .await
        .map_err(|e| launch_err(&e))?;

    let content = format!("launch.init: scaffolded profile at {written}.");
    let structured = json!({ "profile_path": written });
    let hints = substrate_domain::Hints {
        next_action_suggested: Some("launch_list".to_owned()),
        confirm_destructive: Some(true),
        ..substrate_domain::Hints::default()
    };
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- launch_list ------------------------------------------------------------

/// Request type for `launch_list`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchListRequest {
    /// Path to the `.substrate.toml` Profile to read.
    profile_path: String,
}

/// Dispatches `launch_list` — read the Service catalog without a trust verdict. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_launch_list(
    args: Value,
    port: Arc<dyn LaunchPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: LaunchListRequest = parse_args(args)?;
    let catalog = port
        .list(&req.profile_path)
        .await
        .map_err(|e| launch_err(&e))?;

    let content = format!("launch.list: {} service(s).", catalog.len());
    let structured = json!({
        "services": serde_json::to_value(&catalog).unwrap_or(Value::Null),
    });
    let hints = substrate_domain::Hints {
        next_action_suggested: Some("launch_trust".to_owned()),
        ..substrate_domain::Hints::default()
    };
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- launch_trust -----------------------------------------------------------

/// Request type for `launch_trust`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchTrustRequest {
    /// Path to the `.substrate.toml` Profile to bless.
    profile_path: String,
}

/// Dispatches `launch_trust` — bless a Profile into the TOFU trust store. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_launch_trust(
    args: Value,
    port: Arc<dyn LaunchPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: LaunchTrustRequest = parse_args(args)?;
    let record = port
        .trust(&req.profile_path)
        .await
        .map_err(|e| launch_err(&e))?;

    let content = format!("launch.trust: blessed {} ({}).", record.path, record.content);
    let structured = serde_json::to_value(&record).unwrap_or(Value::Null);
    let hints = destructive_hints("launch_up", None);
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- launch_up --------------------------------------------------------------

/// Request type for `launch_up`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchUpRequest {
    /// Path to the trusted `.substrate.toml` Profile to bring up.
    profile_path: String,
    /// Disconnect policy override (`shutdown` | `detach`); defaults to the Profile.
    #[serde(default)]
    on_client_disconnect: Option<DisconnectPolicy>,
    /// Orphan TTL override in seconds (detach only).
    #[serde(default)]
    orphan_ttl_secs: Option<u32>,
}

/// Dispatches `launch_up` — bring up a Stack in readiness-gated topological order. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_launch_up(
    args: Value,
    port: Arc<dyn LaunchPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: LaunchUpRequest = parse_args(args)?;
    let handle = port
        .up(
            &req.profile_path,
            req.on_client_disconnect,
            req.orphan_ttl_secs,
            &NoCancel,
        )
        .await
        .map_err(|e| launch_err(&e))?;

    let stack_id = handle.stack_id.to_crockford();
    let content = format!(
        "launch.up: stack {} state {} ({} service(s)).",
        stack_id,
        handle.state,
        handle.services.len(),
    );
    let mut structured = serde_json::to_value(&handle).unwrap_or(Value::Null);
    if let Value::Object(ref mut map) = structured {
        map.insert("stack_id".to_owned(), Value::String(stack_id.clone()));
        map.insert(
            "stack_state".to_owned(),
            Value::String(handle.state.to_string()),
        );
    }
    let mut hints = destructive_hints("launch_status", Some(&stack_id));
    // `polling_endpoint` uses the logical (dot) tool name per ADR-0069 §"Links"
    // (ADR-0062 amendment): `next_action_suggested` / `alternative_tool` use
    // the wire name (underscore), `polling_endpoint` uses the logical name.
    hints.polling_endpoint = Some("launch.status".to_owned());
    hints.job_state = Some(handle.state.to_string());
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- launch_status ----------------------------------------------------------

/// Request type for `launch_status`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct LaunchStatusRequest {
    /// Optional Stack id; `None` returns every known Stack.
    stack_id: Option<String>,
}

/// Dispatches `launch_status` — snapshot Stack handles. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_launch_status(
    args: Value,
    port: Arc<dyn LaunchPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: LaunchStatusRequest =
        if args.is_null() || args == Value::Object(serde_json::Map::default()) {
            LaunchStatusRequest::default()
        } else {
            parse_args(args)?
        };
    let parsed = match req.stack_id {
        Some(ref s) => Some(StackId::parse_crockford(s)?),
        None => None,
    };
    let handles = port.status(parsed.as_ref()).await?;

    let content = format!("launch.status: {} stack(s).", handles.len());
    let structured = json!({
        "stacks": serde_json::to_value(&handles).unwrap_or(Value::Null),
    });
    let hints = substrate_domain::Hints {
        next_action_suggested: Some("launch_logs".to_owned()),
        ..substrate_domain::Hints::default()
    };
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- launch_logs ------------------------------------------------------------

/// Request type for `launch_logs`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchLogsRequest {
    /// Stack id whose event log is read.
    stack_id: String,
    /// Restrict the events to a single Service.
    #[serde(default)]
    service: Option<String>,
    /// Opaque cursor marking the last position already read.
    #[serde(default)]
    since: Option<String>,
}

/// Dispatches `launch_logs` — read a Stack's cursor-addressed event tail. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_launch_logs(
    args: Value,
    port: Arc<dyn LaunchPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: LaunchLogsRequest = parse_args(args)?;
    let stack_id = StackId::parse_crockford(&req.stack_id)?;
    let (events, next_cursor) = port
        .logs(&stack_id, req.service.as_deref(), req.since.as_deref())
        .await?;

    let content = format!("launch.logs: {} event(s).", events.len());
    let structured = json!({
        "events": serde_json::to_value(&events).unwrap_or(Value::Null),
        "next_cursor": next_cursor,
    });
    let hints = substrate_domain::Hints {
        next_action_suggested: Some("launch_status".to_owned()),
        ..substrate_domain::Hints::default()
    };
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- launch_restart ---------------------------------------------------------

/// Request type for `launch_restart`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchRestartRequest {
    /// Stack id owning the Service to restart.
    stack_id: String,
    /// The Service alias to restart.
    service: String,
}

/// Dispatches `launch_restart` — orchestrated single-Service restart. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_launch_restart(
    args: Value,
    port: Arc<dyn LaunchPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: LaunchRestartRequest = parse_args(args)?;
    let stack_id = StackId::parse_crockford(&req.stack_id)?;
    let handle = port
        .restart(&stack_id, &req.service, &NoCancel)
        .await
        .map_err(|e| launch_err(&e))?;

    let id = handle.stack_id.to_crockford();
    let content = format!(
        "launch.restart: service '{}' in stack {} (state {}).",
        req.service, id, handle.state,
    );
    let mut structured = serde_json::to_value(&handle).unwrap_or(Value::Null);
    if let Value::Object(ref mut map) = structured {
        map.insert("stack_id".to_owned(), Value::String(id.clone()));
        map.insert(
            "stack_state".to_owned(),
            Value::String(handle.state.to_string()),
        );
    }
    let mut hints = destructive_hints("launch_status", Some(&id));
    hints.job_state = Some(handle.state.to_string());
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- launch_reload ----------------------------------------------------------

/// Request type for `launch_reload`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchReloadRequest {
    /// Stack id to reconcile against a new Profile.
    stack_id: String,
    /// Optional path to the new Profile; `None` re-reads the pinned path.
    #[serde(default)]
    profile_path: Option<String>,
}

/// Dispatches `launch_reload` — reconcile a running Stack against a new Profile. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_launch_reload(
    args: Value,
    port: Arc<dyn LaunchPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: LaunchReloadRequest = parse_args(args)?;
    let stack_id = StackId::parse_crockford(&req.stack_id)?;
    let report = port
        .reload(&stack_id, req.profile_path.as_deref(), &NoCancel)
        .await
        .map_err(|e| launch_err(&e))?;

    let content = format!(
        "launch.reload: +{} -{} ~{} (edge-only {}).",
        report.added.len(),
        report.removed.len(),
        report.restarted.len(),
        report.edge_only.len(),
    );
    let mut structured = serde_json::to_value(&report).unwrap_or(Value::Null);
    if let Value::Object(ref mut map) = structured {
        map.insert(
            "stack_id".to_owned(),
            Value::String(req.stack_id.clone()),
        );
    }
    let hints = destructive_hints("launch_status", Some(&req.stack_id));
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- launch_down ------------------------------------------------------------

/// Request type for `launch_down`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchDownRequest {
    /// Stack id to tear down in reverse topological order.
    stack_id: String,
}

/// Dispatches `launch_down` — cascade-stop a Stack and return its final state. See substrate skill.
#[instrument(skip(port, args))]
pub(crate) async fn handle_launch_down(
    args: Value,
    port: Arc<dyn LaunchPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: LaunchDownRequest = parse_args(args)?;
    let stack_id = StackId::parse_crockford(&req.stack_id)?;
    let state = port
        .down(&stack_id, &NoCancel)
        .await
        .map_err(|e| launch_err(&e))?;

    let content = format!("launch.down: stack {} final state {}.", req.stack_id, state);
    let structured = json!({
        "stack_id": req.stack_id,
        "stack_state": state.to_string(),
    });
    let mut hints = destructive_hints("launch_status", Some(&req.stack_id));
    hints.job_state = Some(state.to_string());
    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
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
    use super::*;

    /// Every launch error must map to a numeric JSON-RPC code in the reserved
    /// `-32044..=-32056` band (plus the two base reuses).
    #[test]
    fn rpc_codes_cover_reserved_band() {
        let cases = [
            (LaunchError::ProfileNotTrusted { path: "p".to_owned() }, -32044),
            (
                LaunchError::ConfigSymlinkRejected { path: "p".to_owned() },
                -32045,
            ),
            (LaunchError::ConfigUntrustedDir { path: "p".to_owned() }, -32046),
            (LaunchError::TrustStoreInsecure { path: "p".to_owned() }, -32047),
            (LaunchError::CycleDetected { nodes: vec![] }, -32048),
            (
                LaunchError::DependencyFailed {
                    service: "s".to_owned(),
                    dependency: "d".to_owned(),
                },
                -32049,
            ),
            (LaunchError::OrphanReaped { name: "n".to_owned() }, -32050),
            (LaunchError::OrphanAdopted { name: "n".to_owned() }, -32051),
            (LaunchError::StackTtlExpired { stack_id: "s".to_owned() }, -32052),
            (
                LaunchError::SupervisorUnreachable { stack_id: "s".to_owned() },
                -32053,
            ),
            (LaunchError::RegistryInsecure { path: "p".to_owned() }, -32054),
            (LaunchError::FrameTooLarge { size: 1 }, -32055),
            (
                LaunchError::ChildPidRecycled { name: "n".to_owned(), pid: 1 },
                -32056,
            ),
            (LaunchError::InvalidProfile { msg: "m".to_owned() }, -32009),
        ];
        for (err, expected) in cases {
            assert_eq!(launch_rpc_code(&err), expected, "{}", err.code());
        }
    }

    /// `launch_err` must preserve the authoritative launch code string verbatim
    /// in the surfaced reason rather than collapsing it.
    #[test]
    fn launch_err_preserves_code_in_reason() {
        let err = LaunchError::ProfileNotTrusted { path: "/p".to_owned() };
        let mapped = launch_err(&err);
        assert_eq!(mapped.code(), "SUBSTRATE_PERMISSION_DENIED");
        assert!(
            mapped.to_string().contains("SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED"),
            "mapped reason must carry the launch code: {mapped}"
        );
    }

    /// `InvalidProfile` maps to the base invalid-argument variant.
    #[test]
    fn invalid_profile_maps_to_invalid_argument() {
        let err = LaunchError::InvalidProfile { msg: "bad".to_owned() };
        let mapped = launch_err(&err);
        assert_eq!(mapped.code(), "SUBSTRATE_INVALID_ARGUMENT");
    }
}
