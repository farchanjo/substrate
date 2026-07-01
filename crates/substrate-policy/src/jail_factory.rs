//! `PathJailFactory` — `PortFactory<dyn PathJailPort>` implementation per ADR-0042.
//!
//! Selects the highest available path-jail tier at composition-root startup:
//!
//! 1. Linux: `openat2` tier when `caps.has_openat2` is true.
//! 2. macOS: `O_NOFOLLOW_ANY` tier when `caps.has_o_nofollow_any` is true.
//! 3. Degraded userspace fallback otherwise.
//!
//! The factory records the chosen tier name and exposes it via `chosen_tier()`
//! for the `SUBSTRATE_CAPABILITY_TIERS_SELECTED` audit event.

use std::sync::{Arc, OnceLock};

use substrate_domain::{Capabilities, PathJailPort, PortFactory};

use crate::allowlist::Allowlist;

/// Abstract factory for `PathJailPort` implementations.
///
/// Constructed once by the composition root with the operator's
/// `refuse_degraded` flag (from `security.refuse_degraded_jail`, default `true`).
pub struct PathJailFactory {
    allowlist: Allowlist,
    refuse_degraded: bool,
    chosen_tier: OnceLock<&'static str>,
}

impl PathJailFactory {
    /// Creates a new `PathJailFactory`.
    ///
    /// - `allowlist`: pre-validated root set.
    /// - `refuse_degraded`: when `true`, the composition root must abort
    ///   startup if the degraded tier is selected. The factory records the
    ///   tier and returns the degraded adapter regardless; the caller is
    ///   responsible for inspecting `chosen_tier()` and aborting.
    #[must_use]
    pub const fn new(allowlist: Allowlist, refuse_degraded: bool) -> Self {
        Self {
            allowlist,
            refuse_degraded,
            chosen_tier: OnceLock::new(),
        }
    }
}

impl PortFactory<dyn PathJailPort> for PathJailFactory {
    fn build(&self, caps: &Capabilities) -> Arc<dyn PathJailPort> {
        // --- Tier 1: Linux openat2 ---
        #[cfg(target_os = "linux")]
        if caps.has_openat2 {
            // OnceLock was already set — factory called more than once.
            // Non-fatal; the first value wins.
            self.chosen_tier.set("linux-openat2").unwrap_or(());
            tracing::info!(tier = "linux-openat2", "PathJail tier selected");
            return Arc::new(crate::linux::Openat2Jail::new(self.allowlist.clone()));
        }

        // --- Tier 1: macOS O_NOFOLLOW_ANY ---
        #[cfg(target_os = "macos")]
        if caps.has_o_nofollow_any {
            self.chosen_tier.set("macos-o-nofollow-any").unwrap_or(());
            tracing::info!(tier = "macos-o-nofollow-any", "PathJail tier selected");
            return Arc::new(crate::macos::ONoFollowAnyJail::new(self.allowlist.clone()));
        }

        // Suppress unused-variable warning when neither platform cfg is active.
        let _ = caps;

        // --- Tier degraded ---
        self.chosen_tier.set("userspace-degraded").unwrap_or(());

        if self.refuse_degraded {
            // Log at ERROR level; the composition root must abort after build()
            // returns. We do NOT panic here — panics are forbidden by the
            // workspace lint baseline (clippy::panic = "deny"). The caller
            // checks chosen_tier() == "userspace-degraded" and aborts.
            tracing::error!(
                tier = "userspace-degraded",
                refuse_degraded = true,
                "PathJail tier 1 is unavailable and security.refuse_degraded_jail = true. \
                 The composition root must abort with SUBSTRATE_JAIL_DEGRADED_REFUSED \
                 (exit code 77) before accepting any MCP requests."
            );
        } else {
            tracing::warn!(
                tier = "userspace-degraded",
                "PathJail degraded tier selected; TOCTOU window is not atomically closed. \
                 Emitting SUBSTRATE_JAIL_DEGRADED audit event."
            );
        }

        Arc::new(crate::userspace_jail::UserspaceJail::new(
            self.allowlist.clone(),
        ))
    }

    fn chosen_tier(&self) -> &'static str {
        self.chosen_tier.get().copied().unwrap_or("unset")
    }
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use super::*;

    fn make_allowlist() -> Allowlist {
        let dir = tempfile::tempdir().expect("tempdir must be creatable");
        Allowlist::new(vec![dir.path().to_path_buf()]).expect("valid allowlist")
    }

    #[test]
    fn chosen_tier_is_unset_before_build() {
        let factory = PathJailFactory::new(make_allowlist(), true);
        assert_eq!(factory.chosen_tier(), "unset");
    }

    #[test]
    fn build_with_default_caps_selects_degraded() {
        let allowlist = make_allowlist();
        // Constructed with refuse_degraded = false so test does not produce
        // an ERROR-level log that could confuse CI log parsers.
        let factory = PathJailFactory::new(allowlist, false);
        let caps = Capabilities::default(); // all booleans false
        let _adapter = factory.build(&caps);
        // On any platform, default capabilities always fall through to degraded.
        assert_eq!(factory.chosen_tier(), "userspace-degraded");
    }
}
