//! `ProcessInfo` — point-in-time snapshot of a single running process.
//!
//! Maps to the `ProcessSnapshot` aggregate root defined in the process BC
//! narrative (`docs/arch/domain/process/README.md`). Serialised as-is into
//! `structuredContent`.

use serde::{Deserialize, Serialize};

/// Point-in-time snapshot of a single running process.
///
/// Produced by the platform scanner and returned by `proc.list` and
/// `proc.tree`. Fields that cannot be read without elevated privilege
/// are represented as `Option` and carry `None` rather than failing the
/// entire scan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessInfo {
    /// POSIX process identifier.
    pub pid: u32,

    /// Parent process identifier. `0` for PID 1 (init/launchd).
    pub ppid: u32,

    /// Short command name (basename of argv[0], no arguments).
    pub name: String,

    /// Full command line, space-joined. Empty string when not readable.
    pub command: String,

    /// Real user ID that owns this process.
    pub uid: u32,

    /// Real group ID that owns this process.
    pub gid: u32,

    /// CPU usage percentage over the last sample window.
    ///
    /// Returns `0.0` in the current implementation; delta-based measurement
    /// is deferred to Wave G (TODO: implement two-sample CPU delta).
    pub cpu_pct: f32,

    /// Resident set size in kilobytes.
    pub rss_kb: u64,

    /// Virtual memory size in kilobytes.
    pub vm_kb: u64,

    /// Process start time as Unix epoch seconds, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time_unix: Option<i64>,

    /// Single-character process state (R, S, D, Z, T, …) per OS conventions.
    pub state: String,
}

impl ProcessInfo {
    /// Returns `true` if the process appears to be a zombie (`Z` state).
    #[must_use]
    pub fn is_zombie(&self) -> bool {
        self.state.starts_with('Z')
    }
}
