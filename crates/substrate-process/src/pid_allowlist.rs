//! PID allowlist for `proc.signal`. ADR-0004 Layer 1.
//!
//! Enforces a hard block on privileged and kernel-managed PIDs before any
//! signal delivery or existence check is performed.

use substrate_domain::SubstrateError;

/// PIDs that MUST never receive any signal from substrate.
///
/// - `0` — POSIX "send to whole process group" semantic; dangerous.
/// - `1` — `init`/`systemd`; killing it crashes the host.
/// - `2` — `kthreadd` (Linux kernel thread manager); kernel-managed.
const HARD_BLOCKED: &[u32] = &[0, 1, 2];

/// Returns `Err(SUBSTRATE_PERMISSION_DENIED)` if `pid` is in the hard-blocked
/// list.
///
/// This check MUST run before any process-existence probe so that blocked PIDs
/// never reveal whether the process exists.
///
/// # Errors
///
/// Returns [`SubstrateError::PermissionDenied`] when `pid` is in `HARD_BLOCKED`.
pub fn check_pid_allowed(pid: u32) -> Result<(), SubstrateError> {
    if HARD_BLOCKED.contains(&pid) {
        return Err(SubstrateError::PermissionDenied {
            path: format!(
                "PID {pid} is a privileged or kernel PID and cannot receive signals from substrate"
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
    fn own_pid_is_allowed() {
        let own = std::process::id();
        // Own PID is always > 2 in a real process.
        assert!(check_pid_allowed(own).is_ok());
    }
}
