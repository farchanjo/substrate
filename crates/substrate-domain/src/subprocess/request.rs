//! `SubprocessRequest` — value object for a subprocess spawn invocation.
//!
//! Mirrors `#SubprocessRequest` in `docs/arch/schemas/subprocess.cue`.
//! All fields are validated by `SubprocessRequest::validate` before any OS call
//! is made; the same invariants are enforced by `subprocess_invariants.rego`.
//!
//! References: ADR-0052 §"`SubprocessRequest`", ADR-0004 §"Security Model".

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::subprocess::errors::SubprocessError;
use crate::subprocess::supervisor::{HealthProbe, LogRotation, RestartPolicy};
use crate::value_objects::IdempotencyKey;

/// Unconditionally banned environment variable keys per ADR-0052 §"Layer 5".
///
/// These keys are injection vectors that could compromise the host OS regardless
/// of the operator's `subprocess_env_allowlist` configuration. They are rejected
/// both in `env_allowlist` (inheritance) and `env_override` (explicit setting).
const BANNED_ENV_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
];

/// Maximum permitted `timeout_secs` value per the CUE schema constraint.
const MAX_TIMEOUT_SECS: u32 = 86_400;

/// Minimum permitted `timeout_secs` value per the CUE schema constraint.
const MIN_TIMEOUT_SECS: u32 = 1;

/// Controls how the child process receives standard input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StdinKind {
    /// Child's stdin is closed (`/dev/null`). Default and most secure.
    None,
    /// Child's stdin is connected to a pipe that the caller can write to.
    Piped,
    /// Child's stdin is redirected from a pre-existing file.
    /// Requires `stdin_file_path` to be set.
    FilePath(PathBuf),
}

/// Controls how stdout and stderr are captured from the child process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    /// Bytes are emitted chunk-by-chunk via `notifications/progress` per ADR-0054.
    Stream,
    /// All output is buffered in memory and returned via `subprocess.result`.
    InMemory,
    /// Output is spilled to a temporary file registered in `SubprocessHandle.tmp_files`.
    TmpFile,
}

/// Value object submitted by an MCP client to launch a child process.
///
/// All fields are validated by [`SubprocessRequest::validate`] before any OS
/// call is made. The caller MUST call `validate` and check for
/// `elicitation_confirmed = true` before passing this to `SubprocessPort::spawn`.
///
/// See `docs/arch/schemas/subprocess.cue #SubprocessRequest` and ADR-0052.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubprocessRequest {
    /// Absolute path to the executable to spawn.
    ///
    /// Must begin with `/` and be present in `security.subprocess_binary_allowlist`.
    pub binary_path: PathBuf,

    /// Argument list passed to the binary (`argv[1..]`).
    pub args: Vec<String>,

    /// Names (not values) of parent-environment variables the child may inherit.
    ///
    /// Only keys listed here are forwarded; values are taken from the substrate
    /// process environment at spawn time. Banned keys are unconditionally stripped
    /// regardless of this list.
    pub env_allowlist: Vec<String>,

    /// Explicit key=value environment overrides in the child environment.
    ///
    /// Banned keys (`LD_PRELOAD` etc.) are rejected at validation time.
    pub env_override: BTreeMap<String, String>,

    /// Working directory for the child process.
    ///
    /// Must be an absolute path validated by `PathJail`.
    pub cwd: PathBuf,

    /// How stdin is supplied to the child.
    pub stdin_kind: StdinKind,

    /// How stdout and stderr are captured.
    pub capture_kind: CaptureKind,

    /// Maximum lifetime of the child process in seconds.
    ///
    /// When the child has not exited within this window the cascade kill chain
    /// is triggered and the state transitions to `TimedOut`. Range: 1..=86400.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u32>,

    /// Client-generated deduplication token per ADR-0040.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<IdempotencyKey>,

    /// Set to `true` when the caller has confirmed the elicitation form.
    ///
    /// Every `subprocess.spawn` requires unconditional human confirmation per
    /// ADR-0052 §"Elicitation (mandatory for all spawns)". The handler MUST
    /// emit an elicitation form and gate spawn on this field being `true`.
    pub elicitation_confirmed: bool,

    /// Operator-supplied alias scoped to `(client_id, name)` per ADR-0056.
    ///
    /// Enables idempotent re-spawn: if a non-terminal job already exists under
    /// this `(client_id, name)` pair, `subprocess.spawn` returns the existing
    /// `job_id` without starting a new process. Format: `^[a-z0-9-]{1,64}$`.
    /// Default: `None` (server assigns a `job_id` as in ADR-0052).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Restart policy per ADR-0056. Default: `Never` (one-shot behavior preserved).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_policy: Option<RestartPolicy>,

    /// Health probe gating the `Starting` -> `Ready` transition per ADR-0056.
    /// Default: `None` (no probe; `Running == Ready` immediately).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_probe: Option<HealthProbe>,

    /// Log rotation for `capture_kind = TmpFile` per ADR-0056.
    /// Default: `None` (tmp file grows unbounded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_rotation: Option<LogRotation>,
}

impl SubprocessRequest {
    /// Validates all fields of this request against the invariants specified in
    /// `docs/arch/policies/subprocess_invariants.rego` and ADR-0052.
    ///
    /// Validation is purely in-memory (no OS calls). Callers must pass this
    /// through `PathJail` separately for the Layer 2 jailing check.
    ///
    /// # Errors
    ///
    /// Returns the first validation failure encountered as a `SubprocessError`.
    pub fn validate(&self) -> Result<(), SubprocessError> {
        // binary_path must be absolute.
        if !self.binary_path.is_absolute() {
            return Err(SubprocessError::InvalidRequest {
                msg: format!(
                    "binary_path must be absolute; got '{}'",
                    self.binary_path.display()
                ),
            });
        }

        // cwd must be absolute.
        if !self.cwd.is_absolute() {
            return Err(SubprocessError::InvalidRequest {
                msg: format!("cwd must be absolute; got '{}'", self.cwd.display()),
            });
        }

        // Check env_allowlist for banned keys.
        for key in &self.env_allowlist {
            if BANNED_ENV_VARS.contains(&key.as_str()) {
                return Err(SubprocessError::EnvBanned { var: key.clone() });
            }
        }

        // Check env_override for banned keys.
        for key in self.env_override.keys() {
            if BANNED_ENV_VARS.contains(&key.as_str()) {
                return Err(SubprocessError::EnvBanned { var: key.clone() });
            }
        }

        // stdin_file_path is required iff stdin_kind is FilePath.
        // (FilePath carries the path directly in the enum variant — always present.)

        // Validate timeout_secs range.
        if let Some(secs) = self.timeout_secs
            && !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&secs)
        {
            return Err(SubprocessError::InvalidRequest {
                msg: format!(
                    "timeout_secs must be in range {MIN_TIMEOUT_SECS}..={MAX_TIMEOUT_SECS}; got {secs}"
                ),
            });
        }

        // Elicitation confirmation check: informational only at validate() time;
        // the port implementation must also check this before spawning.
        if !self.elicitation_confirmed {
            return Err(SubprocessError::ElicitationRequired {
                tool: "subprocess.spawn".to_owned(),
            });
        }

        // Validate `name` format: ^[a-z0-9-]{1,64}$
        // The regex crate is not a substrate-domain dependency (zero infra deps rule
        // per ADR-0022); validate inline with a hand-rolled ASCII predicate.
        if let Some(name) = &self.name {
            if name.is_empty() || name.len() > 64 {
                return Err(SubprocessError::InvalidRequest {
                    msg: format!("name must be 1..=64 characters; got length {}", name.len()),
                });
            }
            if !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                return Err(SubprocessError::InvalidRequest {
                    msg: format!("name must match ^[a-z0-9-]{{1,64}}$; got '{name}'"),
                });
            }
        }

        // Validate `restart_policy` bounds.
        match &self.restart_policy {
            None | Some(RestartPolicy::Never) => {},
            Some(RestartPolicy::OnFailure {
                max_retries,
                backoff_ms,
            }) => {
                if !(1..=100).contains(max_retries) {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!(
                            "restart_policy.max_retries must be in 1..=100; got {max_retries}"
                        ),
                    });
                }
                if !(100..=300_000).contains(backoff_ms) {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!(
                            "restart_policy.backoff_ms must be in 100..=300000; got {backoff_ms}"
                        ),
                    });
                }
            },
            Some(RestartPolicy::Always { backoff_ms }) => {
                if !(100..=300_000).contains(backoff_ms) {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!(
                            "restart_policy.backoff_ms must be in 100..=300000; got {backoff_ms}"
                        ),
                    });
                }
            },
        }

        // Validate `health_probe` bounds.
        match &self.health_probe {
            None | Some(HealthProbe::None) => {},
            Some(HealthProbe::HttpGet {
                url,
                expected_status,
                interval_ms,
                startup_grace_ms,
            }) => {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!(
                            "health_probe.url must begin with http:// or https://; got '{url}'"
                        ),
                    });
                }
                if !(100..=599).contains(expected_status) {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!(
                            "health_probe.expected_status must be in 100..=599; got {expected_status}"
                        ),
                    });
                }
                if !(100..=60_000).contains(interval_ms) {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!(
                            "health_probe.interval_ms must be in 100..=60000; got {interval_ms}"
                        ),
                    });
                }
                if *startup_grace_ms > 600_000 {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!(
                            "health_probe.startup_grace_ms must be in 0..=600000; got {startup_grace_ms}"
                        ),
                    });
                }
            },
            Some(HealthProbe::PortOpen {
                port,
                interval_ms,
                startup_grace_ms,
                ..
            }) => {
                if *port == 0 {
                    return Err(SubprocessError::InvalidRequest {
                        msg: "health_probe.port must be in 1..=65535; got 0".to_owned(),
                    });
                }
                if !(100..=60_000).contains(interval_ms) {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!(
                            "health_probe.interval_ms must be in 100..=60000; got {interval_ms}"
                        ),
                    });
                }
                if *startup_grace_ms > 600_000 {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!(
                            "health_probe.startup_grace_ms must be in 0..=600000; got {startup_grace_ms}"
                        ),
                    });
                }
            },
            Some(HealthProbe::LogPattern { regex, timeout_ms }) => {
                if regex.is_empty() {
                    return Err(SubprocessError::InvalidRequest {
                        msg: "health_probe.regex must not be empty".to_owned(),
                    });
                }
                if !(1_000..=600_000).contains(timeout_ms) {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!(
                            "health_probe.timeout_ms must be in 1000..=600000; got {timeout_ms}"
                        ),
                    });
                }
            },
        }

        // Validate `log_rotation` bounds and cross-field constraint with `capture_kind`.
        match &self.log_rotation {
            None | Some(LogRotation::None) => {},
            Some(LogRotation::BySize {
                max_bytes_per_file,
                keep_files,
            }) => {
                const MIN_FILE_BYTES: u64 = 1_048_576;
                const MAX_FILE_BYTES: u64 = 1_073_741_824;
                if !(*max_bytes_per_file >= MIN_FILE_BYTES && *max_bytes_per_file <= MAX_FILE_BYTES)
                {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!(
                            "log_rotation.max_bytes_per_file must be in \
                             1_048_576..=1_073_741_824; got {max_bytes_per_file}"
                        ),
                    });
                }
                if !(1..=20).contains(keep_files) {
                    return Err(SubprocessError::InvalidRequest {
                        msg: format!("log_rotation.keep_files must be in 1..=20; got {keep_files}"),
                    });
                }
                // Cross-field: log rotation requires TmpFile capture.
                if self.capture_kind != CaptureKind::TmpFile {
                    return Err(SubprocessError::InvalidRequest {
                        msg: "log_rotation requires capture_kind = TmpFile".to_owned(),
                    });
                }
            },
        }

        Ok(())
    }
}
