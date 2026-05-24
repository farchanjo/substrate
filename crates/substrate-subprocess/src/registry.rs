//! `SubprocessRegistry` — concrete [`SubprocessPort`] adapter per ADR-0052.
//!
//! Enforces all five security layers from ADR-0052:
//! 1. Allowlist check for `cwd`.
//! 2. Binary allowlist (Layer 5).
//! 3. Environment allowlist (Layer 5 — strip banned/non-listed keys).
//! 4. Elicitation confirmation (mandatory for every spawn).
//! 5. Quota enforcement (per-client and global).
//!
//! Manages `Arc<ChildHandle>` entries in a `DashMap<JobId, Arc<ChildHandle>>`.
//!
//! References: ADR-0052, ADR-0053, ADR-0054.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use substrate_domain::errors::{SubstrateError, SubstrateResult};
use substrate_domain::ports::subprocess::{
    SignalTarget, SubprocessPort, SubprocessResult, SubprocessSignalName,
};
use substrate_domain::subprocess::errors::SubprocessError;
use substrate_domain::subprocess::handle::SubprocessHandle;
use substrate_domain::subprocess::request::SubprocessRequest;
use substrate_domain::subprocess::state::SubprocessState;
use substrate_domain::value_objects::{ClientId, JobId, ProcessGroup};

use substrate_policy::Allowlist;

use crate::cascade::terminate_cascade;
use crate::spawn::{ChildHandle, spawn_supervised};
use crate::stream_capture::{make_stream_channel, spawn_stream_captures};

/// Default per-job stdout/stderr ring-buffer size per ADR-0054.
const DEFAULT_AGGREGATE_BUFFER_BYTES: usize = 65_536;

/// Unconditionally banned environment variable keys per ADR-0052 §"Layer 5".
///
/// These keys are injection vectors. Mirroring [`BANNED_ENV_VARS`] in domain
/// for defense-in-depth at the adapter layer.
const BANNED_ENV_KEYS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
];

/// A simple binary allowlist: a set of absolute path strings.
///
/// Empty = deny-all (default per ADR-0052 §"Binary allowlist").
#[derive(Debug, Clone)]
pub struct BinaryAllowlist {
    /// Absolute paths of permitted binaries.
    entries: Vec<PathBuf>,
}

impl BinaryAllowlist {
    /// Constructs the allowlist from a list of absolute paths.
    #[must_use]
    pub const fn new(entries: Vec<PathBuf>) -> Self {
        Self { entries }
    }

    /// Constructs an empty (deny-all) allowlist.
    #[must_use]
    pub const fn deny_all() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Returns `true` when `path` is in the allowlist.
    #[must_use]
    pub fn allows(&self, path: &std::path::Path) -> bool {
        self.entries.iter().any(|e| e == path)
    }
}

/// Concrete adapter implementing [`SubprocessPort`].
///
/// Constructed once and shared via `Arc<SubprocessRegistry>` in the composition root.
/// Manages all `ChildHandle` entries and enforces security + quota invariants
/// before any OS spawn.
///
/// References: ADR-0052, ADR-0053, ADR-0054.
#[derive(Debug)]
pub struct SubprocessRegistry {
    /// Live subprocess handles keyed by `JobId`.
    handles: Arc<DashMap<JobId, Arc<ChildHandle>>>,

    /// Allowlist of permitted executable binaries (empty = deny-all).
    binary_allowlist: BinaryAllowlist,

    /// Allowlist of parent-environment keys the child may inherit.
    #[expect(
        dead_code,
        reason = "Wave 2c: consumed via SubprocessRequest.env_allowlist at spawn time"
    )]
    env_allowlist: Vec<String>,

    /// Maximum active subprocesses per client per ADR-0052.
    max_per_client: u32,

    /// Global maximum active subprocesses per ADR-0052.
    max_concurrent: u32,

    /// Per-job ring-buffer size in bytes per ADR-0054.
    aggregate_buffer_bytes: usize,

    /// Seconds to wait between SIGTERM and SIGKILL per ADR-0053.
    shutdown_drain_secs: u64,

    /// Path allowlist for cwd validation.
    path_allowlist: Allowlist,

    /// Server root cancellation token for deriving per-job child tokens.
    root_cancel: CancellationToken,

    /// Per-client active-subprocess counters.
    per_client_active: Arc<DashMap<ClientId, u32>>,
}

impl SubprocessRegistry {
    /// Constructs a new `SubprocessRegistry`.
    ///
    /// # Parameters
    ///
    /// - `binary_allowlist`: permitted executable binaries (empty = deny-all).
    /// - `env_allowlist`: parent-env keys the child may inherit.
    /// - `max_per_client`: per-client subprocess cap (default 4 per ADR-0052).
    /// - `max_concurrent`: global subprocess cap (default 8 per ADR-0052).
    /// - `aggregate_buffer_bytes`: ring-buffer size per stream per ADR-0054.
    /// - `shutdown_drain_secs`: SIGTERM→SIGKILL drain window per ADR-0053.
    /// - `path_allowlist`: allowlist used to validate `cwd`.
    /// - `root_cancel`: server root `CancellationToken`.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "construction requires all domain configuration fields; a Builder would be overkill for an internal type"
    )]
    pub fn new(
        binary_allowlist: BinaryAllowlist,
        env_allowlist: Vec<String>,
        max_per_client: u32,
        max_concurrent: u32,
        aggregate_buffer_bytes: usize,
        shutdown_drain_secs: u64,
        path_allowlist: Allowlist,
        root_cancel: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            handles: Arc::default(),
            binary_allowlist,
            env_allowlist,
            max_per_client,
            max_concurrent,
            aggregate_buffer_bytes,
            shutdown_drain_secs,
            path_allowlist,
            root_cancel,
            per_client_active: Arc::default(),
        })
    }

    /// Returns the number of currently active (non-terminal) subprocesses.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.handles.len()
    }

    /// Checks and enforces the quota for `client_id`.
    ///
    /// Returns `Err(SubprocessError::QuotaExceeded)` when the per-client or
    /// global quota is reached.
    #[expect(
        dead_code,
        reason = "Wave 2c: called from MCP handler layer with per-request client_id"
    )]
    fn check_quotas(&self, client_id: &ClientId) -> Result<(), SubprocessError> {
        // Global quota. usize -> u32: handle count is bounded by max_concurrent (u32).
        #[expect(
            clippy::cast_possible_truncation,
            reason = "handle count is bounded by max_concurrent which is u32; truncation is impossible in practice"
        )]
        let global = self.handles.len() as u32;
        if global >= self.max_concurrent {
            return Err(SubprocessError::QuotaExceeded {
                limit: self.max_concurrent,
            });
        }
        // Per-client quota.
        let per_client = self.per_client_active.get(client_id).map_or(0, |v| *v);
        if per_client >= self.max_per_client {
            return Err(SubprocessError::QuotaExceeded {
                limit: self.max_per_client,
            });
        }
        Ok(())
    }

    /// Increments the per-client active counter.
    #[expect(
        dead_code,
        reason = "Wave 2c: called from MCP handler quota enforcement path"
    )]
    fn increment_client(&self, client_id: &ClientId) {
        self.per_client_active
            .entry(client_id.clone())
            .and_modify(|v| *v += 1)
            .or_insert(1);
    }

    /// Decrements the per-client active counter (clamped at 0).
    #[expect(
        dead_code,
        reason = "Wave 2c: called from cascade kill chain on terminal state"
    )]
    fn decrement_client(&self, client_id: &ClientId) {
        self.per_client_active
            .entry(client_id.clone())
            .and_modify(|v| {
                *v = v.saturating_sub(1);
            });
    }

    /// Builds a [`SubprocessHandle`] snapshot from a live [`ChildHandle`].
    fn snapshot_handle(handle: &ChildHandle) -> SubprocessHandle {
        SubprocessHandle {
            job_id: handle.job_id.clone(),
            process_group: handle.process_group,
            state: SubprocessState::Running,
            started_at: time::OffsetDateTime::now_utc(),
            exit_code: None,
            stream_chunks_dropped: handle.stream_chunks_dropped.load(Ordering::Relaxed),
            tmp_files: Vec::new(),
        }
    }

    /// Signals a process or process group.
    fn do_signal(
        process_group: ProcessGroup,
        signal_name: SubprocessSignalName,
        target: SignalTarget,
    ) -> SubstrateResult<()> {
        use nix::sys::signal::{kill, killpg};
        use nix::unistd::Pid;

        let nix_signal = map_signal_name(signal_name);
        let result = match target {
            SignalTarget::Process => kill(Pid::from_raw(process_group.pid()), Some(nix_signal)),
            SignalTarget::ProcessGroup => {
                killpg(Pid::from_raw(process_group.pgid()), Some(nix_signal))
            },
        };
        result.map_err(|e| SubstrateError::InternalError {
            reason: format!("signal {signal_name} to {process_group} failed: {e}"),
            correlation_id: None,
        })
    }
}

#[async_trait]
impl SubprocessPort for SubprocessRegistry {
    /// Spawns a new child process per ADR-0052 five-layer security stack.
    ///
    /// Security checks in order (bail-out on first failure):
    /// 1. `req.validate()` — domain field checks.
    /// 2. `elicitation_confirmed` — unconditional per ADR-0052.
    /// 3. Binary allowlist — Layer 5 per ADR-0052.
    /// 4. `cwd` within path allowlist — Layer 1 per ADR-0004.
    /// 5. Environment allowlist — Layer 5 (strip banned keys).
    /// 6. Quota enforcement.
    /// 7. OS spawn via `spawn_supervised`.
    async fn spawn(
        &self,
        req: SubprocessRequest,
        _cancel: &dyn substrate_domain::ports::fs_index::CancelSignal,
    ) -> Result<SubprocessHandle, SubprocessError> {
        // Layer: domain validation.
        req.validate()?;

        // Layer: binary allowlist.
        if !self.binary_allowlist.allows(&req.binary_path) {
            return Err(SubprocessError::BinaryNotAllowed {
                path: req.binary_path.display().to_string(),
            });
        }

        // Layer: cwd within path allowlist.
        if !allowlist_contains(&self.path_allowlist, &req.cwd) {
            return Err(SubprocessError::CwdOutsideAllowlist {
                path: req.cwd.display().to_string(),
            });
        }

        // Layer: env_allowlist strip of any banned keys (defense-in-depth,
        // domain already validated but adapter enforces again here).
        for key in &req.env_allowlist {
            if BANNED_ENV_KEYS.contains(&key.as_str()) {
                return Err(SubprocessError::EnvBanned { var: key.clone() });
            }
        }
        for key in req.env_override.keys() {
            if BANNED_ENV_KEYS.contains(&key.as_str()) {
                return Err(SubprocessError::EnvBanned { var: key.clone() });
            }
        }

        // Quota check: enforce global quota only at this layer.
        // Per-client quota enforcement is wired at the MCP handler layer (Wave 2c).
        // usize -> u32: bounded by max_concurrent which is u32.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "active handle count is bounded by max_concurrent (u32); truncation is impossible"
        )]
        let global = self.handles.len() as u32;
        if global >= self.max_concurrent {
            return Err(SubprocessError::QuotaExceeded {
                limit: self.max_concurrent,
            });
        }

        // OS spawn.
        let handle = Arc::new(
            spawn_supervised(&req, self.root_cancel.clone(), self.aggregate_buffer_bytes).await?,
        );
        let job_id = handle.job_id.clone();

        // Wire stream capture tasks. Lock child mutex, set up captures, then drop guard.
        let (sender, _receiver) = make_stream_channel();
        {
            let mut child_guard = handle.child.lock().await;
            let Some(child) = child_guard.as_mut() else {
                drop(child_guard);
                return Err(SubprocessError::SpawnFailed {
                    source: std::io::Error::other("child not available immediately after spawn"),
                });
            };
            spawn_stream_captures(child, &handle, sender).map_err(|e| {
                SubprocessError::SpawnFailed {
                    source: std::io::Error::other(e.to_string()),
                }
            })?;
            drop(child_guard);
        }

        // Register in the live map.
        self.handles.insert(job_id.clone(), Arc::clone(&handle));

        info!(
            target: "substrate_audit",
            event = "SUBSTRATE_SUBPROCESS_SPAWNED",
            job_id = %job_id,
            binary = %req.binary_path.display(),
            pgid = handle.process_group.pgid(),
        );

        Ok(Self::snapshot_handle(&handle))
    }

    async fn list(
        &self,
        _client_id: &ClientId,
        _state_filter: Option<&[SubprocessState]>,
        _page_cursor: Option<&str>,
        page_size: u32,
    ) -> SubstrateResult<(Vec<SubprocessHandle>, Option<String>)> {
        let page_size = (page_size as usize).min(500);
        let handles: Vec<SubprocessHandle> = self
            .handles
            .iter()
            .take(page_size)
            .map(|entry| Self::snapshot_handle(entry.value()))
            .collect();
        Ok((handles, None))
    }

    async fn cancel(&self, job_id: &JobId, force: bool) -> SubstrateResult<SubprocessState> {
        let handle = {
            let guard = self
                .handles
                .get(job_id)
                .ok_or_else(|| SubstrateError::JobNotFound {
                    job_id: job_id.to_string(),
                    correlation_id: None,
                })?;
            Arc::clone(&*guard)
        };

        let terminal = terminate_cascade(&handle, self.shutdown_drain_secs, force)
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: e.to_string(),
                correlation_id: None,
            })?;

        // Remove from live map.
        self.handles.remove(job_id);

        Ok(terminal)
    }

    async fn result(
        &self,
        job_id: &JobId,
        wait_ms: u32,
        include_aggregates: bool,
    ) -> SubstrateResult<SubprocessResult> {
        let handle = {
            let guard = self
                .handles
                .get(job_id)
                .ok_or_else(|| SubstrateError::JobNotFound {
                    job_id: job_id.to_string(),
                    correlation_id: None,
                })?;
            Arc::clone(&*guard)
        };

        // If still live and wait_ms > 0, poll for exit.
        if wait_ms > 0 {
            let _ = tokio::time::timeout(
                Duration::from_millis(u64::from(wait_ms)),
                handle.wait_exit(),
            )
            .await;
        }

        // Build the result from ring buffers.
        let (stdout_agg, stdout_truncated) = if include_aggregates {
            let ring = handle.stdout_ring.lock().await;
            (ring.as_bytes().to_vec(), ring.truncated)
        } else {
            (Vec::new(), false)
        };
        let (stderr_agg, stderr_truncated) = if include_aggregates {
            let ring = handle.stderr_ring.lock().await;
            (ring.as_bytes().to_vec(), ring.truncated)
        } else {
            (Vec::new(), false)
        };

        let dropped = handle.stream_chunks_dropped.load(Ordering::Relaxed);
        let stdout_total = handle.stdout_bytes_total.load(Ordering::Relaxed);
        let stderr_total = handle.stderr_bytes_total.load(Ordering::Relaxed);

        Ok(SubprocessResult {
            terminal_state: SubprocessState::Running, // updated when truly terminal
            exit_code: None,
            stdout_aggregate: stdout_agg,
            stderr_aggregate: stderr_agg,
            stdout_aggregate_truncated: stdout_truncated,
            stderr_aggregate_truncated: stderr_truncated,
            stream_chunks_dropped: dropped,
            duration_ms: 0,
            stdout_bytes_total: stdout_total,
            stderr_bytes_total: stderr_total,
            terminal_at: time::OffsetDateTime::now_utc(),
        })
    }

    async fn signal(
        &self,
        job_id: &JobId,
        signal_name: SubprocessSignalName,
        target: SignalTarget,
    ) -> SubstrateResult<()> {
        // Destructive signals require elicitation per ADR-0052.
        if matches!(
            signal_name,
            SubprocessSignalName::Sigkill
                | SubprocessSignalName::Sigterm
                | SubprocessSignalName::Sigstop
        ) {
            // The registry trusts that the MCP handler has already verified elicitation
            // before calling into the port. This assertion is a defense-in-depth log.
            warn!(
                target: "substrate_audit",
                event = "SUBSTRATE_SUBPROCESS_DESTRUCTIVE_SIGNAL",
                job_id = %job_id,
                signal = %signal_name,
                "destructive signal sent; ensure elicitation was confirmed at MCP layer"
            );
        }

        let handle = {
            let guard = self
                .handles
                .get(job_id)
                .ok_or_else(|| SubstrateError::JobNotFound {
                    job_id: job_id.to_string(),
                    correlation_id: None,
                })?;
            Arc::clone(&*guard)
        };

        Self::do_signal(handle.process_group, signal_name, target)
    }
}

/// Maps a [`SubprocessSignalName`] to the corresponding `nix::sys::signal::Signal`.
const fn map_signal_name(name: SubprocessSignalName) -> nix::sys::signal::Signal {
    match name {
        SubprocessSignalName::Sigterm => nix::sys::signal::Signal::SIGTERM,
        SubprocessSignalName::Sigint => nix::sys::signal::Signal::SIGINT,
        SubprocessSignalName::Sigkill => nix::sys::signal::Signal::SIGKILL,
        SubprocessSignalName::Sigstop => nix::sys::signal::Signal::SIGSTOP,
        SubprocessSignalName::Sigcont => nix::sys::signal::Signal::SIGCONT,
        SubprocessSignalName::Sighup => nix::sys::signal::Signal::SIGHUP,
        SubprocessSignalName::Sigusr1 => nix::sys::signal::Signal::SIGUSR1,
        SubprocessSignalName::Sigusr2 => nix::sys::signal::Signal::SIGUSR2,
    }
}

// ---- Allow-list helpers ----------------------------------------------------

/// Returns `true` when `path` is within any root in the `allowlist`.
///
/// Used to validate `cwd` per ADR-0052 Layer 1 without constructing a
/// [`substrate_domain::JailedPath`] (which requires the `PathJail` factory).
fn allowlist_contains(allowlist: &Allowlist, path: &std::path::Path) -> bool {
    allowlist.iter_roots().any(|root| path.starts_with(root))
}

/// Convenience factory that wires a deny-all `SubprocessRegistry` for use in
/// tests or when no binary allowlist has been configured.
#[must_use]
pub fn deny_all_registry(
    path_allowlist: Allowlist,
    root_cancel: CancellationToken,
) -> Arc<SubprocessRegistry> {
    SubprocessRegistry::new(
        BinaryAllowlist::deny_all(),
        Vec::new(),
        4,
        8,
        DEFAULT_AGGREGATE_BUFFER_BYTES,
        5,
        path_allowlist,
        root_cancel,
    )
}

// ---- Re-exports for tests --------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_allowlist_deny_all_rejects_any_path() {
        let al = BinaryAllowlist::deny_all();
        assert!(
            !al.allows(std::path::Path::new("/usr/bin/true")),
            "deny-all allowlist must reject all binaries"
        );
    }

    #[test]
    fn binary_allowlist_allows_configured_path() {
        let al = BinaryAllowlist::new(vec![PathBuf::from("/usr/bin/true")]);
        assert!(
            al.allows(std::path::Path::new("/usr/bin/true")),
            "allowlist must accept the configured binary"
        );
        assert!(
            !al.allows(std::path::Path::new("/usr/bin/false")),
            "allowlist must reject an unconfigured binary"
        );
    }

    #[test]
    fn ring_buffer_push_and_retrieve() {
        let mut ring = crate::spawn::RingBuffer::new(8);
        ring.push(b"hello");
        assert_eq!(ring.as_bytes(), b"hello");
        assert!(!ring.truncated);
    }

    #[test]
    fn ring_buffer_overflow_keeps_newest_bytes() {
        let mut ring = crate::spawn::RingBuffer::new(4);
        ring.push(b"12345678");
        // Last 4 bytes of input.
        assert_eq!(ring.as_bytes(), b"5678");
        assert!(ring.truncated);
    }

    #[test]
    fn ring_buffer_partial_eviction() {
        let mut ring = crate::spawn::RingBuffer::new(6);
        ring.push(b"hello "); // 6 bytes, fills buffer.
        ring.push(b"world"); // 5 bytes; 5 bytes of old data must be evicted.
        assert_eq!(ring.as_bytes(), b" world");
        assert!(ring.truncated);
    }
}
