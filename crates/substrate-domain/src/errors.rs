//! Canonical error taxonomy for the substrate domain.
//!
//! All stable error codes mirror the `#ErrorCatalog` in
//! `docs/arch/schemas/error_catalog.cue` and the narrative in ADR-0010
//! (including amendments from ADR-0040 and ADR-0042).
//!
//! Each variant carries structured context that adapters use when building
//! JSON-RPC error responses. `recovery_hint` strings are capped at 150 chars
//! per the CUE schema constraint.

use uuid::Uuid;

/// Structured, stable errors emitted by the substrate domain layer.
///
/// Adapters translate these into MCP JSON-RPC error responses at the
/// boundary. Domain code MUST NOT construct `McpError` directly.
#[derive(Debug, thiserror::Error)]
pub enum SubstrateError {
    // ---- Security codes (-32001 through -32004) ----
    /// Requested path is not within any configured allowlist root.
    #[error("Path is outside the configured allowlist: {path}")]
    PathOutsideAllowlist {
        /// The offending path string (safe to surface — no OS internals).
        path: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// A `..` segment or encoded traversal sequence was detected in the path.
    #[error("Path traversal attempt blocked: {path}")]
    PathTraversalBlocked {
        /// The offending path string.
        path: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// Symlink resolution exits the allowlist boundary.
    #[error("Symlink escape detected: {path}")]
    SymlinkEscape {
        /// The offending path string.
        path: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// OS-level `EPERM` / `EACCES`.
    #[error("Permission denied accessing: {path}")]
    PermissionDenied {
        /// The resource that could not be accessed.
        path: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    // ---- Not-found code (-32005) ----
    /// The requested resource does not exist.
    #[error("Resource not found: {resource}")]
    NotFound {
        /// Description of the missing resource (no raw kernel paths in security contexts).
        resource: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    // ---- Lifecycle codes (-32006, -32007, -32010, -32011) ----
    /// Operation exceeded the configured deadline.
    #[error("Operation timed out after {elapsed_ms}ms")]
    Timeout {
        /// Elapsed duration at the point of cancellation.
        elapsed_ms: u64,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// Caller cancelled the request.
    #[error("Operation cancelled")]
    Cancelled {
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// Mutating tool was called without `dry_run: true` on first invocation.
    #[error("Dry run required before committing this operation")]
    DryRunRequired {
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// Destructive action awaiting explicit `confirmed: true` via elicitation.
    #[error("Explicit user confirmation is required")]
    ConfirmationRequired {
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    // ---- Resource code (-32008) ----
    /// Memory, file-descriptor, or process-count ceiling reached.
    #[error("Resource limit reached: {detail}")]
    ResourceLimit {
        /// Human-readable description of the limit that was hit.
        detail: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    // ---- Input code (-32009) ----
    /// An argument fails schema or semantic validation.
    #[error("Invalid argument '{offending_field}': {reason}")]
    InvalidArgument {
        /// The specific input field that caused the failure.
        offending_field: String,
        /// Human-readable reason for the rejection.
        reason: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    // ---- Protocol code (-32012) ----
    /// Client protocol version is outside the supported range.
    #[error("Protocol version unsupported: {version}")]
    ProtocolVersionUnsupported {
        /// The version string the client presented.
        version: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    // ---- Internal code (-32099) ----
    /// Unexpected server fault; see correlation ID in structured logs.
    #[error("Internal error: {reason}")]
    InternalError {
        /// Brief description safe for external surfacing.
        reason: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    // ---- Kernel-induced codes (-32014 through -32019) per ADR-0034 ----
    /// `ELOOP` — symlink chain exceeds the OS resolution limit.
    #[error("Symlink loop detected: {path}")]
    SymlinkLoop {
        /// Path where the loop was detected.
        path: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// `EIO` — hardware I/O failure or bad sector.
    #[error("I/O error accessing: {path}")]
    IoError {
        /// Path or resource that triggered the I/O error.
        path: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// `ENOSPC` / `EDQUOT` — disk full or quota exceeded.
    #[error("Storage full while writing: {path}")]
    StorageFull {
        /// Target path that ran out of space.
        path: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// `EROFS` — filesystem is mounted read-only.
    #[error("Filesystem is read-only: {path}")]
    ReadOnlyFs {
        /// Path on the read-only filesystem.
        path: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// Non-UTF-8 bytes in path or string content.
    #[error("Encoding error: {detail}")]
    EncodingError {
        /// Context for where the encoding failure occurred.
        detail: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// `EBUSY` / `ESTALE` / `EAGAIN` — transient resource unavailability.
    #[error("Transient I/O error: {detail}")]
    TransientIo {
        /// Brief description of the transient condition.
        detail: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    // ---- Startup codes (-32020 through -32026) per ADR-0036 ----
    /// Runtime configuration file is syntactically or semantically invalid.
    #[error("Configuration invalid: {offending_field}")]
    ConfigInvalid {
        /// The configuration field or section that is invalid.
        offending_field: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// Runtime configuration file was not found at the expected path.
    #[error("Configuration file not found")]
    ConfigNotFound {
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// An allowlist root directory does not exist on disk.
    #[error("Allowlist root missing: {root}")]
    AllowlistRootMissing {
        /// The missing allowlist root path.
        root: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// An allowlist root directory cannot be read by the server process.
    #[error("Allowlist root unreadable: {root}")]
    AllowlistRootUnreadable {
        /// The unreadable allowlist root path.
        root: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// The tokio runtime or a critical subsystem failed to initialise.
    #[error("Runtime initialisation failed: {reason}")]
    RuntimeInitFailed {
        /// Brief description of the failure.
        reason: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// The process file-descriptor limit is too low for safe operation.
    #[error("File-descriptor limit too low: current={current}")]
    FdLimitTooLow {
        /// The current `RLIMIT_NOFILE` value observed at startup.
        current: u64,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// The OS or architecture combination is not supported.
    #[error("Unsupported platform: {platform}")]
    UnsupportedPlatform {
        /// Brief description of the unsupported platform.
        platform: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    // ---- Async-job BC codes (-32027 through -32031) per ADR-0040 ----
    /// The requested job does not exist (expired or never created).
    #[error("Job not found: {job_id}")]
    JobNotFound {
        /// The `job_id` that was not found.
        job_id: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// A per-client, per-tool, or global concurrent-job quota was exceeded.
    #[error("Quota exceeded: {detail}")]
    QuotaExceeded {
        /// Description of the quota that was hit.
        detail: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// The job reached the `cancelled` terminal state.
    #[error("Job was cancelled: {job_id}")]
    JobCancelled {
        /// The `job_id` of the cancelled job.
        job_id: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// The job exceeded its configured `jobs.timeout.<tool>_secs` limit.
    #[error("Job timed out: {job_id}")]
    JobTimedOut {
        /// The `job_id` of the timed-out job.
        job_id: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// The `wait_ms` parameter in `job.result` exceeded `jobs.result_max_wait_ms`.
    #[error("Result wait exceeded: requested {requested_ms}ms, cap {cap_ms}ms")]
    ResultWaitExceeded {
        /// The `wait_ms` value the caller requested.
        requested_ms: u64,
        /// The server-side cap in milliseconds.
        cap_ms: u64,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    // ---- Capability-startup codes (-32032, -32033) per ADR-0042 + ADR-0035 amendment ----
    /// A `capabilities.override.<port>` entry names a tier that does not exist.
    #[error("Tier override invalid for port '{port}': '{tier}'")]
    TierOverrideInvalid {
        /// The port whose override is invalid (e.g., `walker`, `hash`).
        port: String,
        /// The tier name that was rejected.
        tier: String,
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },

    /// `PathJail` fell below the kernel-enforced tier and `refuse_degraded_jail = true`.
    #[error("Path jail degraded tier refused")]
    JailDegradedRefused {
        /// Optional correlation identifier for log linkage.
        correlation_id: Option<Uuid>,
    },
}

impl SubstrateError {
    /// Returns the stable `SUBSTRATE_*` code string for this error variant.
    ///
    /// These strings are the authoritative identifiers used in JSON-RPC error
    /// responses, audit events, and monitoring dashboards. Never remove or rename
    /// a published code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::PathOutsideAllowlist { .. } => "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST",
            Self::PathTraversalBlocked { .. } => "SUBSTRATE_PATH_TRAVERSAL_BLOCKED",
            Self::SymlinkEscape { .. } => "SUBSTRATE_SYMLINK_ESCAPE",
            Self::PermissionDenied { .. } => "SUBSTRATE_PERMISSION_DENIED",
            Self::NotFound { .. } => "SUBSTRATE_NOT_FOUND",
            Self::Timeout { .. } => "SUBSTRATE_TIMEOUT",
            Self::Cancelled { .. } => "SUBSTRATE_CANCELLED",
            Self::DryRunRequired { .. } => "SUBSTRATE_DRY_RUN_REQUIRED",
            Self::ConfirmationRequired { .. } => "SUBSTRATE_CONFIRMATION_REQUIRED",
            Self::ResourceLimit { .. } => "SUBSTRATE_RESOURCE_LIMIT",
            Self::InvalidArgument { .. } => "SUBSTRATE_INVALID_ARGUMENT",
            Self::ProtocolVersionUnsupported { .. } => "SUBSTRATE_PROTOCOL_VERSION_UNSUPPORTED",
            Self::InternalError { .. } => "SUBSTRATE_INTERNAL_ERROR",
            Self::SymlinkLoop { .. } => "SUBSTRATE_SYMLINK_LOOP",
            Self::IoError { .. } => "SUBSTRATE_IO_ERROR",
            Self::StorageFull { .. } => "SUBSTRATE_STORAGE_FULL",
            Self::ReadOnlyFs { .. } => "SUBSTRATE_READ_ONLY_FS",
            Self::EncodingError { .. } => "SUBSTRATE_ENCODING_ERROR",
            Self::TransientIo { .. } => "SUBSTRATE_TRANSIENT_IO",
            Self::ConfigInvalid { .. } => "SUBSTRATE_CONFIG_INVALID",
            Self::ConfigNotFound { .. } => "SUBSTRATE_CONFIG_NOT_FOUND",
            Self::AllowlistRootMissing { .. } => "SUBSTRATE_ALLOWLIST_ROOT_MISSING",
            Self::AllowlistRootUnreadable { .. } => "SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE",
            Self::RuntimeInitFailed { .. } => "SUBSTRATE_RUNTIME_INIT_FAILED",
            Self::FdLimitTooLow { .. } => "SUBSTRATE_FD_LIMIT_TOO_LOW",
            Self::UnsupportedPlatform { .. } => "SUBSTRATE_UNSUPPORTED_PLATFORM",
            Self::JobNotFound { .. } => "SUBSTRATE_JOB_NOT_FOUND",
            Self::QuotaExceeded { .. } => "SUBSTRATE_QUOTA_EXCEEDED",
            Self::JobCancelled { .. } => "SUBSTRATE_JOB_CANCELLED",
            Self::JobTimedOut { .. } => "SUBSTRATE_JOB_TIMED_OUT",
            Self::ResultWaitExceeded { .. } => "SUBSTRATE_RESULT_WAIT_EXCEEDED",
            Self::TierOverrideInvalid { .. } => "SUBSTRATE_TIER_OVERRIDE_INVALID",
            Self::JailDegradedRefused { .. } => "SUBSTRATE_JAIL_DEGRADED_REFUSED",
        }
    }

    /// Returns the operator-facing recovery hint for this error.
    ///
    /// All hints are ≤ 150 characters per the CUE schema constraint in
    /// `docs/arch/schemas/error_catalog.cue`.
    #[must_use]
    pub const fn recovery_hint(&self) -> &'static str {
        match self {
            Self::PathOutsideAllowlist { .. } => {
                "Add the requested path root to security_policy.allowlist.roots."
            },
            Self::PathTraversalBlocked { .. } => {
                "Remove '..' segments or absolute escape sequences from the path argument."
            },
            Self::SymlinkEscape { .. } => {
                "Resolve the symlink target and verify it stays within an allowed root."
            },
            Self::PermissionDenied { .. } => {
                "Check OS file permissions or adjust security_policy for this tool."
            },
            Self::NotFound { .. } => {
                "Verify the path or resource identifier exists before calling."
            },
            Self::Timeout { .. } => {
                "Increase timeouts.per_tool for this tool or break the operation into smaller chunks."
            },
            Self::Cancelled { .. } => {
                "Retry the call; cancellation originated from the client or a signal."
            },
            Self::DryRunRequired { .. } => {
                "Call the tool first with dry_run=true to preview changes, then re-submit."
            },
            Self::ConfirmationRequired { .. } => {
                "Obtain explicit user confirmation via the elicitation flow, then retry."
            },
            Self::ResourceLimit { .. } => {
                "Reduce payload size or increase semaphore_caps / buffer limits in runtime_config."
            },
            Self::InvalidArgument { .. } => {
                "Consult the tool input_schema and correct the offending argument."
            },
            Self::ProtocolVersionUnsupported { .. } => {
                "Negotiate a supported protocol version during the MCP initialize handshake."
            },
            Self::InternalError { .. } => {
                "Report the correlation_id from the audit log to the substrate maintainers."
            },
            Self::SymlinkLoop { .. } => {
                "Remove circular symlink chains in the target directory tree before retrying."
            },
            Self::IoError { .. } => {
                "Check kernel dmesg and filesystem health; retry after resolving hardware issues."
            },
            Self::StorageFull { .. } => {
                "Free disk space on the target volume and retry the operation."
            },
            Self::ReadOnlyFs { .. } => {
                "Remount the filesystem read-write or redirect writes to a writable path."
            },
            Self::EncodingError { .. } => {
                "Ensure the file is valid UTF-8 or specify the correct encoding in the request."
            },
            Self::TransientIo { .. } => {
                "Retry the operation; transient I/O errors typically resolve on subsequent attempts."
            },
            Self::ConfigInvalid { .. } => {
                "Fix the runtime_config field reported in offending_field and restart the server."
            },
            Self::ConfigNotFound { .. } => {
                "Create or mount the runtime configuration file at the expected path and restart."
            },
            Self::AllowlistRootMissing { .. } => {
                "Create the allowlist root directory or remove the missing entry from the policy."
            },
            Self::AllowlistRootUnreadable { .. } => {
                "Grant read permission to the substrate process on the configured allowlist root."
            },
            Self::RuntimeInitFailed { .. } => {
                "Check server logs for the root cause; verify system resources and restart."
            },
            Self::FdLimitTooLow { .. } => {
                "Increase the process file-descriptor limit (ulimit -n) to at least 1024 and restart."
            },
            Self::UnsupportedPlatform { .. } => {
                "Run substrate on a supported OS/architecture; consult the compatibility matrix."
            },
            Self::JobNotFound { .. } => "Verify job_id; expired jobs cannot be recovered.",
            Self::QuotaExceeded { .. } => {
                "Wait for active jobs to complete or cancel an existing job."
            },
            Self::JobCancelled { .. } => "Retry the operation if cancellation was unintended.",
            Self::JobTimedOut { .. } => "Increase timeout or split the work into smaller units.",
            Self::ResultWaitExceeded { .. } => "Retry with a smaller wait_ms.",
            Self::TierOverrideInvalid { .. } => {
                "Review capabilities.override config and use a valid tier name for this port."
            },
            Self::JailDegradedRefused { .. } => {
                "Upgrade kernel to >= 5.6 (Linux) or macOS >= 11, or set security.refuse_degraded_jail = false."
            },
        }
    }

    /// Returns the optional correlation ID attached to this error.
    #[must_use]
    pub const fn correlation_id(&self) -> Option<Uuid> {
        match self {
            Self::PathOutsideAllowlist { correlation_id, .. }
            | Self::PathTraversalBlocked { correlation_id, .. }
            | Self::SymlinkEscape { correlation_id, .. }
            | Self::PermissionDenied { correlation_id, .. }
            | Self::NotFound { correlation_id, .. }
            | Self::Timeout { correlation_id, .. }
            | Self::Cancelled { correlation_id }
            | Self::DryRunRequired { correlation_id }
            | Self::ConfirmationRequired { correlation_id }
            | Self::ResourceLimit { correlation_id, .. }
            | Self::InvalidArgument { correlation_id, .. }
            | Self::ProtocolVersionUnsupported { correlation_id, .. }
            | Self::InternalError { correlation_id, .. }
            | Self::SymlinkLoop { correlation_id, .. }
            | Self::IoError { correlation_id, .. }
            | Self::StorageFull { correlation_id, .. }
            | Self::ReadOnlyFs { correlation_id, .. }
            | Self::EncodingError { correlation_id, .. }
            | Self::TransientIo { correlation_id, .. }
            | Self::ConfigInvalid { correlation_id, .. }
            | Self::ConfigNotFound { correlation_id }
            | Self::AllowlistRootMissing { correlation_id, .. }
            | Self::AllowlistRootUnreadable { correlation_id, .. }
            | Self::RuntimeInitFailed { correlation_id, .. }
            | Self::FdLimitTooLow { correlation_id, .. }
            | Self::UnsupportedPlatform { correlation_id, .. }
            | Self::JobNotFound { correlation_id, .. }
            | Self::QuotaExceeded { correlation_id, .. }
            | Self::JobCancelled { correlation_id, .. }
            | Self::JobTimedOut { correlation_id, .. }
            | Self::ResultWaitExceeded { correlation_id, .. }
            | Self::TierOverrideInvalid { correlation_id, .. }
            | Self::JailDegradedRefused { correlation_id } => *correlation_id,
        }
    }
}

/// Convenience alias for `Result<T, SubstrateError>`.
pub type SubstrateResult<T> = Result<T, SubstrateError>;
