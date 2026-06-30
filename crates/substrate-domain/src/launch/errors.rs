//! Error taxonomy for the launch bounded context.
//!
//! Extends the base error catalog (ADR-0010) with launch-specific codes
//! introduced in ADR-0063..0069 (`SUBSTRATE_LAUNCH_*`, JSON-RPC -32044..-32056).
//! Every variant carries a stable `SUBSTRATE_*` code string and a recovery hint
//! capped at 150 characters per the CUE constraint in
//! `docs/arch/schemas/error_catalog.cue`. The hints below are copied verbatim
//! from that catalog.
//!
//! References: ADR-0063, ADR-0064, ADR-0065, ADR-0066, ADR-0068.

use std::io;

/// Errors emitted by the launch bounded context per ADR-0063..0069.
///
/// Adapters translate these into MCP JSON-RPC error responses at the boundary
/// (mapping each `code()` to its numeric `-32044..-32056` JSON-RPC code via the
/// error catalog). Domain code MUST NOT construct transport errors directly.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    /// The Profile's inode/content tuple is not present in the trust store.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED` (-32044).
    #[error("Profile is not trusted: {path}")]
    ProfileNotTrusted {
        /// The canonical Profile path that failed the trust check.
        path: String,
    },

    /// The `.substrate.toml` path resolved to a symlink, which is rejected.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_CONFIG_SYMLINK_REJECTED` (-32045).
    #[error("Config path is a symlink: {path}")]
    ConfigSymlinkRejected {
        /// The offending symlinked path.
        path: String,
    },

    /// The config's parent directory is world-writable or not owned by the caller.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_CONFIG_UNTRUSTED_DIR` (-32046).
    #[error("Config parent directory is untrusted: {path}")]
    ConfigUntrustedDir {
        /// The offending directory path.
        path: String,
    },

    /// The trust store file or its directory has insecure permissions.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_TRUST_STORE_INSECURE` (-32047).
    #[error("Trust store has insecure permissions: {path}")]
    TrustStoreInsecure {
        /// The trust store path with the loose permissions.
        path: String,
    },

    /// The `depends_on` edges form a cycle (no valid topological order).
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_CYCLE_DETECTED` (-32048).
    #[error("Dependency cycle detected among services: {nodes:?}")]
    CycleDetected {
        /// The Services that remain unresolved (members of the cycle).
        nodes: Vec<String>,
    },

    /// A required dependency failed to reach `Ready` within its probe budget.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_DEPENDENCY_FAILED` (-32049).
    #[error("Service '{service}' dependency '{dependency}' failed readiness")]
    DependencyFailed {
        /// The dependent Service that could not start.
        service: String,
        /// The dependency that failed readiness.
        dependency: String,
    },

    /// A previously detached orphan process was reaped on startup.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_ORPHAN_REAPED` (-32050).
    #[error("Orphaned process '{name}' was reaped on boot")]
    OrphanReaped {
        /// The Service name of the reaped orphan.
        name: String,
    },

    /// A previously detached orphan process was re-adopted on startup.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_ORPHAN_ADOPTED` (-32051).
    #[error("Orphaned process '{name}' was adopted on boot")]
    OrphanAdopted {
        /// The Service name of the adopted orphan.
        name: String,
    },

    /// A detached Stack exceeded `orphan_ttl_secs` and was brought down.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_STACK_TTL_EXPIRED` (-32052).
    #[error("Detached stack '{stack_id}' exceeded its orphan TTL")]
    StackTtlExpired {
        /// The Stack id whose TTL expired.
        stack_id: String,
    },

    /// The detached supervisor is not responding.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_SUPERVISOR_UNREACHABLE` (-32053).
    #[error("Detached supervisor for stack '{stack_id}' is unreachable")]
    SupervisorUnreachable {
        /// The Stack id whose supervisor is unreachable.
        stack_id: String,
    },

    /// The supervisor registry directory or control FIFO has insecure permissions.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_REGISTRY_INSECURE` (-32054).
    #[error("Supervisor registry has insecure permissions: {path}")]
    RegistryInsecure {
        /// The registry path with the loose permissions.
        path: String,
    },

    /// A control-FIFO command frame exceeded the atomic-write framing limit.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_FRAME_TOO_LARGE` (-32055).
    #[error("Control frame too large: {size} bytes")]
    FrameTooLarge {
        /// The rejected frame size in bytes.
        size: usize,
    },

    /// A recorded child's pid was recycled to a different process.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_CHILD_PID_RECYCLED` (-32056).
    #[error("Child '{name}' pid {pid} was recycled")]
    ChildPidRecycled {
        /// The Service name of the recycled child.
        name: String,
        /// The pid that was found to belong to a different process.
        pid: i32,
    },

    /// A `LaunchProfile` field fails structural validation.
    ///
    /// Reuses `SUBSTRATE_INVALID_ARGUMENT` from the base error taxonomy (ADR-0010).
    #[error("Invalid launch profile: {msg}")]
    InvalidProfile {
        /// Human-readable description of the validation failure.
        msg: String,
    },

    /// The launch orchestrator failed to spawn a Service via the subprocess port.
    ///
    /// Stable code: `SUBSTRATE_LAUNCH_SPAWN_FAILED`.
    #[error("Failed to spawn launch service: {source}")]
    SpawnFailed {
        /// The underlying OS I/O error surfaced by the subprocess adapter.
        #[source]
        source: io::Error,
    },
}

impl LaunchError {
    /// Returns the stable `SUBSTRATE_*` code string for this error variant.
    ///
    /// These strings are the authoritative identifiers mapped to numeric JSON-RPC
    /// codes (`-32044..-32056`) at the MCP boundary via the error catalog.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ProfileNotTrusted { .. } => "SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED",
            Self::ConfigSymlinkRejected { .. } => "SUBSTRATE_LAUNCH_CONFIG_SYMLINK_REJECTED",
            Self::ConfigUntrustedDir { .. } => "SUBSTRATE_LAUNCH_CONFIG_UNTRUSTED_DIR",
            Self::TrustStoreInsecure { .. } => "SUBSTRATE_LAUNCH_TRUST_STORE_INSECURE",
            Self::CycleDetected { .. } => "SUBSTRATE_LAUNCH_CYCLE_DETECTED",
            Self::DependencyFailed { .. } => "SUBSTRATE_LAUNCH_DEPENDENCY_FAILED",
            Self::OrphanReaped { .. } => "SUBSTRATE_LAUNCH_ORPHAN_REAPED",
            Self::OrphanAdopted { .. } => "SUBSTRATE_LAUNCH_ORPHAN_ADOPTED",
            Self::StackTtlExpired { .. } => "SUBSTRATE_LAUNCH_STACK_TTL_EXPIRED",
            Self::SupervisorUnreachable { .. } => "SUBSTRATE_LAUNCH_SUPERVISOR_UNREACHABLE",
            Self::RegistryInsecure { .. } => "SUBSTRATE_LAUNCH_REGISTRY_INSECURE",
            Self::FrameTooLarge { .. } => "SUBSTRATE_LAUNCH_FRAME_TOO_LARGE",
            Self::ChildPidRecycled { .. } => "SUBSTRATE_LAUNCH_CHILD_PID_RECYCLED",
            Self::InvalidProfile { .. } => "SUBSTRATE_INVALID_ARGUMENT",
            Self::SpawnFailed { .. } => "SUBSTRATE_LAUNCH_SPAWN_FAILED",
        }
    }

    /// Returns the operator-facing recovery hint for this error.
    ///
    /// All hints are `<= 150` characters per the CUE schema constraint in
    /// `docs/arch/schemas/error_catalog.cue`; the launch-BC hints are copied
    /// verbatim from that catalog.
    #[must_use]
    pub const fn recovery_hint(&self) -> &'static str {
        match self {
            Self::ProfileNotTrusted { .. } => {
                "Run launch.trust to bless this .substrate.toml after reviewing it; its inode/content tuple is not in the trust store."
            },
            Self::ConfigSymlinkRejected { .. } => {
                "The .substrate.toml path is a symlink; replace it with a regular file. Symlinked config is rejected per ADR-0064."
            },
            Self::ConfigUntrustedDir { .. } => {
                "The config's parent directory is world-writable or not owned by you; fix its ownership and permissions before launch.up."
            },
            Self::TrustStoreInsecure { .. } => {
                "Set ~/.config/substrate to mode 0700 and trust.toml to 0600 owned by you; the trust store permissions are too loose."
            },
            Self::CycleDetected { .. } => {
                "Remove the dependency cycle from depends_on in .substrate.toml; run launch.list to inspect the graph before retrying."
            },
            Self::DependencyFailed { .. } => {
                "Check launch.status for the failed dependency; fix its readiness probe or set required=false to make it optional."
            },
            Self::OrphanReaped { .. } => {
                "A previously detached process was reaped on startup; re-run launch.up to restart the stack."
            },
            Self::OrphanAdopted { .. } => {
                "A detached process was re-adopted on startup; use launch.status to inspect it."
            },
            Self::StackTtlExpired { .. } => {
                "The detached stack exceeded launch.orphan_ttl_secs without a client; re-run launch.up to restart it."
            },
            Self::SupervisorUnreachable { .. } => {
                "The detached supervisor is not responding; run launch.status to trigger reaper-on-boot."
            },
            Self::RegistryInsecure { .. } => {
                "Set the launch stacks dir to 0700 and control.fifo to 0600 owned by you, with no world-writable ancestor; then retry."
            },
            Self::FrameTooLarge { .. } => {
                "The control-FIFO command frame exceeds PIPE_BUF-1 and was rejected to preserve atomic framing; send a smaller command."
            },
            Self::ChildPidRecycled { .. } => {
                "A recorded child's pid was recycled to another process; the stale entry was cleared with no signal sent. Re-run launch.up."
            },
            Self::InvalidProfile { .. } => {
                "Consult the .substrate.toml schema; commands must be argv arrays, version >= 1, ttl in 0..86400, and depends_on must form a DAG."
            },
            Self::SpawnFailed { .. } => {
                "The launch supervisor could not spawn a service via the subprocess port; verify the binary exists and is in the allowlist."
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant, used to drive exhaustive code/hint checks.
    fn all_variants() -> Vec<LaunchError> {
        vec![
            LaunchError::ProfileNotTrusted { path: "p".to_owned() },
            LaunchError::ConfigSymlinkRejected { path: "p".to_owned() },
            LaunchError::ConfigUntrustedDir { path: "p".to_owned() },
            LaunchError::TrustStoreInsecure { path: "p".to_owned() },
            LaunchError::CycleDetected { nodes: vec!["a".to_owned()] },
            LaunchError::DependencyFailed {
                service: "s".to_owned(),
                dependency: "d".to_owned(),
            },
            LaunchError::OrphanReaped { name: "n".to_owned() },
            LaunchError::OrphanAdopted { name: "n".to_owned() },
            LaunchError::StackTtlExpired { stack_id: "s".to_owned() },
            LaunchError::SupervisorUnreachable { stack_id: "s".to_owned() },
            LaunchError::RegistryInsecure { path: "p".to_owned() },
            LaunchError::FrameTooLarge { size: 99 },
            LaunchError::ChildPidRecycled { name: "n".to_owned(), pid: 42 },
            LaunchError::InvalidProfile { msg: "m".to_owned() },
            LaunchError::SpawnFailed {
                source: io::Error::new(io::ErrorKind::NotFound, "x"),
            },
        ]
    }

    #[test]
    fn codes_match_expected_literals() {
        let pairs = [
            (
                LaunchError::ProfileNotTrusted { path: "p".to_owned() },
                "SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED",
            ),
            (
                LaunchError::ConfigSymlinkRejected { path: "p".to_owned() },
                "SUBSTRATE_LAUNCH_CONFIG_SYMLINK_REJECTED",
            ),
            (
                LaunchError::ConfigUntrustedDir { path: "p".to_owned() },
                "SUBSTRATE_LAUNCH_CONFIG_UNTRUSTED_DIR",
            ),
            (
                LaunchError::TrustStoreInsecure { path: "p".to_owned() },
                "SUBSTRATE_LAUNCH_TRUST_STORE_INSECURE",
            ),
            (
                LaunchError::CycleDetected { nodes: vec![] },
                "SUBSTRATE_LAUNCH_CYCLE_DETECTED",
            ),
            (
                LaunchError::DependencyFailed {
                    service: "s".to_owned(),
                    dependency: "d".to_owned(),
                },
                "SUBSTRATE_LAUNCH_DEPENDENCY_FAILED",
            ),
            (
                LaunchError::OrphanReaped { name: "n".to_owned() },
                "SUBSTRATE_LAUNCH_ORPHAN_REAPED",
            ),
            (
                LaunchError::OrphanAdopted { name: "n".to_owned() },
                "SUBSTRATE_LAUNCH_ORPHAN_ADOPTED",
            ),
            (
                LaunchError::StackTtlExpired { stack_id: "s".to_owned() },
                "SUBSTRATE_LAUNCH_STACK_TTL_EXPIRED",
            ),
            (
                LaunchError::SupervisorUnreachable { stack_id: "s".to_owned() },
                "SUBSTRATE_LAUNCH_SUPERVISOR_UNREACHABLE",
            ),
            (
                LaunchError::RegistryInsecure { path: "p".to_owned() },
                "SUBSTRATE_LAUNCH_REGISTRY_INSECURE",
            ),
            (
                LaunchError::FrameTooLarge { size: 1 },
                "SUBSTRATE_LAUNCH_FRAME_TOO_LARGE",
            ),
            (
                LaunchError::ChildPidRecycled { name: "n".to_owned(), pid: 1 },
                "SUBSTRATE_LAUNCH_CHILD_PID_RECYCLED",
            ),
            (
                LaunchError::InvalidProfile { msg: "m".to_owned() },
                "SUBSTRATE_INVALID_ARGUMENT",
            ),
            (
                LaunchError::SpawnFailed {
                    source: io::Error::new(io::ErrorKind::NotFound, "x"),
                },
                "SUBSTRATE_LAUNCH_SPAWN_FAILED",
            ),
        ];
        for (err, expected) in pairs {
            assert_eq!(err.code(), expected);
        }
    }

    #[test]
    fn every_recovery_hint_within_150_chars() {
        for err in all_variants() {
            let hint = err.recovery_hint();
            assert!(
                hint.len() <= 150,
                "hint for {} is {} chars (> 150): {hint}",
                err.code(),
                hint.len()
            );
        }
    }
}
