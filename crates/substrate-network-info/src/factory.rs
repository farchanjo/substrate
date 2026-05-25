//! Platform capability factory for the network-info adapter (ADR-0058).
//!
//! [`NetworkInfoFactory::build`] probes the current platform and returns the
//! best available [`NetworkInfoPort`] implementation together with the selected
//! [`NetworkInfoTier`] variant. The composition root calls this once at startup
//! and wires the resulting `Arc<dyn NetworkInfoPort>` into the MCP handler.
//!
//! The factory emits a `SUBSTRATE_CAPABILITY_TIERS_SELECTED` audit trace event
//! (target `substrate_audit`) so operators can verify which tier is active.

use std::sync::Arc;

use substrate_domain::ports::network_info::{NetworkInfoPort, NoopNetworkInfoPort};
use tracing::info;

// ---- Tier -------------------------------------------------------------------

/// Indicates which network-info implementation was selected at runtime.
///
/// Logged at startup as part of the `SUBSTRATE_CAPABILITY_TIERS_SELECTED`
/// audit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkInfoTier {
    /// macOS kernel TCP PCB list via `sysctlbyname("net.inet.tcp.pcblist_n")`.
    MacosSysctl,
    /// Linux `/proc/net/{tcp,tcp6,udp,udp6}` + `/proc/net/snmp` text parser.
    LinuxProcNet,
    /// Platform not supported; all methods return `InternalError`.
    Unsupported,
}

// ---- Factory ----------------------------------------------------------------

/// Builds the platform-appropriate [`NetworkInfoPort`] adapter.
pub struct NetworkInfoFactory;

impl NetworkInfoFactory {
    /// Returns the best-fit adapter and the selected tier.
    ///
    /// The selection logic:
    ///
    /// 1. **macOS** — probes `sysctlbyname("net.inet.tcp.stats")`. If the call
    ///    succeeds, returns [`MacosSysctlAdapter`](crate::macos::MacosSysctlAdapter).
    /// 2. **Linux** — unconditionally returns
    ///    [`LinuxProcNetAdapter`](crate::linux::LinuxProcNetAdapter) (procfs is
    ///    always available on supported kernels ≥ 2.6).
    /// 3. **Other** — returns [`NoopNetworkInfoPort`].
    ///
    /// Emits `SUBSTRATE_CAPABILITY_TIERS_SELECTED` to the `substrate_audit`
    /// tracing target so the composition root log shows which tier is active.
    #[must_use]
    pub fn build() -> (Arc<dyn NetworkInfoPort>, NetworkInfoTier) {
        #[cfg(target_os = "macos")]
        {
            let tier = if crate::macos::probe_sysctl() {
                NetworkInfoTier::MacosSysctl
            } else {
                NetworkInfoTier::Unsupported
            };
            let port: Arc<dyn NetworkInfoPort> = match tier {
                NetworkInfoTier::MacosSysctl => {
                    Arc::new(crate::macos::MacosSysctlAdapter::default())
                }
                _ => Arc::new(NoopNetworkInfoPort::default()),
            };
            info!(
                target: "substrate_audit",
                event = "SUBSTRATE_CAPABILITY_TIERS_SELECTED",
                net_info_tier = ?tier,
            );
            return (port, tier);
        }

        #[cfg(target_os = "linux")]
        {
            let tier = NetworkInfoTier::LinuxProcNet;
            let port: Arc<dyn NetworkInfoPort> =
                Arc::new(crate::linux::LinuxProcNetAdapter::default());
            info!(
                target: "substrate_audit",
                event = "SUBSTRATE_CAPABILITY_TIERS_SELECTED",
                net_info_tier = ?tier,
            );
            return (port, tier);
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let tier = NetworkInfoTier::Unsupported;
            info!(
                target: "substrate_audit",
                event = "SUBSTRATE_CAPABILITY_TIERS_SELECTED",
                net_info_tier = ?tier,
            );
            (Arc::new(NoopNetworkInfoPort::default()), tier)
        }
    }
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{NetworkInfoFactory, NetworkInfoTier};

    #[test]
    fn build_returns_correct_tier_for_platform() {
        let (_port, tier) = NetworkInfoFactory::build();

        #[cfg(target_os = "macos")]
        assert!(
            matches!(tier, NetworkInfoTier::MacosSysctl | NetworkInfoTier::Unsupported),
            "macOS should select MacosSysctl or Unsupported, got {tier:?}",
        );

        #[cfg(target_os = "linux")]
        assert_eq!(tier, NetworkInfoTier::LinuxProcNet);

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        assert_eq!(tier, NetworkInfoTier::Unsupported);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn build_selects_sysctl_on_real_macos() {
        let (_port, tier) = NetworkInfoFactory::build();
        assert_eq!(
            tier,
            NetworkInfoTier::MacosSysctl,
            "sysctl probe should succeed on a real macOS machine"
        );
    }
}
