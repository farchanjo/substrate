//! MCP `initialize` handler — protocol version negotiation per ADR-0013.
//!
//! Computes the negotiated protocol version per the min(client, preferred)
//! policy and assembles the `InitializeResponse` with experimental substrate
//! capability flags per the ADR-0013 amendment (ADR-0040 + ADR-0043).
//!
//! The experimental block is diagnostic only; clients MUST NOT branch on it.

#![allow(clippy::redundant_pub_crate, reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates")]

use serde_json::json;
use substrate_domain::Capabilities;

/// Minimum protocol version accepted per ADR-0013.
#[allow(dead_code, reason = "Wave B scaffold — used by rmcp initialize handler in Wave D")]
pub(crate) const PROTOCOL_VERSION_MINIMUM: &str = "2025-06-18";

/// Preferred (maximum) protocol version per ADR-0013.
#[allow(dead_code, reason = "Wave B scaffold — used by rmcp initialize handler in Wave D")]
pub(crate) const PROTOCOL_VERSION_PREFERRED: &str = "2025-11-25";

/// Substrate server name declared in `initialize` response.
#[allow(dead_code, reason = "Wave B scaffold — used by rmcp initialize handler in Wave D")]
pub(crate) const SERVER_NAME: &str = "substrate";

/// Substrate server version — sourced from Cargo at compile time.
#[allow(dead_code, reason = "Wave B scaffold — used by rmcp initialize handler in Wave D")]
pub(crate) const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Negotiated MCP protocol version outcome.
#[allow(dead_code, reason = "Wave B scaffold — used by rmcp initialize handler in Wave D")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NegotiatedVersion {
    /// Client version is below minimum; must reject with error `-32600`.
    BelowMinimum,
    /// Client version is in the accepted range; use the returned string.
    Accepted(String),
}

/// Negotiates the protocol version per ADR-0013.
///
/// Returns `BelowMinimum` when `client_version < "2025-06-18"`, or `Accepted`
/// with `min(client_version, "2025-11-25")` otherwise.
///
/// Version comparison uses lexicographic ordering on the YYYY-MM-DD string,
/// which is correct because all version strings are ISO 8601 dates.
// Wave B: called by rmcp initialize handler wired in Wave D.
#[allow(dead_code, reason = "Wave B scaffold — called by rmcp initialize handler in Wave D")]
#[must_use]
pub(crate) fn negotiate_version(client_version: &str) -> NegotiatedVersion {
    if client_version < PROTOCOL_VERSION_MINIMUM {
        return NegotiatedVersion::BelowMinimum;
    }
    let negotiated = if client_version > PROTOCOL_VERSION_PREFERRED {
        PROTOCOL_VERSION_PREFERRED.to_owned()
    } else {
        client_version.to_owned()
    };
    NegotiatedVersion::Accepted(negotiated)
}

/// Builds the `capabilities.experimental.substrate` block per ADR-0013 amendment.
///
/// Includes:
/// - `jobs`: `true` when the async job control-plane is wired (ADR-0040).
/// - `simd_tier`: snapshot of the SIMD tier chosen at startup (ADR-0043).
/// - `platform_tiers`: map of port name → chosen tier string (ADR-0042).
///
/// All values are diagnostic only; clients MUST NOT make behavioral decisions
/// based on them.
// Wave B: called by rmcp initialize handler wired in Wave D.
#[allow(dead_code, reason = "Wave B scaffold — called by rmcp initialize handler in Wave D")]
#[must_use]
pub(crate) fn build_experimental_capabilities(
    caps: &Capabilities,
    jobs_wired: bool,
) -> serde_json::Value {
    json!({
        "substrate": {
            "jobs": jobs_wired,
            "simd_tier": format!("{:?}", caps.simd_tier).to_lowercase(),
            "platform_tiers": {
                "walker": format!("{:?}", caps.walker_tier),
                "watcher": format!("{:?}", caps.watcher_tier),
                "jail": format!("{:?}", caps.jail_tier),
                "hash": format!("{:?}", caps.hash_tier),
                "stat": format!("{:?}", caps.stat_tier),
            }
        }
    })
}

// ---- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_below_minimum_is_rejected() {
        assert_eq!(
            negotiate_version("2025-03-26"),
            NegotiatedVersion::BelowMinimum
        );
    }

    #[test]
    fn version_at_minimum_is_accepted() {
        assert_eq!(
            negotiate_version("2025-06-18"),
            NegotiatedVersion::Accepted("2025-06-18".to_owned())
        );
    }

    #[test]
    fn version_at_preferred_is_accepted() {
        assert_eq!(
            negotiate_version("2025-11-25"),
            NegotiatedVersion::Accepted("2025-11-25".to_owned())
        );
    }

    #[test]
    fn version_above_preferred_caps_at_preferred() {
        assert_eq!(
            negotiate_version("2026-03-01"),
            NegotiatedVersion::Accepted("2025-11-25".to_owned())
        );
    }
}
