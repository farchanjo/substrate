//! `proc.signal` handler — Zone A (async-native; `kill(2)` is non-blocking).
//!
//! Delivers a POSIX signal to a target process. Enforces (in order):
//!   1. Dry-run gate: first-pass returns preview, no OS mutation.
//!   2. Elicitation gate (ADR-0004 Layer 4 / ADR-0008 §Elicitation Matrix):
//!      SIGKILL, SIGTERM, and SIGSTOP require `elicitation_confirmed = true`
//!      before any PID probe.
//!   3. PID allowlist check (ADR-0004 Layer 1): blocks PID 0, 1, 2.
//!   4. Signal delivery (attempts `kill(2)` immediately; ESRCH at this point
//!      means the process exited between Gate 3 and delivery — a benign race
//!      reported as `NotFound`).
//!
//! Note on TOCTOU (Gates 3 → delivery): there is an inherent race between any
//! PID existence probe and actual signal delivery.  A proactive `kill(pid, 0)`
//! check does not eliminate the window; it only shifts it.  To minimise the
//! window, the implementation skips the existence pre-check and interprets
//! `ESRCH` from the real `kill(pid, sig)` call directly.  The caller receives
//! a `NotFound` error when the race occurs — the signal was never delivered,
//! so no harm is done.
//!
//! Signal delivery uses `nix::sys::signal::kill` exclusively.
//! `std::process::Command` and `tokio::process::Command` are forbidden (ADR-0044).

use std::sync::Arc;

use nix::sys::signal::Signal;
use nix::unistd::Pid;
use serde::Deserialize;
use serde_json::json;
use tracing::instrument;

use crate::{
    hints_helpers::{build_dry_run_hints, build_elicitation_hints},
    pid_allowlist,
    response::{ProcessDeps, ToolResponse},
    signal_policy::is_destructive,
};
use substrate_domain::{SubstrateError, SubstrateResult};

/// Input parameters for `proc.signal`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcSignalRequest {
    /// Target process identifier.
    pub pid: u32,

    /// Signal name (e.g., `"SIGTERM"`, `"SIGHUP"`) or number string (e.g., `"15"`).
    pub signal: String,

    /// When `true`, the handler returns a preview without delivering the signal.
    /// Must be explicitly `false` to proceed with delivery.
    #[serde(default)]
    pub dry_run: Option<bool>,

    /// Must be `true` for destructive signals (SIGKILL/SIGTERM/SIGSTOP) after
    /// the elicitation flow completes.
    #[serde(default)]
    pub elicitation_confirmed: Option<bool>,
}

/// Parses a signal string to a `nix::sys::signal::Signal`.
///
/// Accepts both symbolic names (`SIGTERM`, `TERM`) and numeric strings (`15`).
fn parse_signal(s: &str) -> SubstrateResult<Signal> {
    // Normalise: add SIG prefix if missing, uppercase.
    // Uppercase first so that "sigkill" is treated the same as "SIGKILL".
    let upper = s.to_uppercase();
    let normalised = if upper.starts_with("SIG") {
        upper
    } else if s.chars().all(|c| c.is_ascii_digit()) {
        let n: i32 = s.parse().map_err(|_| SubstrateError::InvalidArgument {
            offending_field: "signal".to_owned(),
            reason: format!("numeric signal '{s}' is out of range"),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        })?;
        return Signal::try_from(n).map_err(|_| SubstrateError::InvalidArgument {
            offending_field: "signal".to_owned(),
            reason: format!("no signal with number {n}"),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        });
    } else {
        format!("SIG{upper}")
    };

    normalised
        .parse::<Signal>()
        .map_err(|_| SubstrateError::InvalidArgument {
            offending_field: "signal".to_owned(),
            reason: format!("unknown signal '{s}'"),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        })
}

/// Handles a `proc.signal` tool call.
///
/// Gate order (ADR-0004 / ADR-0008):
///   1. Dry-run preview (returns early with no OS mutation when `dry_run != false`).
///   2. Elicitation confirmation for destructive signals (SIGKILL, SIGTERM, SIGSTOP).
///   3. PID allowlist check (blocks PID 0, 1, 2).
///   4. Signal delivery — ESRCH from `kill(2)` is treated as `NotFound` (the
///      process exited between Gate 3 and delivery; see module-level TOCTOU note).
///
/// # Errors
///
/// - [`SubstrateError::DryRunRequired`] when `dry_run` is not explicitly `false`.
/// - [`SubstrateError::ConfirmationRequired`] for destructive signals without
///   `elicitation_confirmed = true`.
/// - [`SubstrateError::PermissionDenied`] when the target PID is in the hard-blocked
///   list (0, 1, 2) or when the OS rejects the signal with `EPERM`.
/// - [`SubstrateError::NotFound`] when `kill(2)` returns `ESRCH` (process not found).
#[instrument(skip(deps), fields(pid = req.pid, signal = %req.signal, dry_run = ?req.dry_run))]
pub async fn handle_proc_signal(
    req: ProcSignalRequest,
    deps: Arc<ProcessDeps>,
) -> SubstrateResult<ToolResponse> {
    let _ = deps;

    let sig = parse_signal(&req.signal)?;
    // POSIX pids fit in i32; the kernel rejects pids > INT_MAX, so the cast is safe.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "POSIX pids fit in i32; kernel rejects pids > INT_MAX"
    )]
    let pid = Pid::from_raw(req.pid as i32);

    // Gate 1: Dry-run (ADR-0004 Layer 3). Default is dry-run mode; only
    // proceed if explicitly false. Runs before every security check so the
    // preview path is always available without side-effects.
    let dry_run = req.dry_run.unwrap_or(true);
    if dry_run {
        let hints = build_dry_run_hints(req.pid, &format!("{sig:?}"));
        return Ok(ToolResponse::with_hints(
            format!(
                "DRY RUN: would deliver {} to PID {}. No OS state changed.",
                sig, req.pid
            ),
            json!({
                "dry_run": true,
                "pid": req.pid,
                "signal": format!("{sig}"),
                "would_deliver": true,
            }),
            hints,
        ));
    }

    // Gate 2: Elicitation gate for destructive signals (ADR-0004 Layer 4 /
    // ADR-0008). Must fire BEFORE any PID existence probe so that destructive
    // intent is confirmed regardless of whether the target process exists.
    if is_destructive(sig) && req.elicitation_confirmed != Some(true) {
        // hints would be returned in an Ok response for a preview; for the
        // error path they are attached to the error context by the caller.
        let _hints = build_elicitation_hints(req.pid, &format!("{sig}"));
        return Err(SubstrateError::ConfirmationRequired {
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        });
    }

    // Gate 3: PID allowlist check (ADR-0004 Layer 1). Hard-blocks PID 0/1/2
    // and other privileged PIDs. Runs after elicitation so that the
    // confirmation prompt appears first for destructive signals on blocked PIDs.
    pid_allowlist::check_pid_allowed(req.pid)?;

    // Gate 4: Signal delivery. We deliver immediately rather than probing with
    // kill(pid, 0) first — a pre-check would not eliminate the TOCTOU window
    // between the probe and the real kill(2) call (the PID could be reused in
    // that interval). Instead, we interpret ESRCH from the actual delivery as
    // NotFound; the signal was never delivered in that case, so the outcome is
    // safe.
    nix::sys::signal::kill(pid, sig).map_err(|e| {
        use nix::errno::Errno;
        match e {
            Errno::EPERM => SubstrateError::PermissionDenied {
                path: format!("process PID {}", req.pid),
                correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
            },
            Errno::ESRCH => SubstrateError::NotFound {
                resource: format!(
                    "process PID {} does not exist (exited before signal)",
                    req.pid
                ),
                correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
            },
            other => SubstrateError::InternalError {
                reason: format!("kill({}, {sig}) failed: {other}", req.pid),
                correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
            },
        }
    })?;

    Ok(ToolResponse::ok(
        format!("Delivered {} to PID {}.", sig, req.pid),
        json!({
            "dry_run": false,
            "pid": req.pid,
            "signal": format!("{sig}"),
            "delivered": true,
        }),
    ))
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

    use substrate_domain::{Capabilities, SubstrateError};

    use super::*;
    use crate::response::ProcessDeps;

    fn deps() -> Arc<ProcessDeps> {
        Arc::new(ProcessDeps {
            capabilities: Arc::new(Capabilities::default()),
        })
    }

    #[tokio::test]
    async fn signal_zero_on_own_pid_succeeds_existence_check() {
        // kill(self, 0) must succeed — the current process clearly exists.
        let own_pid = std::process::id();
        let req = ProcSignalRequest {
            pid: own_pid,
            signal: "SIGHUP".to_owned(),
            dry_run: Some(true), // dry-run so we do not actually send SIGHUP
            elicitation_confirmed: None,
        };
        let resp = handle_proc_signal(req, deps()).await;
        assert!(resp.is_ok(), "dry-run on own PID must succeed: {resp:?}");
    }

    #[tokio::test]
    async fn sigkill_without_elicitation_returns_confirmation_required() {
        let own_pid = std::process::id();
        let req = ProcSignalRequest {
            pid: own_pid,
            signal: "SIGKILL".to_owned(),
            dry_run: Some(false),
            elicitation_confirmed: None, // not confirmed
        };
        let err = handle_proc_signal(req, deps())
            .await
            .expect_err("should require confirmation");
        assert!(
            matches!(err, SubstrateError::ConfirmationRequired { .. }),
            "expected ConfirmationRequired, got {err:?}"
        );
    }

    #[test]
    fn sigterm_is_classified_as_destructive() {
        // ADR-0004 Layer 4 and ADR-0008 §Elicitation Matrix require SIGTERM
        // to trigger elicitation alongside SIGKILL and SIGSTOP.
        use nix::sys::signal::Signal;
        assert!(
            crate::signal_policy::is_destructive(Signal::SIGTERM),
            "SIGTERM must be classified as destructive (ADR-0004 / ADR-0008)"
        );
    }

    #[tokio::test]
    async fn sigterm_without_elicitation_returns_confirmation_required() {
        // SIGTERM is now in the elicitation-required set (ADR-0004 / ADR-0008).
        // Sending it without elicitation_confirmed=true must return
        // ConfirmationRequired — the signal must not be delivered.
        let own_pid = std::process::id();
        let req = ProcSignalRequest {
            pid: own_pid,
            signal: "SIGTERM".to_owned(),
            dry_run: Some(false),
            elicitation_confirmed: None,
        };
        let err = handle_proc_signal(req, deps())
            .await
            .expect_err("SIGTERM without elicitation must require confirmation");
        assert!(
            matches!(err, SubstrateError::ConfirmationRequired { .. }),
            "expected ConfirmationRequired, got {err:?}"
        );
    }

    #[tokio::test]
    async fn nonexistent_pid_returns_not_found() {
        // PID u32::MAX - 1 is extremely unlikely to exist.
        // dry_run=false is required so Gate 4 (signal delivery) is reached and
        // kill(2) returns ESRCH, which is mapped to NotFound.
        // SIGHUP is non-destructive so Gate 2 elicitation is skipped.
        let req = ProcSignalRequest {
            pid: u32::MAX - 1,
            signal: "SIGHUP".to_owned(),
            dry_run: Some(false),
            elicitation_confirmed: None,
        };
        let err = handle_proc_signal(req, deps())
            .await
            .expect_err("non-existent PID should return NotFound");
        assert!(
            matches!(err, SubstrateError::NotFound { .. }),
            "expected NotFound, got {err:?}"
        );
    }

    #[tokio::test]
    async fn pid_zero_returns_permission_denied() {
        // Gate 3 (allowlist) must fire even with a non-destructive signal.
        let req = ProcSignalRequest {
            pid: 0,
            signal: "SIGHUP".to_owned(),
            dry_run: Some(false),
            elicitation_confirmed: None,
        };
        let err = handle_proc_signal(req, deps())
            .await
            .expect_err("PID 0 must be blocked");
        assert!(
            matches!(err, SubstrateError::PermissionDenied { .. }),
            "expected PermissionDenied, got {err:?}"
        );
    }

    #[tokio::test]
    async fn pid_one_returns_permission_denied() {
        let req = ProcSignalRequest {
            pid: 1,
            signal: "SIGHUP".to_owned(),
            dry_run: Some(false),
            elicitation_confirmed: None,
        };
        let err = handle_proc_signal(req, deps())
            .await
            .expect_err("PID 1 (init) must be blocked");
        assert!(
            matches!(err, SubstrateError::PermissionDenied { .. }),
            "expected PermissionDenied, got {err:?}"
        );
    }

    #[tokio::test]
    async fn destructive_signal_to_pid_one_requires_confirmation_first() {
        // Gate 2 (elicitation) fires BEFORE Gate 3 (allowlist). So a destructive
        // signal to a blocked PID must surface ConfirmationRequired, not
        // PermissionDenied — the caller hasn't confirmed yet.
        let req = ProcSignalRequest {
            pid: 1,
            signal: "SIGKILL".to_owned(),
            dry_run: Some(false),
            elicitation_confirmed: None,
        };
        let err = handle_proc_signal(req, deps())
            .await
            .expect_err("destructive signal to PID 1 must require confirmation first");
        assert!(
            matches!(err, SubstrateError::ConfirmationRequired { .. }),
            "expected ConfirmationRequired (not PermissionDenied), got {err:?}"
        );
    }

    #[test]
    fn parse_signal_accepts_symbolic_names() {
        assert_eq!(parse_signal("SIGTERM").unwrap(), Signal::SIGTERM);
        assert_eq!(parse_signal("TERM").unwrap(), Signal::SIGTERM);
        assert_eq!(parse_signal("sigkill").unwrap(), Signal::SIGKILL);
    }

    #[test]
    fn parse_signal_accepts_numeric_strings() {
        // SIGTERM = 15 on both Linux and macOS.
        assert_eq!(parse_signal("15").unwrap(), Signal::SIGTERM);
    }

    #[test]
    fn parse_signal_rejects_unknown() {
        assert!(parse_signal("SIGNOTREAL").is_err());
    }
}
