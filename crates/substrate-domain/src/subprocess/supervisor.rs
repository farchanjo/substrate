//! Supervisor policy value objects per ADR-0056.
//!
//! `RestartPolicy`, `HealthProbe`, and `LogRotation` are operator-supplied
//! enums attached to a `SubprocessRequest` to opt into supervisor semantics.
//! All three preserve backward compatibility: omitting any of them keeps the
//! original one-shot subprocess behavior from ADR-0052.

use serde::{Deserialize, Serialize};

/// Restart policy controlling supervisor re-spawn behavior per ADR-0056.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "PascalCase")]
pub enum RestartPolicy {
    /// No restart on exit. Default — preserves one-shot semantics.
    Never,
    /// Re-spawn on non-zero exit, up to `max_retries`, with a fixed backoff
    /// (with optional jitter up to `subprocess.supervisor_max_restart_jitter_pct`
    /// percent, capped at `backoff_ms`). Retry counter resets after
    /// `2 * backoff_ms` milliseconds stable in `Ready` state.
    OnFailure {
        /// Maximum number of consecutive re-spawn attempts. Range: 1..=100.
        max_retries: u32,
        /// Fixed wait before each re-spawn, in milliseconds. Range: 100..=300000.
        backoff_ms: u64,
    },
    /// Re-spawn on any exit (zero or non-zero) with the given backoff,
    /// indefinitely until the job is explicitly cancelled.
    Always {
        /// Fixed wait before each re-spawn, in milliseconds. Range: 100..=300000.
        backoff_ms: u64,
    },
}

/// Health probe gating the `Starting` -> `Ready` state transition per ADR-0056.
///
/// Three consecutive probe failures trigger the `restart_policy`.
/// When `HealthProbe::None`, `Starting` exits to `Ready` atomically within the
/// same scheduler tick, preserving backward compatibility per ADR-0056.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "PascalCase")]
pub enum HealthProbe {
    /// No probe. `Running == Ready` immediately.
    None,
    /// HTTP GET poll against `url`. Requires Cargo feature `outbound-net`.
    ///
    /// The probe fires after `startup_grace_ms` and then every `interval_ms`
    /// while the state is `Starting`. Transitions to `Ready` on the first
    /// response matching `expected_status`.
    HttpGet {
        /// Absolute `http://` or `https://` URL to poll.
        url: String,
        /// Expected HTTP status code. Range: 100..=599.
        expected_status: u16,
        /// Polling cadence while `Starting`, in milliseconds. Range: 100..=60000.
        interval_ms: u64,
        /// Initial delay before the first probe fires, in milliseconds.
        /// Range: 0..=600000.
        startup_grace_ms: u64,
    },
    /// TCP connect check against `host:port`.
    ///
    /// The probe fires after `startup_grace_ms` and then every `interval_ms`
    /// while the state is `Starting`. A successful TCP connect transitions
    /// the state to `Ready`.
    PortOpen {
        /// Hostname or IP address to connect to.
        host: String,
        /// TCP port to connect to. Range: 1..=65535.
        port: u16,
        /// Polling cadence while `Starting`, in milliseconds. Range: 100..=60000.
        interval_ms: u64,
        /// Initial delay before the first probe fires, in milliseconds.
        /// Range: 0..=600000.
        startup_grace_ms: u64,
    },
    /// One-shot regex scan over stdout/stderr stream chunks until match OR timeout.
    ///
    /// Each stream chunk is matched against `regex` as it arrives. On the first
    /// match the state transitions to `Ready`. If `timeout_ms` elapses without a
    /// match, the state transitions to `Failed` per ADR-0056.
    LogPattern {
        /// Non-empty regex applied to each stream chunk.
        regex: String,
        /// Maximum wait for the pattern match, in milliseconds. Range: 1000..=600000.
        timeout_ms: u64,
    },
}

/// Log rotation configuration for `capture_kind = TmpFile` per ADR-0056.
///
/// Active only when `capture_kind == CaptureKind::TmpFile`. When omitted, the
/// temporary file grows unbounded (original one-shot behavior).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "PascalCase")]
pub enum LogRotation {
    /// No rotation. The tmp file grows unbounded.
    None,
    /// Rotate when the current file reaches `max_bytes_per_file` bytes.
    ///
    /// Keeps the last `keep_files` rotated files; the oldest beyond that limit
    /// are unlinked. Cumulative storage cap = `max_bytes_per_file * keep_files`.
    BySize {
        /// Maximum size of each rotated file in bytes. Range: `1_048_576..=1_073_741_824` (1 MiB..=1 GiB).
        max_bytes_per_file: u64,
        /// Number of rotated files to keep. Range: 1..=20.
        keep_files: u8,
    },
}
