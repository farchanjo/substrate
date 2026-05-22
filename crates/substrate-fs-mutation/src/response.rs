//! Shared response types and dependency bundle for all fs-mutation tool handlers.
//!
//! [`FsMutationDeps`] is the single struct threaded through every handler so that
//! the composition root (`substrate-mcp-server`) controls lifetime and wiring.

use std::sync::Arc;

use substrate_domain::{Capabilities, Hints, PathJailPort};

#[cfg(feature = "fs-index")]
use substrate_domain::FsIndexPort;

/// Dependency bundle for all fs-mutation tool handlers.
///
/// The composition root constructs this once and shares it across concurrent
/// handler invocations via `Arc<FsMutationDeps>`.
#[derive(Clone)]
pub struct FsMutationDeps {
    /// Path-jail adapter — validates all caller-supplied paths.
    pub jail: Arc<dyn PathJailPort>,

    /// Runtime capability snapshot — used to annotate diagnostic tier in hints.
    pub capabilities: Arc<Capabilities>,

    /// Optional filesystem-index port for write-through updates (ADR-0041).
    ///
    /// Present only when the `fs-index` Cargo feature is enabled.
    #[cfg(feature = "fs-index")]
    pub index: Arc<dyn FsIndexPort>,
}

impl std::fmt::Debug for FsMutationDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsMutationDeps")
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

/// The response envelope returned by every fs-mutation handler.
///
/// The composition root (`substrate-mcp-server`) converts this into a
/// proper MCP `CallToolResult` with `content` + `structuredContent`.
#[derive(Debug, Clone)]
pub struct ToolResponse {
    /// Model-oriented text (≤80 tokens per ADR-0007 narrative arc).
    pub content: String,

    /// Programmatic JSON payload for the `structuredContent` field.
    pub structured_content: serde_json::Value,

    /// Structured hints map (ADR-0007 + ADR-0040 extension).
    pub hints: Hints,
}

impl ToolResponse {
    /// Constructs a minimal `ToolResponse` for success paths.
    #[must_use]
    pub fn ok(content: impl Into<String>, structured_content: serde_json::Value) -> Self {
        Self {
            content: content.into(),
            structured_content,
            hints: Hints::default(),
        }
    }

    /// Constructs a `ToolResponse` with explicit hints.
    #[must_use]
    pub fn with_hints(
        content: impl Into<String>,
        structured_content: serde_json::Value,
        hints: Hints,
    ) -> Self {
        Self {
            content: content.into(),
            structured_content,
            hints,
        }
    }
}
