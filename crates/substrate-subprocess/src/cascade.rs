//! Cascade kill chain per ADR-0053 §"Explicit Cleanup Chain".
//!
//! Implements the ordered sequence:
//! 1. Cancel the job's `CancellationToken`.
//! 2. `killpg(pgid, SIGTERM)` — signal the entire process group.
//! 3. Wait up to `drain_secs` for the child to exit.
//! 4. If still alive: `killpg(pgid, SIGKILL)`.
//! 5. Drain stdout/stderr mpsc buffers.
//! 6. Remove any registered tmp files per ADR-0033.
//! 7. Return the terminal [`SubprocessState`].
//!
//! # Invariant
//!
//! This function MUST NOT be written as a `Drop` impl because `panic = "abort"`
//! per ADR-0014 suppresses `Drop`. Cleanup is explicit in the async cancel path.
//!
//! References: ADR-0053 §"Explicit Cleanup Chain", ADR-0014 §"panic=abort".

use std::sync::Arc;
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tracing::{info, warn};

use substrate_domain::subprocess::{SubprocessError, SubprocessState};

use crate::spawn::ChildHandle;

/// Executes the cascade kill chain for `handle`, returning the terminal state.
///
/// When `force` is `true`, `SIGKILL` is sent immediately without the SIGTERM
/// drain window. When `false`, `SIGTERM` is sent first with a `drain_secs` wait.
///
/// This function must be called from within an async context on the tokio runtime.
///
/// # Errors
///
/// Returns `SubprocessError::SpawnFailed` (carrying an `io::Error`) only when
/// the OS-level `killpg` call fails for a reason other than ESRCH (already exited).
///
/// References: ADR-0053 §"Explicit Cleanup Chain", ADR-0053 §"PID Reuse Race Mitigation".
pub async fn terminate_cascade(
    handle: &Arc<ChildHandle>,
    drain_secs: u64,
    force: bool,
) -> Result<SubprocessState, SubprocessError> {
    let job_id = &handle.job_id;
    let pgid = handle.process_group.pgid();

    // Step 1: cancel the job token so reader tasks exit their select! loops.
    handle.cancel.cancel();

    // Step 2 / force path: choose signal and send.
    let first_signal = if force {
        Signal::SIGKILL
    } else {
        Signal::SIGTERM
    };
    let first_event = if force {
        "SUBSTRATE_SUBPROCESS_KILLPG_KILL"
    } else {
        "SUBSTRATE_SUBPROCESS_KILLPG_TERM"
    };

    send_signal_to_group(pgid, first_signal, job_id.to_string().as_str(), first_event);

    let terminal_state = if force {
        // Force: no drain; child may still be running, but SIGKILL is delivered.
        wait_or_kill(handle, pgid, 0, drain_secs, job_id.to_string().as_str()).await
    } else {
        wait_or_kill(handle, pgid, drain_secs, 0, job_id.to_string().as_str()).await
    };

    // Step 6: clean up registered tmp files per ADR-0033.
    let paths: Vec<_> = handle.tmp_files.lock().await.clone();
    let failures = crate::cleanup::cleanup_tmp_files(&paths).await;
    for (path, err) in &failures {
        warn!(
            target: "substrate_audit",
            event = "SUBSTRATE_SUBPROCESS_TMP_CLEANUP_FAILED",
            job_id = %job_id,
            path = %path.display(),
            error = %err,
            "tmp file cleanup failed (non-fatal)"
        );
    }
    if failures.is_empty() {
        info!(
            target: "substrate_audit",
            event = "SUBSTRATE_SUBPROCESS_TMP_CLEANED",
            job_id = %job_id,
            count = paths.len(),
        );
    }

    Ok(terminal_state)
}

/// Sends `signal` to the process group `pgid`, emitting an audit event.
///
/// ESRCH (no such process group) is treated as a no-op — the child already
/// exited. All other errors are logged as warnings but do not abort the chain.
fn send_signal_to_group(pgid: i32, signal: Signal, job_id: &str, event: &str) {
    match killpg(Pid::from_raw(pgid), Some(signal)) {
        Ok(()) => {
            info!(
                target: "substrate_audit",
                event = event,
                job_id = job_id,
                pgid = pgid,
                signal = %signal,
            );
        },
        Err(nix::errno::Errno::ESRCH) => {
            // Process group already gone — not an error.
            info!(
                job_id = job_id,
                pgid = pgid,
                "killpg: ESRCH (process group already exited)"
            );
        },
        Err(e) => {
            warn!(
                job_id = job_id,
                pgid = pgid,
                error = %e,
                "killpg failed (non-fatal; cascade continues)"
            );
        },
    }
}

/// Waits up to `drain_secs` for the child to exit; if it has not, sends SIGKILL.
///
/// Returns the terminal [`SubprocessState`]:
/// - `Cancelled` if the child exited within the drain window.
/// - `Killed` if SIGKILL was required.
///
/// `_force_drain_secs` is reserved for future use (currently unused when force=true).
async fn wait_or_kill(
    handle: &Arc<ChildHandle>,
    pgid: i32,
    drain_secs: u64,
    _force_drain_secs: u64,
    job_id: &str,
) -> SubprocessState {
    if drain_secs == 0 {
        // Immediate SIGKILL path (force=true).
        drain_child(handle).await;
        info!(
            target: "substrate_audit",
            event = "SUBSTRATE_SUBPROCESS_REAPED",
            job_id = job_id,
            pgid = pgid,
        );
        return SubprocessState::Killed;
    }

    // SIGTERM + drain window.
    tokio::select! {
        biased;
        status = handle.wait_exit() => {
            match status {
                Ok(Some(st)) => {
                    info!(
                        target: "substrate_audit",
                        event = "SUBSTRATE_SUBPROCESS_REAPED",
                        job_id = job_id,
                        pgid = pgid,
                        exit_code = ?st.code(),
                    );
                    return SubprocessState::Cancelled;
                },
                Ok(None) => {
                    // Already waited; treat as Cancelled.
                    return SubprocessState::Cancelled;
                },
                Err(e) => {
                    // PID/PGID-reuse mitigation (ADR-0053): wait_exit() already TOOK
                    // the child from the mutex, so on error the leader may already be
                    // reaped by the kernel and its PGID recyclable. Sending killpg
                    // here could signal an unrelated, recycled process group. Treat as
                    // terminal WITHOUT a post-reap killpg.
                    warn!(
                        job_id = job_id,
                        error = %e,
                        "wait_exit errored during drain; leader may be reaped — \
                         skipping post-reap killpg to avoid PGID-reuse race"
                    );
                    return SubprocessState::Killed;
                },
            }
        },
        () = tokio::time::sleep(Duration::from_secs(drain_secs)) => {
            // Drain window expired; the leader is still alive (the wait_exit arm did
            // not win), so its PGID has NOT been reaped/recycled. Escalating to
            // killpg here is race-free.
        },
    }

    // Step 4: drain window expired — SIGKILL the still-live group, THEN reap.
    // Order matters: killpg precedes drain_child so we never signal a recycled PGID.
    send_signal_to_group(
        pgid,
        Signal::SIGKILL,
        job_id,
        "SUBSTRATE_SUBPROCESS_KILLPG_KILL",
    );
    drain_child(handle).await;
    info!(
        target: "substrate_audit",
        event = "SUBSTRATE_SUBPROCESS_REAPED",
        job_id = job_id,
        pgid = pgid,
    );
    SubprocessState::Killed
}

/// Awaits the child process reaping after SIGKILL.
///
/// Silently ignores errors — if the child cannot be waited on it has already
/// been reaped or was never alive.
async fn drain_child(handle: &Arc<ChildHandle>) {
    let _ = handle.wait_exit().await;
}
