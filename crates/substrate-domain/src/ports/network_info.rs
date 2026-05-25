//! `NetworkInfoPort` — inbound port for the network-info bounded context per ADR-0058.
//!
//! Implemented by the `substrate-network-info` adapter crate (behind the
//! `network-info` Cargo feature). The composition root wires an
//! `Arc<dyn NetworkInfoPort>` when the feature is active, or a
//! [`NoopNetworkInfoPort`] Null Object when disabled or running on an
//! unsupported platform.
//!
//! References: ADR-0058.

use async_trait::async_trait;

use crate::errors::SubstrateError;
use crate::network::{
    ConnectionCounts, NetworkTcpListRequest, NetworkTcpListResult, NetworkUdpListRequest,
    NetworkUdpListResult, TcpStats,
};

// ---- NetworkInfoPort --------------------------------------------------------

/// Inbound port for the network-info bounded context per ADR-0058.
///
/// Adapter implementations live in `substrate-network-info` (gated behind the
/// `network-info` Cargo feature). Domain code and MCP tool handlers depend only
/// on this trait.
///
/// All `async fn` methods are cancel-safe at the `await` boundary per ADR-0037.
/// Adapters MUST check the cancellation token inside `tokio::select! biased`
/// with the work arm first.
#[async_trait]
pub trait NetworkInfoPort: Send + Sync {
    /// Returns a paginated list of TCP socket entries from the OS network stack.
    ///
    /// When `req.state_filter` is `Some`, only entries in the listed states are
    /// returned. Callers MUST invoke [`NetworkTcpListRequest::validate`] before
    /// calling; adapters MAY call it again as a defense-in-depth measure.
    ///
    /// # Errors
    ///
    /// - [`SubstrateError::InvalidArgument`] — request validation failed.
    /// - [`SubstrateError::PermissionDenied`] — OS denied access to socket table.
    /// - [`SubstrateError::InternalError`] — platform-specific failure.
    async fn list_tcp(
        &self,
        req: NetworkTcpListRequest,
    ) -> Result<NetworkTcpListResult, SubstrateError>;

    /// Returns a paginated list of UDP socket entries from the OS network stack.
    ///
    /// Callers MUST invoke [`NetworkUdpListRequest::validate`] before calling;
    /// adapters MAY call it again as a defense-in-depth measure.
    ///
    /// # Errors
    ///
    /// - [`SubstrateError::InvalidArgument`] — request validation failed.
    /// - [`SubstrateError::PermissionDenied`] — OS denied access to socket table.
    /// - [`SubstrateError::InternalError`] — platform-specific failure.
    async fn list_udp(
        &self,
        req: NetworkUdpListRequest,
    ) -> Result<NetworkUdpListResult, SubstrateError>;

    /// Returns cumulative TCP protocol statistics for the host.
    ///
    /// On macOS counters come from `sysctl net.inet.tcp.stats`; on Linux from
    /// `/proc/net/snmp` (the `Tcp:` and `TcpExt:` rows).
    ///
    /// # Errors
    ///
    /// - [`SubstrateError::PermissionDenied`] — OS denied access.
    /// - [`SubstrateError::InternalError`] — platform-specific failure.
    async fn tcp_stats(&self) -> Result<TcpStats, SubstrateError>;

    /// Returns per-state TCP connection counts for the host.
    ///
    /// This is a cheaper alternative to `list_tcp` when only aggregate counts
    /// are needed — adapters SHOULD avoid allocating full entry lists.
    ///
    /// # Errors
    ///
    /// - [`SubstrateError::PermissionDenied`] — OS denied access.
    /// - [`SubstrateError::InternalError`] — platform-specific failure.
    async fn connection_count(&self) -> Result<ConnectionCounts, SubstrateError>;
}

// ---- NoopNetworkInfoPort ----------------------------------------------------

/// Null-Object implementation of [`NetworkInfoPort`].
///
/// Returns [`SubstrateError::InternalError`] for every call. Wired by the
/// composition root on platforms where the network-info adapter is unavailable
/// (Windows, WASI, or when the `network-info` feature is disabled).
///
/// # Example
///
/// ```rust
/// use substrate_domain::ports::network_info::{NetworkInfoPort, NoopNetworkInfoPort};
/// use substrate_domain::network::NetworkTcpListRequest;
///
/// let port = NoopNetworkInfoPort;
/// // In async context: port.list_tcp(req).await would return Err(InternalError {...})
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopNetworkInfoPort;

#[async_trait]
impl NetworkInfoPort for NoopNetworkInfoPort {
    async fn list_tcp(
        &self,
        _req: NetworkTcpListRequest,
    ) -> Result<NetworkTcpListResult, SubstrateError> {
        Err(SubstrateError::InternalError {
            reason: "network-info not supported on this platform".to_string(),
            correlation_id: None,
        })
    }

    async fn list_udp(
        &self,
        _req: NetworkUdpListRequest,
    ) -> Result<NetworkUdpListResult, SubstrateError> {
        Err(SubstrateError::InternalError {
            reason: "network-info not supported on this platform".to_string(),
            correlation_id: None,
        })
    }

    async fn tcp_stats(&self) -> Result<TcpStats, SubstrateError> {
        Err(SubstrateError::InternalError {
            reason: "network-info not supported on this platform".to_string(),
            correlation_id: None,
        })
    }

    async fn connection_count(&self) -> Result<ConnectionCounts, SubstrateError> {
        Err(SubstrateError::InternalError {
            reason: "network-info not supported on this platform".to_string(),
            correlation_id: None,
        })
    }
}
