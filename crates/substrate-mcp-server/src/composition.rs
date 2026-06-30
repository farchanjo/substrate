//! Composition root — wires domain ports to adapter implementations.
//!
//! This module is the only place that instantiates concrete adapter types and
//! connects them to port traits. No other crate performs this wiring.
//!
//! Per ADR-0022, only `substrate-mcp-server` depends on rmcp and tokio with
//! full features. Adapter crates never depend on each other.
//!
//! # Wiring order
//!
//! 1. Build `Allowlist` from config security roots.
//! 2. Build `Arc<dyn PathJailPort>` via `PathJailFactory`.
//! 3. Build `Arc<dyn DirWalkerPort>` via `WalkerFactory`.
//! 4. Build `Arc<dyn HashPort>` via `HashFactory`.
//! 5. Build `Arc<dyn StatPort>` via `StatFactory`.
//! 6. Build `Arc<dyn FsIndexPort>` via `FsIndexFactory`.
//! 7. Construct BC dependency bundles.
//! 8. Build `Arc<dyn JobRegistryPort>` via `InMemoryJobRegistry::new`.
//! 9. Build `ToolDispatcher`.
//! 10. Emit `SUBSTRATE_CAPABILITY_TIERS_SELECTED` audit event.

#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use substrate_archive::ArchiveDeps;
use substrate_config::RuntimeConfig;
use substrate_domain::{
    Capabilities, FsIndexPort, JailedPath, JobRegistryPort, PortFactory, SubstrateResult,
};
use substrate_fs_index::FsIndexFactory;
use substrate_fs_mutation::FsMutationDeps;
use substrate_fs_query::{
    FsQueryDeps, hash_factory::HashFactory, stat_factory::StatFactory,
    walker_factory::WalkerFactory,
};
use substrate_jobs::InMemoryJobRegistry;

use crate::handlers::rmcp_progress_notifier::RmcpPeerNotifier;
use substrate_policy::{Allowlist, PathJailFactory};
use substrate_process::{ProcessDeps, default_scanner, new_pid_cpu_cache};
use substrate_system_info::{SystemInfoDeps, new_cpu_state};
use substrate_text::TextDeps;

use crate::audit;
use crate::handlers::dispatcher::ToolDispatcher;

use substrate_network_info::NetworkInfoFactory;

// ---- Runtime component bundle ------------------------------------------------

/// All runtime-constructed ports and shared state injected into the MCP handler.
pub(crate) struct RuntimeComponents {
    /// Central tool dispatcher — routes incoming `tools/call` to the correct
    /// adapter handler and manages Bucket B/C job promotion.
    pub(crate) dispatcher: ToolDispatcher,

    /// Optional filesystem index — used by `fs_find` fast-path (ADR-0041).
    #[expect(
        dead_code,
        reason = "wired into FsQueryDeps via dispatcher; retained for auditing the index Arc"
    )]
    pub(crate) fs_index: Arc<dyn FsIndexPort>,

    /// Root cancellation token; cancelling this propagates to all adapters.
    pub(crate) shutdown_token: CancellationToken,

    /// Frozen snapshot of the runtime configuration.
    pub(crate) config: Arc<RuntimeConfig>,

    /// Frozen snapshot of detected capabilities (from `OnceLock`).
    pub(crate) caps: Arc<Capabilities>,

    /// Late-bound progress notifier — peer is injected after `initialize`.
    ///
    /// Shared between the `SubstrateService` (which calls `set_peer`) and the
    /// `InMemoryJobRegistry` (which calls `notify_progress`/`notify_complete`).
    pub(crate) notifier: Arc<RmcpPeerNotifier>,

    /// Optional subprocess port for signal-handler cascade termination.
    ///
    /// Populated when the `subprocess` Cargo feature is active; `None` otherwise.
    /// The signal handler uses this to send SIGTERM/SIGKILL to live subprocess
    /// process groups during graceful shutdown (ADR-0032 amendment 2026-05-24).
    #[cfg(feature = "subprocess")]
    pub(crate) subprocess_for_shutdown:
        Option<Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>>,
}

// ---- Wiring ------------------------------------------------------------------

/// Constructs and wires all runtime components.
///
/// Runs after signal handlers are installed and config is loaded. Must complete
/// before any MCP `initialize` request is accepted.
///
/// # Errors
///
/// Returns `SUBSTRATE_RUNTIME_INIT_FAILED` when an adapter fails to construct.
#[expect(
    clippy::too_many_lines,
    reason = "composition root wiring: each line is a one-step factory call or assignment; \
              extracting sub-functions would create false cohesion without reducing real complexity"
)]
pub(crate) async fn wire(
    config: &RuntimeConfig,
    caps: &Capabilities,
) -> SubstrateResult<RuntimeComponents> {
    let shutdown_token = CancellationToken::new();
    let config_arc: Arc<RuntimeConfig> = Arc::new(config.clone());
    let caps_arc: Arc<Capabilities> = Arc::new(caps.clone());

    // ---- Allowlist (ADR-0004) -----------------------------------------------
    //
    // Roots come from `[policy]` section in the TOML config (ADR-0004).
    // An empty roots slice causes Allowlist::new to return ConfigInvalid
    // (fail-closed: the composition root propagates this as exit code 73).
    let allowlist = Allowlist::new(config.policy.roots.clone())?;

    // Pre-build the `JailedPath` slice that mutation handlers need as the
    // allowlist-root anchor for kernel-level confinement. We collect the
    // canonicalized roots from the allowlist before handing ownership to
    // the factory. Each PathBuf is already canonical (done by Allowlist::new).
    let allowlist_roots: Vec<JailedPath> = allowlist
        .iter_roots()
        .map(|p| allowlist.jail(p))
        // Roots are guaranteed to satisfy containment — unwrap is safe here.
        .collect::<SubstrateResult<Vec<_>>>()?;

    // ---- PathJail (ADR-0035 / ADR-0042) ------------------------------------
    let jail_factory = PathJailFactory::new(allowlist, config.security.refuse_degraded_jail);
    let jail: Arc<dyn substrate_domain::PathJailPort> = jail_factory.build(caps);

    // ---- DirWalker (ADR-0042) -----------------------------------------------
    let walker_factory = WalkerFactory::new();
    let walker: Arc<dyn substrate_domain::DirWalkerPort> = walker_factory.build(caps);

    // ---- HashPort (ADR-0042 / ADR-0043) ------------------------------------
    let hash_factory = HashFactory::new();
    let hasher: Arc<dyn substrate_domain::HashPort> = hash_factory.build(caps);

    // ---- StatPort (ADR-0042) ------------------------------------------------
    let stat_factory = StatFactory::new();
    let statter: Arc<dyn substrate_domain::StatPort> = stat_factory.build(caps);

    // ---- FsIndex (ADR-0041) -------------------------------------------------
    let fs_index: Arc<dyn FsIndexPort> = FsIndexFactory::new().build(caps);

    // ---- Audit event (ADR-0042) ---------------------------------------------
    //
    // Emitted after all factories have run so the chosen tier names are stable.
    audit::emit_capability_tiers_selected(caps).await;

    // ---- BC dependency bundles (ADR-0022) -----------------------------------

    let fs_query_deps = FsQueryDeps {
        jail: Arc::clone(&jail),
        walker: Arc::clone(&walker),
        hasher: Arc::clone(&hasher),
        statter: Arc::clone(&statter),
        capabilities: Arc::clone(&caps_arc),
    };

    // FsMutationDeps only includes the fs-index port when the `fs-index` Cargo
    // feature is active (optional write-through updates per ADR-0041).
    let fs_mutation_deps = FsMutationDeps {
        jail: Arc::clone(&jail),
        capabilities: Arc::clone(&caps_arc),
        #[cfg(feature = "fs-index")]
        index: Arc::clone(&fs_index),
    };

    let process_deps = ProcessDeps {
        capabilities: Arc::clone(&caps_arc),
    };

    let system_info_deps = SystemInfoDeps {
        capabilities: Arc::clone(&caps_arc),
    };

    let text_deps = TextDeps {
        jail: Arc::clone(&jail),
        capabilities: Arc::clone(&caps_arc),
    };

    let archive_deps = ArchiveDeps {
        jail: Arc::clone(&jail),
        hasher: Arc::clone(&hasher),
        capabilities: Arc::clone(&caps_arc),
    };

    // ---- Job registry (ADR-0040) --------------------------------------------
    //
    // The async job control-plane is ALWAYS wired. ADR-0040 defines it as a core
    // subsystem with safe defaults (max_concurrent=16, result_ttl=300s, …); there
    // is no "disabled" mode. When the operator omits the `[jobs]` TOML section,
    // `JobConfig::default()` is applied so Bucket B/C tools (archive.*, large
    // fs.read/hash/copy, text.search/count_lines) still promote to background
    // jobs correctly instead of failing with an internal error.
    //
    // The `RmcpPeerNotifier` is built once and shared between the job registry
    // (progress emitter) and `SubstrateService` (peer injector after initialize).
    let notifier: Arc<RmcpPeerNotifier> = Arc::new(RmcpPeerNotifier::new());

    let job_cfg = config.jobs.clone().unwrap_or_default();

    // ADR-0059: enforce the wait-window invariant
    // (0 < result_default_wait_ms <= result_max_wait_ms) at startup.
    // Fail-closed per ADR-0004: a misconfigured wait window aborts boot rather
    // than letting unbounded waits leak through at runtime.
    if let Err(reason) = job_cfg.quotas.validate_wait_window() {
        return Err(substrate_domain::SubstrateError::ConfigInvalid {
            offending_field: format!("jobs.quotas ({reason})"),
            correlation_id: None,
        });
    }

    let notifier_dyn: Arc<dyn substrate_jobs::ProgressNotifier> =
        Arc::clone(&notifier) as Arc<dyn substrate_jobs::ProgressNotifier>;
    let job_registry: Arc<dyn JobRegistryPort> =
        InMemoryJobRegistry::new(job_cfg, notifier_dyn, shutdown_token.child_token());

    // ---- NetworkInfoPort (ADR-0058) -----------------------------------------
    //
    // The factory probes the current platform and selects the best adapter:
    //   - macOS: MacosSysctlAdapter (sysctlbyname net.inet.tcp.pcblist_n)
    //   - Linux: LinuxProcNetAdapter (/proc/net/{tcp,tcp6,udp,udp6,snmp})
    //   - Other: NoopNetworkInfoPort — all calls return InternalError at runtime.
    //
    // This port is always-on (no feature flag). Platforms without support receive
    // the Noop fallback; tool calls return SUBSTRATE_INTERNAL_ERROR.
    let (network_port, network_tier) = NetworkInfoFactory::build();
    tracing::info!(network_tier = ?network_tier, "network-info tier selected");

    // ---- Process scanner (ADR-0028) -----------------------------------------
    //
    // Built once at composition time; the platform-specific implementation
    // (Linux: procfs, macOS: sysctl) is selected at compile time by cfg gating
    // inside `substrate_process::default_scanner()`.
    let scanner = default_scanner();

    // ---- Shared CPU snapshot for sys.cpu delta (ADR-0050) -------------------
    let cpu_state = new_cpu_state();

    // ---- Shared per-PID CPU delta cache for proc.stats/proc.top (ADR-0051) --
    let pid_cpu_cache = new_pid_cpu_cache();

    // ---- SubprocessRegistry (ADR-0052) — feature-gated ---------------------
    //
    // The subprocess port is wired only when the `subprocess` Cargo feature is
    // active. A deny-all `SubprocessRegistry` is constructed with the server's
    // root `CancellationToken` and the path allowlist so that the registry can
    // validate `cwd` containment per ADR-0052 Layer 1.
    //
    // The binary allowlist is empty by default (deny-all) matching the ADR-0052
    // default-deny stance. Operators opt in via the `[subprocess]` TOML section.
    //
    // Two `Arc` clones are derived: one goes into `ToolDispatcher.subprocess` for
    // tool dispatch; one is stored in `RuntimeComponents.subprocess_for_shutdown`
    // for the signal-handler cascade termination per ADR-0032 amendment 2026-05-24.
    //
    // `tmp_root` is resolved per the ADR-0033 amendment 2026-05-24 contract:
    //   - explicit `subprocess.tmp_root` from TOML → use verbatim.
    //   - absent → fall back to `policy.roots.first().cloned()`.
    //   - still absent → pass `None`; the registry rejects TmpFile captures at
    //     spawn time with `SubprocessError::InvalidRequest`.
    #[cfg(feature = "subprocess")]
    let (subprocess_port, subprocess_port_for_shutdown): (
        std::sync::Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>,
        Option<std::sync::Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>>,
    ) = {
        let subprocess_cfg = config.subprocess.clone().unwrap_or_default();
        let startup_cfg = config.startup.clone().unwrap_or_default();

        // Resolve tmp_root: explicit TOML value wins; fall back to first policy root.
        let tmp_root: Option<std::path::PathBuf> = subprocess_cfg
            .tmp_root
            .or_else(|| config.policy.roots.first().cloned());

        // ADR-0055 orphan reaper: sweep stale transit .tmp.<uuid7> files older
        // than `startup.orphan_reap_age_secs` before any new subprocess can
        // create transit files. Best-effort — failures are logged and ignored.
        if !startup_cfg.disable_orphan_reaper
            && let Some(ref tmp_root_path) = tmp_root
        {
            let reap_age = std::time::Duration::from_secs(startup_cfg.orphan_reap_age_secs);
            let reap_budget =
                std::time::Duration::from_secs(startup_cfg.orphan_reap_max_duration_secs);
            match tokio::time::timeout(
                reap_budget,
                substrate_subprocess::run_orphan_reaper_once(tmp_root_path, reap_age),
            )
            .await
            {
                Ok(Ok(stats)) => {
                    tracing::info!(
                        reaped = stats.reaped,
                        skipped_young = stats.skipped_young,
                        skipped_unrelated = stats.skipped_unrelated,
                        errors = stats.errors,
                        "orphan reaper completed at startup"
                    );
                },
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "orphan reaper failed to read tmp_root (non-fatal)");
                },
                Err(_elapsed) => {
                    tracing::warn!(
                        budget_secs = startup_cfg.orphan_reap_max_duration_secs,
                        "orphan reaper exceeded duration budget; continuing startup"
                    );
                },
            }
        }

        let path_allowlist_clone = Allowlist::new(config.policy.roots.clone())?;
        // Wave 3a used a builder pattern for tmp_root instead of the originally
        // spec'd constructor parameter. We adapt: call `new` then chain
        // `with_tmp_root` when a tmp_root is available.
        let registry_base = substrate_subprocess::registry::SubprocessRegistry::new(
            substrate_subprocess::registry::BinaryAllowlist::new(
                subprocess_cfg.binary_allowlist.clone(),
            ),
            Vec::new(),
            subprocess_cfg.max_per_client,
            subprocess_cfg.max_concurrent,
            subprocess_cfg.aggregate_buffer_bytes,
            subprocess_cfg.shutdown_drain_secs,
            path_allowlist_clone,
            shutdown_token.child_token(),
        );
        let registry_with_tmp = if let Some(root) = tmp_root {
            registry_base.with_tmp_root(root)
        } else {
            registry_base
        };
        // Wire the RmcpStreamNotifier observer (Observer + Mediator pattern per
        // ADR-0054 and arch review). Shares the late-bound peer with
        // `RmcpPeerNotifier` so both job-progress and stream-chunk events flow
        // over the same `Peer<RoleServer>` after `initialize`.
        let stream_observer: std::sync::Arc<
            dyn substrate_domain::ports::stream_observer::StreamChunkObserver,
        > = std::sync::Arc::new(
            crate::handlers::rmcp_stream_notifier::RmcpStreamNotifier::new(Arc::clone(&notifier)),
        );
        let registry = registry_with_tmp.with_observers(vec![stream_observer]);
        let port_a: std::sync::Arc<dyn substrate_domain::ports::subprocess::SubprocessPort> =
            Arc::clone(&registry)
                as std::sync::Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>;
        let port_b: Option<
            std::sync::Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>,
        > = Some(Arc::clone(&registry)
            as std::sync::Arc<
                dyn substrate_domain::ports::subprocess::SubprocessPort,
            >);
        (port_a, port_b)
    };

    // ---- LaunchRegistry (ADR-0063..0069) — feature-gated -------------------
    //
    // The launch orchestrator port is wired only when the `launch` Cargo feature
    // is active. The `launch` feature implies `subprocess`, so `subprocess_port`
    // above is always in scope here: the registry routes every managed Service
    // through that injected `Arc<dyn SubprocessPort>` and never spawns processes
    // directly (no `no_subprocess.rego` exception needed per ADR-0063 §"MVP").
    //
    // `state_root` holds the per-Stack durable state files and the TOFU trust
    // store (`<state_root>/launch-trust.toml`). It falls back to the first policy
    // root and then to the system temp dir so the registry always has a home.
    #[cfg(feature = "launch")]
    let launch_port: std::sync::Arc<dyn substrate_domain::ports::launch::LaunchPort> = {
        let state_root = config
            .policy
            .roots
            .first()
            .cloned()
            .unwrap_or_else(std::env::temp_dir);
        substrate_launch::LaunchRegistry::new(Arc::clone(&subprocess_port), state_root)
    };

    // ---- Reaper-on-boot reconcile sweep (ADR-0068) --------------------------
    //
    // A prior server session may have spawned detached supervisors (`launch.up`
    // with `on_client_disconnect = detach`) that outlived it. Sweep the durable
    // stacks root once, before the MCP transport opens: a still-live supervisor is
    // re-attached (left untouched), while a dead supervisor's orphaned children are
    // adopted or reaped per ADR-0068. Reaper findings restore a clean host; they
    // are never a startup error, so an unresolved root or sweep failure is logged,
    // not propagated.
    #[cfg(feature = "launch")]
    {
        match substrate_launch::supervisor_registry::launch_stacks_root() {
            Ok(stacks_root) => match substrate_launch::reaper::reconcile_sweep(&stacks_root).await {
                Ok(report) if !report.is_empty() => tracing::info!(
                    reattached = report.reattached.len(),
                    adopted = report.adopted.len(),
                    reaped = report.reaped.len(),
                    recycled = report.recycled.len(),
                    "launch reaper-on-boot reconcile sweep applied"
                ),
                Ok(_) => {},
                Err(e) => {
                    tracing::warn!(error = %e, "launch reaper-on-boot sweep failed (non-fatal)");
                },
            },
            Err(e) => {
                tracing::warn!(error = %e, "launch stacks root unresolved; skipping reaper sweep");
            },
        }
    }

    // ---- ToolDispatcher (ADR-0022) ------------------------------------------
    let dispatcher = ToolDispatcher {
        fs_query: fs_query_deps,
        fs_mutation: fs_mutation_deps,
        process: process_deps,
        scanner,
        system_info: system_info_deps,
        text: text_deps,
        archive: archive_deps,
        jobs: Arc::clone(&job_registry),
        config: Arc::clone(&config_arc),
        allowlist_roots,
        cpu_state,
        pid_cpu_cache,
        #[cfg(feature = "subprocess")]
        subprocess: subprocess_port,
        #[cfg(feature = "launch")]
        launch: launch_port,
        network: network_port,
    };

    Ok(RuntimeComponents {
        dispatcher,
        fs_index,
        shutdown_token,
        config: config_arc,
        caps: caps_arc,
        notifier,
        #[cfg(feature = "subprocess")]
        subprocess_for_shutdown: subprocess_port_for_shutdown,
    })
}

/// Builds the minimal `SubprocessPort` the detached supervisor drives (ADR-0068).
///
/// The `substrate --supervise` process is not the MCP server: it manages only the
/// Stack's Services, so it needs the subprocess registry but none of the MCP
/// stream observers or the startup orphan reaper that [`wire`] sets up. Every
/// managed Service is still spawned through this single injected port (hexagonal
/// layering, ADR-0022), with `tmp_root` resolved per the ADR-0033 amendment
/// contract (explicit `subprocess.tmp_root`, else the first policy root).
///
/// # Errors
///
/// Returns `SUBSTRATE_CONFIG_INVALID` when the policy allowlist cannot be built
/// (for example an empty or non-canonicalizable roots list).
#[cfg(feature = "launch")]
pub(crate) fn build_supervisor_subprocess_port(
    config: &RuntimeConfig,
    root_cancel: &CancellationToken,
) -> SubstrateResult<Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>> {
    let subprocess_cfg = config.subprocess.clone().unwrap_or_default();
    let tmp_root = subprocess_cfg
        .tmp_root
        .clone()
        .or_else(|| config.policy.roots.first().cloned());
    let path_allowlist = Allowlist::new(config.policy.roots.clone())?;
    let registry = substrate_subprocess::registry::SubprocessRegistry::new(
        substrate_subprocess::registry::BinaryAllowlist::new(subprocess_cfg.binary_allowlist.clone()),
        Vec::new(),
        subprocess_cfg.max_per_client,
        subprocess_cfg.max_concurrent,
        subprocess_cfg.aggregate_buffer_bytes,
        subprocess_cfg.shutdown_drain_secs,
        path_allowlist,
        root_cancel.child_token(),
    );
    let registry = if let Some(root) = tmp_root {
        registry.with_tmp_root(root)
    } else {
        registry
    };
    let port: Arc<dyn substrate_domain::ports::subprocess::SubprocessPort> = registry;
    Ok(port)
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test code: panicking assertions are idiomatic in unit tests"
)]
mod tests {
    use super::*;
    use substrate_domain::ClientId;

    // Regression: with no `[jobs]` TOML section (`config.jobs == None`), the
    // composition root must still wire the real `InMemoryJobRegistry`. Before the
    // fix it fell back to `NullJobRegistry`, so every job-control-plane call and
    // every Bucket B/C promotion returned SUBSTRATE_INTERNAL_ERROR.
    #[tokio::test]
    async fn job_control_plane_wired_when_config_section_absent() {
        // A real, canonical allowlist root so `Allowlist::new` succeeds.
        let root = std::fs::canonicalize(std::env::temp_dir()).expect("temp dir must canonicalize");

        let mut config = RuntimeConfig::default();
        config.policy.roots = vec![root];
        // Accept the userspace jail tier in the test harness regardless of the
        // probed capability tier (see ADR-0035 / ADR-0042).
        config.security.refuse_degraded_jail = false;
        assert!(config.jobs.is_none(), "precondition: no [jobs] section");

        let caps = Capabilities::default();
        let components = wire(&config, &caps).await.expect("wire must succeed");

        let client = ClientId::parse("test-client").expect("valid client id");
        // `NullJobRegistry::list` returned Err; the real registry returns an
        // empty page for a client with no submitted jobs.
        let page = components
            .dispatcher
            .jobs
            .list(&client, None, substrate_domain::PageSize::default())
            .await
            .expect("real job registry must list, not error from a null stub");
        assert!(page.jobs.is_empty(), "fresh registry has no jobs");
    }
}
