//! `JobBucket` — static dispatch classification for every MCP tool per ADR-0040.
//!
//! Mirrors `#JobBucket` in `docs/arch/schemas/job.cue`.
//! Bucket assignment is compile-time constant per tool except for Bucket B,
//! whose actual inline-vs-job path is decided at runtime based on payload size.

use serde::{Deserialize, Serialize};

/// Classifies every MCP tool into a dispatch bucket per ADR-0040.
///
/// Serialisation uses the exact CUE string values from `#JobBucket`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobBucket {
    /// **A — Sync inline (snapshot-instant).**
    ///
    /// Tool response is returned synchronously within the MCP request lifetime.
    /// Examples: `sys.uname`, `sys.info`, `fs.stat`, `proc.list`.
    #[serde(rename = "A_sync_inline")]
    ASyncInline,

    /// **B — Auto-mode (inline if small, job if large).**
    ///
    /// The inline vs. job path is decided at runtime based on a per-tool size
    /// threshold declared in `[jobs.inline_thresholds]`.
    /// Examples: `fs.find`, `fs.read`, `fs.hash`, `text.search`.
    #[serde(rename = "B_auto_mode")]
    BAutoMode,

    /// **C — Always async (job mandatory).**
    ///
    /// The tool always dispatches an async job regardless of payload size.
    /// Examples: `archive.tar.create`, `archive.zip.extract`.
    #[serde(rename = "C_always_async")]
    CAlwaysAsync,

    /// **D — Sync side-effect (commit fast, audit fire-and-forget).**
    ///
    /// Tool commits its side effect synchronously and returns immediately;
    /// the audit event is written asynchronously in the background.
    /// Examples: `fs.mkdir`, `fs.rename`, `fs.touch`, `proc.signal`.
    #[serde(rename = "D_sync_side_effect")]
    DSyncSideEffect,
}

impl std::fmt::Display for JobBucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::ASyncInline => "A_sync_inline",
            Self::BAutoMode => "B_auto_mode",
            Self::CAlwaysAsync => "C_always_async",
            Self::DSyncSideEffect => "D_sync_side_effect",
        };
        f.write_str(s)
    }
}
