//! Startup audit event emitter per ADR-0038.
//!
//! Audit events are structured `tracing` events routed to stderr via the
//! subscriber initialized in `logging::init`. The `target = "substrate.audit"`
//! field allows log processors to identify audit records.
//!
//! # Current scope
//!
//! Only startup-phase events are emitted here. Per-call audit events (tool
//! invocations, state transitions) will be added in the adapter waves (Wave D/E).
//!
//! # Real audit writer
//!
//! In a future wave, this module will delegate to a dedicated audit writer
//! (file, syslog, or external sink) configured via `[audit]` TOML config.
//! The current implementation is a thin tracing wrapper.

#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

use substrate_domain::Capabilities;

/// Emits the `SUBSTRATE_CAPABILITY_TIERS_SELECTED` startup audit event per ADR-0042.
///
/// Called once, after capability probing completes and before any MCP session
/// is accepted. Records the selected tier for every port as a structured log
/// event routed to stderr.
#[expect(
    clippy::unused_async,
    reason = "async signature retained for Wave D when emit may delegate to an async audit writer"
)]
pub(crate) async fn emit_capability_tiers_selected(caps: &Capabilities) {
    // Emit as a structured tracing event with target = "substrate.audit" so that
    // log processors can filter audit records separately from diagnostic logs.
    tracing::info!(
        target: "substrate.audit",
        event_type = "SUBSTRATE_CAPABILITY_TIERS_SELECTED",
        simd_tier = ?caps.simd_tier,
        walker_tier = ?caps.walker_tier,
        watcher_tier = ?caps.watcher_tier,
        jail_tier = ?caps.jail_tier,
        hash_tier = ?caps.hash_tier,
        stat_tier = ?caps.stat_tier,
        has_openat2 = caps.has_openat2,
        has_statx = caps.has_statx,
        has_io_uring = caps.has_io_uring,
        has_inotify = caps.has_inotify,
        has_fanotify = caps.has_fanotify,
        has_getattrlistbulk = caps.has_getattrlistbulk,
        has_fsevents = caps.has_fsevents,
        has_kqueue = caps.has_kqueue,
        has_o_nofollow_any = caps.has_o_nofollow_any,
        seq = 0u64,
        "SUBSTRATE_CAPABILITY_TIERS_SELECTED"
    );
}

/// Emits a `SUBSTRATE_JAIL_DEGRADED` audit event per ADR-0042.
///
/// Called when `PathJail` falls back to the userspace-degraded tier, regardless
/// of whether `refuse_degraded_jail` aborts startup. Helps operators understand
/// the degraded security posture in their environment.
#[expect(
    clippy::unused_async,
    reason = "async signature retained for Wave D when emit may delegate to an async audit writer"
)]
pub(crate) async fn emit_jail_degraded(caps: &Capabilities) {
    tracing::warn!(
        target: "substrate.audit",
        event_type = "SUBSTRATE_JAIL_DEGRADED",
        jail_tier = ?caps.jail_tier,
        has_openat2 = caps.has_openat2,
        has_o_nofollow_any = caps.has_o_nofollow_any,
        "PathJail fell back to userspace-degraded tier; TOCTOU window is not atomically closed"
    );
}
