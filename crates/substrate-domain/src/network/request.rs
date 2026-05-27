//! Request and result value objects for the network-info bounded context.
//!
//! These types mirror the `#NetworkTcpListRequest`, `#NetworkTcpListResult`,
//! `#NetworkUdpListRequest`, and `#NetworkUdpListResult` definitions in
//! `docs/arch/schemas/network.cue` per ADR-0058 §"Wire Shape".
//!
//! References: ADR-0058, ADR-0057 (pagination).

use serde::{Deserialize, Serialize};

use crate::errors::SubstrateError;
use crate::subprocess::pagination::Pagination;

use super::socket::{SocketEntry, TcpState};

// ---- NetworkTcpListRequest --------------------------------------------------

/// Request parameters for `network.tcp_list`.
///
/// When `state_filter` is `None`, all TCP connections are returned. When
/// `Some`, at least one state MUST be listed (an empty vec is rejected by
/// [`validate`](Self::validate)).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkTcpListRequest {
    /// Restrict results to connections in these TCP states.
    ///
    /// `None` returns all states. `Some([])` is rejected as a validation error —
    /// callers that want all states MUST omit the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_filter: Option<Vec<TcpState>>,

    /// When `true`, the adapter attempts to resolve the owning PID for each entry.
    ///
    /// Defaults to `false`. PID resolution requires elevated privileges on some
    /// platforms; when resolution fails for an individual entry, `pid` is `None`.
    #[serde(default)]
    pub resolve_pid: bool,

    /// Optional pagination cursor.
    ///
    /// When `None`, the adapter returns the first page using
    /// [`Pagination::default`](crate::subprocess::pagination::Pagination::default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<Pagination>,
}

impl NetworkTcpListRequest {
    /// Validates the request parameters.
    ///
    /// # Errors
    ///
    /// - [`SubstrateError::InvalidArgument`] when `state_filter` is `Some` but empty.
    /// - [`SubstrateError::InvalidArgument`] when `pagination` fails its own validation.
    pub fn validate(&self) -> Result<(), SubstrateError> {
        if let Some(ref states) = self.state_filter {
            if states.is_empty() {
                return Err(SubstrateError::InvalidArgument {
                    offending_field: "state_filter".to_string(),
                    reason: "state_filter must contain at least one TcpState when set; \
                             omit the field to return all states"
                        .to_string(),
                    correlation_id: None,
                });
            }
        }
        if let Some(ref p) = self.pagination {
            p.validate().map_err(|e| SubstrateError::InvalidArgument {
                offending_field: "pagination".to_string(),
                reason: e.to_string(),
                correlation_id: None,
            })?;
        }
        Ok(())
    }
}

// ---- NetworkTcpListResult ---------------------------------------------------

/// Paginated result returned by `network.tcp_list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkTcpListResult {
    /// The socket entries for this page.
    pub entries: Vec<SocketEntry>,

    /// Total number of TCP entries across all pages (before pagination).
    pub total: u64,

    /// 0-based offset to pass as `pagination.offset` in the next call.
    ///
    /// `None` when this page exhausts the result set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
}

// ---- NetworkUdpListRequest --------------------------------------------------

/// Request parameters for `network.udp_list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkUdpListRequest {
    /// When `true`, the adapter attempts to resolve the owning PID for each entry.
    ///
    /// Defaults to `false`. PID resolution requires elevated privileges on some
    /// platforms; when resolution fails for an individual entry, `pid` is `None`.
    #[serde(default)]
    pub resolve_pid: bool,

    /// Optional pagination cursor.
    ///
    /// When `None`, the adapter returns the first page using
    /// [`Pagination::default`](crate::subprocess::pagination::Pagination::default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<Pagination>,
}

impl NetworkUdpListRequest {
    /// Validates the request parameters.
    ///
    /// # Errors
    ///
    /// - [`SubstrateError::InvalidArgument`] when `pagination` fails its own validation.
    pub fn validate(&self) -> Result<(), SubstrateError> {
        if let Some(ref p) = self.pagination {
            p.validate().map_err(|e| SubstrateError::InvalidArgument {
                offending_field: "pagination".to_string(),
                reason: e.to_string(),
                correlation_id: None,
            })?;
        }
        Ok(())
    }
}

// ---- NetworkUdpListResult ---------------------------------------------------

/// Paginated result returned by `network.udp_list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkUdpListResult {
    /// The socket entries for this page.
    pub entries: Vec<SocketEntry>,

    /// Total number of UDP entries across all pages (before pagination).
    pub total: u64,

    /// 0-based offset to pass as `pagination.offset` in the next call.
    ///
    /// `None` when this page exhausts the result set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ADR-0061: NetworkTcpListRequest / NetworkUdpListRequest Default ----

    /// `NetworkTcpListRequest::default()` must produce a valid request with
    /// no state filter, no PID resolution, and no pagination cursor.
    ///
    /// ADR-0061 requires a manual `Default` impl so the struct is safe to use
    /// with the `is_null() || empty_object` handler shortcut in the future.
    #[test]
    fn network_tcp_list_request_default_is_sensible() {
        let req = NetworkTcpListRequest::default();
        assert!(
            req.state_filter.is_none(),
            "default state_filter should be None (return all states)"
        );
        assert!(!req.resolve_pid, "default resolve_pid should be false");
        assert!(
            req.pagination.is_none(),
            "default pagination should be None"
        );
    }

    /// `NetworkUdpListRequest::default()` must produce a valid request.
    #[test]
    fn network_udp_list_request_default_is_sensible() {
        let req = NetworkUdpListRequest::default();
        assert!(!req.resolve_pid, "default resolve_pid should be false");
        assert!(
            req.pagination.is_none(),
            "default pagination should be None"
        );
    }
}
