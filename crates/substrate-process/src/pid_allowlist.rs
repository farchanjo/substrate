//! PID allowlist for `proc.signal`. ADR-0004 Layer 1.
//!
//! Enforces a hard block on privileged and kernel-managed PIDs, and on the
//! substrate server's own process identity, before any signal delivery or
//! existence check is performed.

use std::sync::OnceLock;

use substrate_domain::SubstrateError;

/// PIDs that MUST never receive any signal from substrate.
///
/// - `0` — POSIX "send to whole process group" semantic; dangerous.
/// - `1` — `init`/`systemd`; killing it crashes the host.
/// - `2` — `kthreadd` (Linux kernel thread manager); kernel-managed.
const HARD_BLOCKED: &[u32] = &[0, 1, 2];

/// Returns the substrate server's own PID plus its process-group and
/// session-leader PIDs, resolved once and cached for the life of the process.
///
/// WHY: `proc.signal` is driven by an LLM agent acting on natural-language
/// intent, which is untrusted input from the server's point of view. Without
/// this guard, an agent could ask substrate to deliver `SIGKILL` (or any
/// other signal) to itself — or to the process group / session leader that
/// owns it — which would tear the process down immediately and bypass the
/// ADR-0032 graceful-drain shutdown path entirely (no drain window, no
/// in-flight job cleanup, no final flush). The server's PID, PGID, and SID
/// never change after startup, so they are resolved lazily on first use
/// rather than re-queried on every call.
fn self_protected_pids() -> &'static [u32] {
    static SELF_PIDS: OnceLock<Vec<u32>> = OnceLock::new();
    SELF_PIDS.get_or_init(|| {
        let mut pids = vec![std::process::id()];

        // getpgrp(2) is documented as always successful (POSIX); it returns
        // the calling process's own process-group ID, which for a
        // foreground-launched server is often the owning shell/supervisor's
        // PID and MUST NOT be signalable via this tool either.
        if let Ok(pgrp) = u32::try_from(nix::unistd::getpgrp().as_raw()) {
            pids.push(pgrp);
        }

        // getsid(None) resolves the caller's own session-leader PID. This is
        // best-effort: a failure here (e.g. no controlling session in a
        // sandboxed environment) just means one fewer identity to guard, not
        // a hard failure of the allowlist check itself.
        if let Ok(sid_pid) = nix::unistd::getsid(None)
            && let Ok(sid) = u32::try_from(sid_pid.as_raw())
        {
            pids.push(sid);
        }

        pids
    })
}

/// Returns `Err(SUBSTRATE_PERMISSION_DENIED)` if `pid` is hard-blocked.
///
/// A `pid` is hard-blocked when it is in the static `HARD_BLOCKED` list or
/// identifies the substrate server itself (its own PID, process group, or
/// session leader — see [`self_protected_pids`]).
///
/// This check MUST run before any process-existence probe so that blocked PIDs
/// never reveal whether the process exists.
///
/// # Errors
///
/// Returns [`SubstrateError::PermissionDenied`] when `pid` is in
/// `HARD_BLOCKED` or matches one of the server's self-protected PIDs.
pub fn check_pid_allowed(pid: u32) -> Result<(), SubstrateError> {
    if HARD_BLOCKED.contains(&pid) || self_protected_pids().contains(&pid) {
        return Err(SubstrateError::PermissionDenied {
            path: format!(
                "PID {pid} is a privileged, kernel, or substrate-server PID and cannot receive signals from substrate"
            ),
            correlation_id: None,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use substrate_domain::SubstrateError;

    use super::check_pid_allowed;

    #[test]
    fn pid_zero_is_blocked() {
        assert!(matches!(
            check_pid_allowed(0),
            Err(SubstrateError::PermissionDenied { .. })
        ));
    }

    #[test]
    fn pid_one_is_blocked() {
        assert!(matches!(
            check_pid_allowed(1),
            Err(SubstrateError::PermissionDenied { .. })
        ));
    }

    #[test]
    fn pid_two_is_blocked() {
        assert!(matches!(
            check_pid_allowed(2),
            Err(SubstrateError::PermissionDenied { .. })
        ));
    }

    #[test]
    fn pid_three_is_allowed() {
        assert!(check_pid_allowed(3).is_ok());
    }

    #[test]
    fn own_pid_is_blocked() {
        // A `proc.signal` call targeting the substrate server's own PID must
        // be rejected, not accepted — sending it a signal (SIGKILL in
        // particular) would bypass the ADR-0032 graceful-drain shutdown path.
        let own = std::process::id();
        assert!(matches!(
            check_pid_allowed(own),
            Err(SubstrateError::PermissionDenied { .. })
        ));
    }

    #[test]
    fn own_process_group_is_blocked() {
        // pid_t is always non-negative for a live process group on any real
        // POSIX system; if the conversion ever failed there would be nothing
        // meaningful to assert.
        let Ok(pgrp) = u32::try_from(nix::unistd::getpgrp().as_raw()) else {
            return;
        };
        assert!(matches!(
            check_pid_allowed(pgrp),
            Err(SubstrateError::PermissionDenied { .. })
        ));
    }

    #[test]
    fn own_session_leader_is_blocked() {
        // No controlling session in this environment (e.g. some CI
        // sandboxes) or a non-positive pid_t — nothing to assert either way.
        let Ok(sid_pid) = nix::unistd::getsid(None) else {
            return;
        };
        let Ok(sid) = u32::try_from(sid_pid.as_raw()) else {
            return;
        };
        assert!(matches!(
            check_pid_allowed(sid),
            Err(SubstrateError::PermissionDenied { .. })
        ));
    }
}
