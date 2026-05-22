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
use substrate_process::{ProcessDeps, default_scanner};
use substrate_system_info::SystemInfoDeps;
use substrate_text::TextDeps;

use crate::audit;
use crate::handlers::dispatcher::ToolDispatcher;

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
        .map(|p| allowlist.jail(p.to_path_buf()))
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
    let notifier_dyn: Arc<dyn substrate_jobs::ProgressNotifier> =
        Arc::clone(&notifier) as Arc<dyn substrate_jobs::ProgressNotifier>;
    let job_registry: Arc<dyn JobRegistryPort> =
        InMemoryJobRegistry::new(job_cfg, notifier_dyn, shutdown_token.child_token());

    // ---- Process scanner (ADR-0028) -----------------------------------------
    //
    // Built once at composition time; the platform-specific implementation
    // (Linux: procfs, macOS: sysctl) is selected at compile time by cfg gating
    // inside `substrate_process::default_scanner()`.
    let scanner = default_scanner();

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
    };

    Ok(RuntimeComponents {
        dispatcher,
        fs_index,
        shutdown_token,
        config: config_arc,
        caps: caps_arc,
        notifier,
    })
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
            .list(&client, None)
            .await
            .expect("real job registry must list, not error from a null stub");
        assert!(page.jobs.is_empty(), "fresh registry has no jobs");
    }
}
