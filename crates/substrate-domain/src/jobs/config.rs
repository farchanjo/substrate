//! Job control-plane configuration value objects per ADR-0040.
//!
//! Mirrors `#JobQuotas`, `#JobInlineThresholds`, `#JobTimeouts`, and `#JobConfig`
//! from `docs/arch/schemas/job.cue`. Loaded from the TOML `[jobs]` section by
//! `substrate-config`.

use serde::{Deserialize, Serialize};

/// Resource limits for the async job control-plane per ADR-0040.
///
/// All fields have safe defaults matching the CUE schema `| *<default>` values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobQuotas {
    /// Global limit on active (pending + running) jobs. Default: 16.
    pub max_concurrent: u32,

    /// Per-client active job limit. Default: 4.
    pub max_per_client: u32,

    /// Retention period in seconds after terminal state entry. Default: 300.
    ///
    /// After eviction, `job.result` and `job.status` return
    /// `SUBSTRATE_JOB_NOT_FOUND`.
    pub result_ttl_secs: u32,

    /// Cap in milliseconds for the `wait_ms` parameter of `job.result`. Default: 30000.
    pub result_max_wait_ms: u32,

    /// Substituted `wait_ms` value when callers omit the field per ADR-0059. Default: 5000.
    ///
    /// Invariant: `0 < result_default_wait_ms <= result_max_wait_ms`. An explicit
    /// `wait_ms = 0` in the request payload is honored unchanged; only field-absence
    /// triggers substitution.
    ///
    /// Carries a serde default so existing TOML configs (predating ADR-0059) that
    /// enumerate `[jobs.quotas]` fields explicitly continue to deserialize without
    /// requiring an operator-side edit.
    #[serde(default = "default_result_default_wait_ms")]
    pub result_default_wait_ms: u32,

    /// Minimum emission interval between progress events in milliseconds. Default: 250.
    ///
    /// Events are also suppressed unless the progress delta >= 1 percentage point.
    pub progress_interval_ms: u32,

    /// Bounded mpsc channel capacity per job for progress events. Default: 64.
    ///
    /// Events submitted via `try_send` when the channel is full are dropped and counted
    /// in `progress_events_dropped`.
    pub progress_channel_size: u32,

    /// Background GC wake interval in seconds for evicting expired jobs. Default: 60.
    pub gc_interval_secs: u32,
}

const fn default_result_default_wait_ms() -> u32 {
    5_000
}

impl Default for JobQuotas {
    fn default() -> Self {
        Self {
            max_concurrent: 16,
            max_per_client: 4,
            result_ttl_secs: 300,
            result_max_wait_ms: 30_000,
            result_default_wait_ms: 5_000,
            progress_interval_ms: 250,
            progress_channel_size: 64,
            gc_interval_secs: 60,
        }
    }
}

impl JobQuotas {
    /// Validates the cross-field invariant
    /// `0 < result_default_wait_ms <= result_max_wait_ms` per ADR-0059.
    ///
    /// Returns the offending field name + message when violated. The composition
    /// root calls this after TOML load and refuses to start on violation
    /// (fail-closed per ADR-0004).
    ///
    /// # Errors
    ///
    /// Returns `Err` when the configured default-wait is zero or exceeds the
    /// configured maximum-wait.
    pub const fn validate_wait_window(&self) -> Result<(), &'static str> {
        if self.result_default_wait_ms == 0 {
            return Err("jobs.quotas.result_default_wait_ms must be > 0 per ADR-0059");
        }
        if self.result_default_wait_ms > self.result_max_wait_ms {
            return Err(
                "jobs.quotas.result_default_wait_ms must be <= result_max_wait_ms per ADR-0059",
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod quota_tests {
    use super::JobQuotas;

    #[test]
    fn default_passes_wait_window_invariant() {
        assert!(JobQuotas::default().validate_wait_window().is_ok());
    }

    #[test]
    fn zero_default_wait_is_rejected() {
        let q = JobQuotas {
            result_default_wait_ms: 0,
            ..JobQuotas::default()
        };
        assert!(q.validate_wait_window().is_err());
    }

    #[test]
    fn default_above_max_is_rejected() {
        let q = JobQuotas {
            result_max_wait_ms: 1_000,
            result_default_wait_ms: 5_000,
            ..JobQuotas::default()
        };
        assert!(q.validate_wait_window().is_err());
    }

    #[test]
    fn equal_default_and_max_is_allowed() {
        let q = JobQuotas {
            result_max_wait_ms: 5_000,
            result_default_wait_ms: 5_000,
            ..JobQuotas::default()
        };
        assert!(q.validate_wait_window().is_ok());
    }
}

/// Per-tool inline thresholds for Bucket B auto-mode dispatch per ADR-0040.
///
/// A tool invocation below its threshold returns an inline result; at or above
/// the threshold the tool is promoted to an async job.
///
/// Mirrors `#JobInlineThresholds` in `docs/arch/schemas/job.cue`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInlineThresholds {
    /// Inline if the candidate count is below this value. Default: 1000.
    pub fs_find_inline_entries: u64,

    /// Inline if the file byte size is below this value. Default: 1 MiB.
    pub fs_read_inline_bytes: u64,

    /// Inline if the input byte size is below this value. Default: 4 MiB.
    pub fs_hash_inline_bytes: u64,

    /// Inline if the source file size is below this value. Default: 1 MiB.
    pub fs_copy_inline_bytes: u64,

    /// Inline if the file byte size is below this value. Default: 512 KiB.
    pub text_search_inline_bytes: u64,

    /// Inline if the file byte size is below this value. Default: 512 KiB.
    pub text_count_lines_inline_bytes: u64,

    /// Inline if the uncompressed byte size is below this value. Default: 128 KiB.
    pub archive_gzip_inline_bytes: u64,

    /// Inline if the archive byte size is below this value. Default: 4 MiB.
    pub archive_hash_inline_bytes: u64,
}

impl Default for JobInlineThresholds {
    fn default() -> Self {
        Self {
            fs_find_inline_entries: 1_000,
            fs_read_inline_bytes: 1_048_576,        // 1 MiB
            fs_hash_inline_bytes: 4_194_304,        // 4 MiB
            fs_copy_inline_bytes: 1_048_576,        // 1 MiB
            text_search_inline_bytes: 524_288,      // 512 KiB
            text_count_lines_inline_bytes: 524_288, // 512 KiB
            archive_gzip_inline_bytes: 131_072,     // 128 KiB
            archive_hash_inline_bytes: 4_194_304,   // 4 MiB
        }
    }
}

/// Per-tool execution time limits for async jobs per ADR-0040.
///
/// Per-tool entries override the default. All values are in seconds.
/// Mirrors `#JobTimeouts` in `docs/arch/schemas/job.cue`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobTimeouts {
    /// Default timeout in seconds when no per-tool override is present. Default: 600.
    pub default_secs: u32,

    /// Cap for `archive.tar.create` and `archive.zip.create` jobs. Default: 1800.
    pub archive_create_secs: u32,

    /// Cap for `archive.tar.extract` and `archive.zip.extract` jobs. Default: 1800.
    pub archive_extract_secs: u32,

    /// Cap for `fs.find` jobs promoted to Bucket C. Default: 60.
    pub fs_find_secs: u32,

    /// Cap for `fs.hash` jobs in Bucket B or C. Default: 600.
    pub fs_hash_secs: u32,
}

impl Default for JobTimeouts {
    fn default() -> Self {
        Self {
            default_secs: 600,
            archive_create_secs: 1_800,
            archive_extract_secs: 1_800,
            fs_find_secs: 60,
            fs_hash_secs: 600,
        }
    }
}

/// Top-level configuration aggregate for the async job control-plane per ADR-0040.
///
/// Embedded in the main `RuntimeConfig` under the `[jobs]` TOML section.
/// Mirrors `#JobConfig` in `docs/arch/schemas/job.cue`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobConfig {
    /// Resource limits (concurrency, TTL, channel sizes).
    pub quotas: JobQuotas,

    /// Per-tool size thresholds for Bucket B auto-mode.
    pub inline_thresholds: JobInlineThresholds,

    /// Per-tool execution time limits.
    pub timeouts: JobTimeouts,
}
