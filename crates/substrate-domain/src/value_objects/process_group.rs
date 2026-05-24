//! `ProcessGroup` — value object capturing the OS process group assigned to a spawned child.
//!
//! A `ProcessGroup` is created after `setsid()` succeeds in the pre-exec hook
//! (ADR-0053). The `pgid` equals the child's `pid` when the child is the session
//! leader (the `setsid` pattern). `killpg(pgid, signal)` is the canonical way to
//! send a signal to the entire group, ensuring grandchildren are also reaped.
//!
//! References: ADR-0052 §"`ProcessGroup`", ADR-0053 §"Process Group Leadership".

use serde::{Deserialize, Serialize};

use crate::errors::{SubstrateError, SubstrateResult};

/// OS process group descriptor created after `setsid()` in the pre-exec hook.
///
/// Invariant: both `pid` and `pgid` are >= 2. PIDs 0 (kernel-special) and 1
/// (`init`/`launchd`) are reserved by the OS and are never valid subprocess PIDs
/// per ADR-0035 security policy.
///
/// See ADR-0052 §"`ProcessGroup`" and ADR-0053 §"Process Group Leadership".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessGroup {
    /// OS process identifier of the spawned child. Always >= 2.
    pid: i32,
    /// Process group identifier assigned by `setsid()`. Always >= 2.
    /// Equals `pid` when the child is the process group leader.
    pgid: i32,
}

impl ProcessGroup {
    /// Constructs a `ProcessGroup`, validating that both `pid` and `pgid` are >= 2.
    ///
    /// PIDs 0 and 1 are OS-reserved; PID 2 is the lowest safe subprocess PID.
    ///
    /// # Errors
    ///
    /// Returns `SUBSTRATE_INVALID_ARGUMENT` when either `pid` or `pgid` is < 2.
    #[expect(
        clippy::similar_names,
        reason = "pid and pgid are distinct OS concepts that must retain their POSIX names for clarity"
    )]
    pub fn new(raw_pid: i32, raw_pgid: i32) -> SubstrateResult<Self> {
        if raw_pid < 2 {
            return Err(SubstrateError::InvalidArgument {
                offending_field: "pid".to_owned(),
                reason: format!(
                    "pid must be >= 2 (OS-reserved PIDs 0 and 1 are forbidden); got {raw_pid}"
                ),
                correlation_id: None,
            });
        }
        if raw_pgid < 2 {
            return Err(SubstrateError::InvalidArgument {
                offending_field: "pgid".to_owned(),
                reason: format!(
                    "pgid must be >= 2 (OS-reserved PGIDs 0 and 1 are forbidden); got {raw_pgid}"
                ),
                correlation_id: None,
            });
        }
        Ok(Self {
            pid: raw_pid,
            pgid: raw_pgid,
        })
    }

    /// Returns the OS process identifier.
    #[must_use]
    pub const fn pid(&self) -> i32 {
        self.pid
    }

    /// Returns the process group identifier assigned by `setsid()`.
    #[must_use]
    pub const fn pgid(&self) -> i32 {
        self.pgid
    }

    /// Returns `true` when the child is the leader of its own process group.
    ///
    /// This is always the case after a successful `setsid()` call per ADR-0053,
    /// where the child's `pgid` is set to its own `pid`.
    #[must_use]
    pub const fn is_group_leader(&self) -> bool {
        self.pid == self.pgid
    }
}

impl std::fmt::Display for ProcessGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pid={} pgid={}", self.pid, self.pgid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_process_group_constructed() {
        #[expect(
            clippy::expect_used,
            reason = "test assertion: valid values must succeed"
        )]
        let pg = ProcessGroup::new(1000, 1000).expect("pid=1000, pgid=1000 must be valid");
        assert_eq!(pg.pid(), 1000);
        assert_eq!(pg.pgid(), 1000);
        assert!(pg.is_group_leader(), "pid == pgid => is_group_leader");
    }

    #[test]
    fn pid_zero_rejected() {
        assert!(
            ProcessGroup::new(0, 1000).is_err(),
            "pid=0 must be rejected"
        );
    }

    #[test]
    fn pid_one_rejected() {
        assert!(
            ProcessGroup::new(1, 1000).is_err(),
            "pid=1 must be rejected"
        );
    }

    #[test]
    fn pgid_zero_rejected() {
        assert!(
            ProcessGroup::new(1000, 0).is_err(),
            "pgid=0 must be rejected"
        );
    }

    #[test]
    fn pgid_one_rejected() {
        assert!(
            ProcessGroup::new(1000, 1).is_err(),
            "pgid=1 must be rejected"
        );
    }

    #[test]
    fn non_leader_when_pid_ne_pgid() {
        #[expect(
            clippy::expect_used,
            reason = "test assertion: valid values must succeed"
        )]
        let pg = ProcessGroup::new(1001, 1000).expect("pid=1001, pgid=1000 must be valid");
        assert!(!pg.is_group_leader(), "pid != pgid => not a group leader");
    }
}
