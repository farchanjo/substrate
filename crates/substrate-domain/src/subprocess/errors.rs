//! Error taxonomy for the subprocess bounded context.
//!
//! Extends the base error catalog from ADR-0010 with subprocess-specific codes
//! introduced in ADR-0052. Every variant carries a stable `SUBSTRATE_*` code
//! string and a recovery hint capped at 150 characters per the CUE constraint
//! in `docs/arch/schemas/error_catalog.cue`.
//!
//! References: ADR-0052 §"New Error Codes", ADR-0010 §"Error taxonomy".

use std::io;

use crate::subprocess::state::SubprocessState;
use crate::subprocess::stream::Stream;
use crate::value_objects::JobId;

/// Errors emitted by the subprocess bounded context per ADR-0052.
///
/// Adapters translate these into MCP JSON-RPC error responses at the boundary.
/// Domain code MUST NOT construct `McpError` directly.
#[derive(Debug, thiserror::Error)]
pub enum SubprocessError {
    /// The requested binary path is not present in `security.subprocess_binary_allowlist`.
    ///
    /// Stable code: `SUBSTRATE_SUBPROCESS_BINARY_NOT_ALLOWED`.
    #[error("Binary not in subprocess allowlist: {path}")]
    BinaryNotAllowed {
        /// The binary path that was rejected.
        path: String,
    },

    /// An environment variable key is unconditionally banned (e.g., `LD_PRELOAD`).
    ///
    /// Stable code: `SUBSTRATE_SUBPROCESS_ENV_BANNED`.
    #[error("Environment variable is unconditionally banned: {var}")]
    EnvBanned {
        /// The banned variable name.
        var: String,
    },

    /// The `cwd` argument is outside all configured `allowed_paths` roots.
    ///
    /// Stable code: `SUBSTRATE_SUBPROCESS_CWD_OUTSIDE_ALLOWLIST`.
    #[error("Subprocess cwd is outside the configured allowlist: {path}")]
    CwdOutsideAllowlist {
        /// The offending cwd path.
        path: String,
    },

    /// Per-client or global subprocess quota reached.
    ///
    /// Stable code: `SUBSTRATE_SUBPROCESS_QUOTA_EXCEEDED`.
    #[error("Subprocess quota exceeded (limit {limit})")]
    QuotaExceeded {
        /// The quota ceiling that was hit.
        limit: u32,
    },

    /// `tokio::process::Command` returned an error during spawn.
    ///
    /// Stable code: `SUBSTRATE_SUBPROCESS_SPAWN_FAILED`.
    #[error("Failed to spawn subprocess: {source}")]
    SpawnFailed {
        /// The underlying OS I/O error.
        #[source]
        source: io::Error,
    },

    /// The child process exceeded its configured `timeout_secs` limit.
    ///
    /// Stable code: `SUBSTRATE_SUBPROCESS_TIMEOUT`.
    #[error("Subprocess timed out after {secs}s")]
    Timeout {
        /// The timeout value in seconds.
        secs: u32,
    },

    /// The child process was killed via `SIGKILL` by the cascade kill chain.
    ///
    /// Stable code: `SUBSTRATE_SUBPROCESS_KILLED`.
    #[error("Subprocess was killed by the cascade kill chain")]
    Killed,

    /// The tool requires elicitation confirmation that was not provided.
    ///
    /// Every `subprocess.spawn` call requires unconditional elicitation per ADR-0052
    /// §"Elicitation (mandatory for all spawns)".
    ///
    /// Stable code: `SUBSTRATE_ELICITATION_REQUIRED`.
    #[error("Elicitation required before spawning '{tool}'")]
    ElicitationRequired {
        /// The tool that requires confirmation.
        tool: String,
    },

    /// A stream chunk was dropped due to bounded mpsc channel backpressure.
    ///
    /// Stable code: `SUBSTRATE_STREAM_CHUNK_DROPPED`.
    #[error("Stream chunk dropped for job {job_id} on {stream}")]
    StreamChunkDropped {
        /// The job whose stream dropped a chunk.
        job_id: JobId,
        /// The stream (stdout or stderr) that dropped the chunk.
        stream: Stream,
    },

    /// A `SubprocessRequest` field fails semantic validation.
    ///
    /// Reuses `SUBSTRATE_INVALID_ARGUMENT` from the base error taxonomy (ADR-0010).
    #[error("Invalid subprocess request: {msg}")]
    InvalidRequest {
        /// Human-readable description of the validation failure.
        msg: String,
    },

    /// A state transition that violates the subprocess state machine was attempted.
    ///
    /// Stable code: `SUBSTRATE_INVALID_STATE_TRANSITION`.
    #[error("Invalid subprocess state transition: {from} -> {to}")]
    InvalidStateTransition {
        /// The current state.
        from: SubprocessState,
        /// The target state that was rejected.
        to: SubprocessState,
    },
}

impl SubprocessError {
    /// Returns the stable `SUBSTRATE_*` code string for this error variant.
    ///
    /// These strings are the authoritative identifiers used in JSON-RPC error
    /// responses, audit events, and monitoring dashboards.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::BinaryNotAllowed { .. } => "SUBSTRATE_SUBPROCESS_BINARY_NOT_ALLOWED",
            Self::EnvBanned { .. } => "SUBSTRATE_SUBPROCESS_ENV_BANNED",
            Self::CwdOutsideAllowlist { .. } => "SUBSTRATE_SUBPROCESS_CWD_OUTSIDE_ALLOWLIST",
            Self::QuotaExceeded { .. } => "SUBSTRATE_SUBPROCESS_QUOTA_EXCEEDED",
            Self::SpawnFailed { .. } => "SUBSTRATE_SUBPROCESS_SPAWN_FAILED",
            Self::Timeout { .. } => "SUBSTRATE_SUBPROCESS_TIMEOUT",
            Self::Killed => "SUBSTRATE_SUBPROCESS_KILLED",
            Self::ElicitationRequired { .. } => "SUBSTRATE_ELICITATION_REQUIRED",
            Self::StreamChunkDropped { .. } => "SUBSTRATE_STREAM_CHUNK_DROPPED",
            Self::InvalidRequest { .. } => "SUBSTRATE_INVALID_ARGUMENT",
            Self::InvalidStateTransition { .. } => "SUBSTRATE_INVALID_STATE_TRANSITION",
        }
    }

    /// Returns the operator-facing recovery hint for this error.
    ///
    /// All hints are <= 150 characters per the CUE schema constraint in
    /// `docs/arch/schemas/error_catalog.cue`.
    #[must_use]
    pub const fn recovery_hint(&self) -> &'static str {
        match self {
            Self::BinaryNotAllowed { .. } => {
                "Add the binary absolute path to security.subprocess_binary_allowlist in the server config."
            },
            Self::EnvBanned { .. } => {
                "Remove banned env var (LD_PRELOAD, DYLD_INSERT_LIBRARIES, LD_LIBRARY_PATH) from the request."
            },
            Self::CwdOutsideAllowlist { .. } => {
                "Specify a cwd path within a root listed in security_policy.allowlist.roots."
            },
            Self::QuotaExceeded { .. } => {
                "Wait for active subprocesses to complete or cancel an existing subprocess."
            },
            Self::SpawnFailed { .. } => {
                "Verify the binary exists and is executable at the configured path."
            },
            Self::Timeout { .. } => {
                "Increase timeout_secs in the subprocess.spawn request or break work into smaller units."
            },
            Self::Killed => {
                "The process did not respond to SIGTERM within cascade_drain_secs; review the binary."
            },
            Self::ElicitationRequired { .. } => {
                "Confirm the subprocess invocation via the MCP elicitation form before retrying."
            },
            Self::StreamChunkDropped { .. } => {
                "Slow down output or increase subprocess.aggregate_buffer_bytes to reduce drops."
            },
            Self::InvalidRequest { .. } => {
                "Consult the subprocess.spawn input_schema and correct the offending argument."
            },
            Self::InvalidStateTransition { .. } => {
                "Report the correlation_id; this is an internal state machine violation."
            },
        }
    }
}
