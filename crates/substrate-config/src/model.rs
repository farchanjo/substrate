//! Configuration model types mirroring `docs/arch/schemas/runtime_config.cue`,
//! `docs/arch/schemas/index_config.cue`, and `docs/arch/schemas/security_policy.cue`.
//!
//! Every struct uses `#[serde(deny_unknown_fields)]` per ADR-0006 so that typos
//! in operator TOML fail at load time with a clear error.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use substrate_domain::{CapabilityOverride, jobs::config::JobConfig};

// ---- Top-level aggregate root ------------------------------------------------

/// Top-level runtime configuration aggregate root.
///
/// Mirrors `#RuntimeConfig` in `docs/arch/schemas/runtime_config.cue`.
/// All sub-sections have safe defaults; an empty TOML file is valid.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RuntimeConfig {
    /// Structured log emission controls.
    pub logging: LoggingConfig,

    /// Allowlist policy: the set of absolute path roots accessible to tools.
    ///
    /// Mirrors `#Allowlist.roots` in `docs/arch/schemas/security_policy.cue`.
    /// A process with an empty allowlist refuses all path-jail operations.
    /// The field defaults to an empty `Vec` so that an empty TOML file is
    /// structurally valid, but the composition root will fail at `Allowlist::new`
    /// if `policy.roots` is unset (fail-closed per ADR-0004).
    #[serde(default)]
    pub policy: PolicyConfig,

    /// Runtime-level security hardening knobs.
    pub security: SecurityRuntime,

    /// Execution time limits (global default + per-tool overrides).
    pub timeouts: Timeouts,

    /// Concurrent execution semaphore caps.
    pub semaphore_caps: SemaphoreCaps,

    /// MCP wire-level protocol tunables.
    pub protocol: ProtocolConfig,

    /// Graceful-shutdown drain window in seconds (default 5, max 120).
    ///
    /// Per ADR-0032: SIGTERM/SIGINT trigger graceful drain up to this ceiling.
    #[serde(default = "default_5")]
    pub shutdown_drain_secs: u32,

    /// Async job control-plane quotas and thresholds per ADR-0040.
    ///
    /// Optional only at the TOML layer: when the `[jobs]` section is omitted the
    /// composition root applies `JobConfig::default()` (ADR-0040 defaults). The
    /// control-plane is always wired — there is no disabled mode — so Bucket B/C
    /// tools always promote to background jobs.
    #[serde(default)]
    pub jobs: Option<JobConfig>,

    /// Optional in-process filesystem index per ADR-0041.
    ///
    /// Disabled by default; requires the `fs-index` Cargo feature.
    #[serde(default)]
    pub index: Option<IndexConfig>,

    /// Operator-supplied capability tier overrides per ADR-0042.
    ///
    /// Useful for integration testing specific tier paths.
    #[serde(default)]
    pub capabilities: Option<CapabilitiesSection>,

    /// SIMD tier opt-in configuration per ADR-0043.
    #[serde(default)]
    pub simd: Option<SimdConfig>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            logging: LoggingConfig::default(),
            policy: PolicyConfig::default(),
            security: SecurityRuntime::default(),
            timeouts: Timeouts::default(),
            semaphore_caps: SemaphoreCaps::default(),
            protocol: ProtocolConfig::default(),
            shutdown_drain_secs: default_5(),
            jobs: None,
            index: None,
            capabilities: None,
            simd: None,
        }
    }
}

// ---- Logging -----------------------------------------------------------------

/// Minimum structured log verbosity level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// All spans and events including internal tracing detail.
    Trace,
    /// Internal debug information useful during development.
    Debug,
    /// Normal operational messages.
    #[default]
    Info,
    /// Conditions that are not errors but may require attention.
    Warn,
    /// Error conditions; the operation failed.
    Error,
}

/// Log output destination.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogTarget {
    /// All log lines written to `stderr` (default).
    #[default]
    Stderr,
    /// Log lines written to a rotating file at `file_path`.
    File,
}

/// Behavior when a log write fails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogWriteErrorPolicy {
    /// Emit a warning to stderr and continue (default).
    #[default]
    WarnStderrFallback,
    /// Terminate the process to preserve audit integrity.
    Abort,
}

/// Structured log emission controls per `#LoggingConfig` in `runtime_config.cue`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LoggingConfig {
    /// Minimum severity emitted. Default: `info`.
    #[serde(default)]
    pub level: LogLevel,

    /// Output destination. Default: `stderr`.
    #[serde(default)]
    pub target: LogTarget,

    /// Required when `target = "file"`; must be an absolute path.
    #[serde(default)]
    pub file_path: Option<PathBuf>,

    /// Additional redaction patterns (Go-compatible regex) applied before any
    /// log line is written; matches are replaced with `[REDACTED]`.
    #[serde(default)]
    pub redaction_extra_patterns: Vec<String>,

    /// Rolling size ceiling for a log file before rotation (default 100 MiB).
    #[serde(default = "default_100_mib_u64")]
    pub max_log_file_bytes: u64,

    /// Number of rotated log files retained on disk (default 7).
    #[serde(default = "default_7")]
    pub log_rotate_count: u32,

    /// Behavior on log write failure. Default: `warn_stderr_fallback`.
    #[serde(default)]
    pub log_write_error_policy: LogWriteErrorPolicy,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::default(),
            target: LogTarget::default(),
            file_path: None,
            redaction_extra_patterns: Vec::new(),
            max_log_file_bytes: default_100_mib_u64(),
            log_rotate_count: default_7(),
            log_write_error_policy: LogWriteErrorPolicy::default(),
        }
    }
}

// ---- PolicyConfig -----------------------------------------------------------

/// Allowlist policy configuration per `#Allowlist` in `docs/arch/schemas/security_policy.cue`.
///
/// Embedded under the `[policy]` TOML section.
/// An empty `roots` list is valid TOML but causes the composition root to fail
/// with `SUBSTRATE_CONFIG_INVALID` at startup (fail-closed per ADR-0004).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PolicyConfig {
    /// Ordered list of absolute directory prefixes that tools may access.
    ///
    /// All entries are canonicalized at composition-root startup; symlinks in
    /// root paths are rejected per ADR-0035 §Decision 5.
    #[serde(default)]
    pub roots: Vec<PathBuf>,
}

// ---- Security ----------------------------------------------------------------

/// Runtime-level security hardening knobs per `#SecurityRuntime` in `runtime_config.cue`.
#[expect(
    clippy::struct_excessive_bools,
    reason = "security config intentionally exposes individual on/off knobs; a state-machine would be less ergonomic for TOML deserialization"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SecurityRuntime {
    /// Refuse hard links to files outside the allowlist. Default: `false`.
    #[serde(default = "default_false")]
    pub reject_hardlinks: bool,

    /// Allow symlinks inside extracted archive contents. Default: `false`.
    #[serde(default = "default_false")]
    pub archive_allow_symlinks: bool,

    /// RSS ceiling for the substrate process in bytes (default 256 MiB).
    ///
    /// The runtime raises `SUBSTRATE_RESOURCE_LIMIT` when the limit is exceeded.
    #[serde(default = "default_256_mib_u64")]
    pub max_process_rss_bytes: u64,

    /// Abort startup when `PathJail` falls back to the userspace-degraded tier.
    ///
    /// Per ADR-0035 and ADR-0042. Default: `true` (fail-closed). Operators who
    /// accept the TOCTOU risk must set this to `false` explicitly.
    #[serde(default = "default_true")]
    pub refuse_degraded_jail: bool,

    /// Abort startup when `FsWatcher` falls back to `PollingWatcher`. Default: `false`.
    #[serde(default = "default_false")]
    pub refuse_polling_watcher: bool,

    /// Emit a `tracing::info!` line listing all chosen adapter tiers at startup
    /// per ADR-0042. Default: `true`.
    #[serde(default = "default_true")]
    pub log_tier_on_startup: bool,
}

impl Default for SecurityRuntime {
    fn default() -> Self {
        Self {
            reject_hardlinks: false,
            archive_allow_symlinks: false,
            max_process_rss_bytes: default_256_mib_u64(),
            refuse_degraded_jail: true,
            refuse_polling_watcher: false,
            log_tier_on_startup: true,
        }
    }
}

// ---- Timeouts ----------------------------------------------------------------

/// Execution time limits per `#Timeouts` in `runtime_config.cue`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Timeouts {
    /// Global default timeout in seconds when no per-tool override is present. Default: 30.
    #[serde(default = "default_30")]
    pub global_default_seconds: u32,

    /// Per-tool timeout overrides; keys are tool names (e.g. `"fs.find"`).
    #[serde(default)]
    pub per_tool: BTreeMap<String, u32>,

    /// Graceful-shutdown drain ceiling in seconds (redundant with top-level field;
    /// kept for backward compat with config files that nest it here). Default: 5.
    #[serde(default = "default_5")]
    pub shutdown_drain_secs: u32,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            global_default_seconds: default_30(),
            per_tool: BTreeMap::new(),
            shutdown_drain_secs: default_5(),
        }
    }
}

// ---- SemaphoreCaps -----------------------------------------------------------

/// Concurrent execution semaphore caps per `#SemaphoreCaps` in `runtime_config.cue`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SemaphoreCaps {
    /// Maximum concurrent CPU-bound tool executions.
    #[serde(default = "default_cpu_bound_max")]
    pub cpu_bound_max: u32,

    /// Per-namespace concurrency ceilings; keys are tool namespaces (e.g. `"fs"`).
    #[serde(default)]
    pub per_namespace: BTreeMap<String, u32>,

    /// Maximum callers queued behind a full semaphore. Default: 256.
    #[serde(default = "default_256_u32")]
    pub max_waiters: u32,

    /// Zone-B concurrency cap. When absent, computed as `num_cpus * 4` at startup.
    #[serde(default)]
    pub zone_b_max: Option<u32>,
}

impl Default for SemaphoreCaps {
    fn default() -> Self {
        Self {
            cpu_bound_max: default_cpu_bound_max(),
            per_namespace: BTreeMap::new(),
            max_waiters: default_256_u32(),
            zone_b_max: None,
        }
    }
}

// ---- ProtocolConfig ----------------------------------------------------------

/// MCP wire-level protocol tunables per `#ProtocolConfig` in `runtime_config.cue`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProtocolConfig {
    /// Hard ceiling for pagination; clients may not exceed this. Default: 500.
    #[serde(default = "default_500")]
    pub max_page_size: u32,

    /// Default page size when the client omits `page_size`. Default: 50.
    #[serde(default = "default_50")]
    pub default_page_size: u32,

    /// Cap for single in-memory read/write operations in bytes (default 8 MiB, hard ceiling 32 MiB).
    #[serde(default = "default_8_mib_u64")]
    pub max_in_memory_buffer_bytes: u64,

    /// Cap on the decompressed size of any archive processed in bytes (default 1 GiB).
    #[serde(default = "default_1_gib_u64")]
    pub max_archive_input_bytes: u64,

    /// Maximum concurrent JSON-RPC requests before `SUBSTRATE_RESOURCE_LIMIT`. Default: 32.
    #[serde(default = "default_32")]
    pub max_in_flight_requests: u32,

    /// Maximum single inbound JSON-RPC message size in bytes (default 1 MiB).
    #[serde(default = "default_1_mib_u64")]
    pub max_inbound_message_bytes: u64,

    /// Maximum time in seconds to wait for an elicitation response. Default: 60.
    #[serde(default = "default_60")]
    pub elicitation_timeout_secs: u32,

    /// Maximum outbound frame queue depth per connection. Default: 1024.
    #[serde(default = "default_1024")]
    pub max_outbound_frame_queue: u32,

    /// Write timeout in seconds for outbound frames before closing a stalled connection. Default: 30.
    #[serde(default = "default_30")]
    pub write_timeout_secs: u32,
}

impl Default for ProtocolConfig {
    fn default() -> Self {
        Self {
            max_page_size: default_500(),
            default_page_size: default_50(),
            max_in_memory_buffer_bytes: default_8_mib_u64(),
            max_archive_input_bytes: default_1_gib_u64(),
            max_in_flight_requests: default_32(),
            max_inbound_message_bytes: default_1_mib_u64(),
            elicitation_timeout_secs: default_60(),
            max_outbound_frame_queue: default_1024(),
            write_timeout_secs: default_30(),
        }
    }
}

// ---- IndexConfig -------------------------------------------------------------

/// Optional in-process filesystem index configuration per ADR-0041.
///
/// Mirrors `#IndexConfig` in `docs/arch/schemas/index_config.cue`.
/// Embedded under the `[index]` TOML section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct IndexConfig {
    /// Activate the in-process filesystem index. Default: `false`.
    ///
    /// Requires the `fs-index` Cargo feature to be compiled in.
    #[serde(default = "default_false")]
    pub enabled: bool,

    /// Activate the filesystem watcher layer (Layer 2) per ADR-0041. Default: `false`.
    ///
    /// Requires both `fs-index` and `fs-index-watch` Cargo features.
    #[serde(default = "default_false")]
    pub watch_enabled: bool,

    /// Snapshot freshness TTL in seconds. Default: 60.
    ///
    /// On expiry an incremental Zone B rebuild is triggered on the next lookup.
    #[serde(default = "default_60")]
    pub ttl_secs: u32,

    /// Maximum number of path entries in the snapshot. Default: 1 000 000.
    ///
    /// `0` means unbounded (not recommended; may exhaust process RSS).
    #[serde(default = "default_1_000_000")]
    pub max_entries: u32,

    /// Approximate memory ceiling for the snapshot in bytes (default 256 MiB).
    ///
    /// `0` means unbounded (not recommended).
    #[serde(default = "default_256_mib_u64")]
    pub max_bytes: u64,

    /// Polling interval in seconds for the `PollingWatcher` Null Object. Default: 30.
    ///
    /// Only active when `watch_enabled = true` but no kernel watcher is available.
    #[serde(default = "default_30")]
    pub poll_secs: u32,

    /// Per-root parallel rebuild cap during Zone B snapshot refresh. Default: 2.
    #[serde(default = "default_2")]
    pub rebuild_concurrency: u8,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            watch_enabled: false,
            ttl_secs: default_60(),
            max_entries: default_1_000_000(),
            max_bytes: default_256_mib_u64(),
            poll_secs: default_30(),
            rebuild_concurrency: default_2(),
        }
    }
}

// ---- CapabilitiesSection / SimdConfig ----------------------------------------

/// Operator-supplied capability tier overrides per ADR-0042.
///
/// Embedded under the `[capabilities]` TOML section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CapabilitiesSection {
    /// Force specific adapter tiers regardless of probe results.
    ///
    /// Invalid tier names abort startup with `SUBSTRATE_CONFIG_INVALID`.
    #[serde(default)]
    pub r#override: Option<CapabilityOverride>,
}

/// SIMD tier opt-in configuration per ADR-0043.
///
/// Embedded under the `[simd]` TOML section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SimdConfig {
    /// Enable AVX-512 when hardware is capable. Default: `false`.
    ///
    /// Set `true` only after confirming no frequency throttling on target hardware.
    #[serde(default = "default_false")]
    pub allow_avx512: bool,
}

// ---- Default value functions -------------------------------------------------

const fn default_true() -> bool {
    true
}
const fn default_false() -> bool {
    false
}
const fn default_5() -> u32 {
    5
}
const fn default_7() -> u32 {
    7
}
const fn default_30() -> u32 {
    30
}
const fn default_32() -> u32 {
    32
}
const fn default_50() -> u32 {
    50
}
const fn default_60() -> u32 {
    60
}
const fn default_256_u32() -> u32 {
    256
}
const fn default_500() -> u32 {
    500
}
const fn default_1024() -> u32 {
    1_024
}
const fn default_1_000_000() -> u32 {
    1_000_000
}
const fn default_2() -> u8 {
    2
}
const fn default_cpu_bound_max() -> u32 {
    4
}
const fn default_8_mib_u64() -> u64 {
    8 * 1_024 * 1_024
}
const fn default_1_mib_u64() -> u64 {
    1_024 * 1_024
}
const fn default_100_mib_u64() -> u64 {
    100 * 1_024 * 1_024
}
const fn default_256_mib_u64() -> u64 {
    256 * 1_024 * 1_024
}
const fn default_1_gib_u64() -> u64 {
    1_024 * 1_024 * 1_024
}
