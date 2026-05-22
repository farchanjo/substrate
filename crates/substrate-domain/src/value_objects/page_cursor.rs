//! `PageCursor` — opaque, base64url-encoded pagination token per ADR-0008.
//!
//! The domain layer holds raw cursor bytes and provides typed access.
//! Adapters handle base64url encoding/decoding at the boundary (using
//! `base64-simd` or similar) so the domain remains encoding-agnostic.
//!
//! Clients MUST treat the cursor payload as opaque; never construct cursors
//! manually. Cursors are valid only for the issuing tool and page size.

use serde::{Deserialize, Serialize};

/// An opaque pagination cursor carrying raw continuation state.
///
/// `PageCursor` is a thin new-type around `Vec<u8>`. It carries no
/// encoding — base64url encoding for the wire format happens in
/// `substrate-mcp-server` at the MCP boundary.
///
/// # ADR-0008 Contract
///
/// - Page size default: 50; max: 500.
/// - Cursors are opaque to callers; never parse the inner bytes.
/// - Cursors expire when the underlying data changes significantly.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PageCursor(Vec<u8>);

impl PageCursor {
    /// Constructs a cursor from raw continuation bytes.
    ///
    /// Adapters call this after decoding a base64url token from the wire.
    #[must_use]
    pub const fn from_bytes(b: Vec<u8>) -> Self {
        Self(b)
    }

    /// Returns the raw cursor bytes.
    ///
    /// Adapters call this when encoding the cursor for the wire.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the cursor and returns the raw bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}
