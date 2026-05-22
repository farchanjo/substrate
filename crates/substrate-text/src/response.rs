//! Shared response types and dependency bundle for all text tool handlers.
//!
//! `TextDeps` is the single struct threaded through every handler so the
//! composition root (`substrate-mcp-server`) controls lifetime and wiring.

use std::sync::Arc;

use substrate_domain::{Capabilities, Hints, PathJailPort};

/// Dependency bundle for all text tool handlers.
///
/// The composition root constructs this once and shares it across concurrent
/// handler invocations via `Arc<TextDeps>`.
#[derive(Clone)]
pub struct TextDeps {
    /// Path-jail adapter — validates all caller-supplied paths.
    pub jail: Arc<dyn PathJailPort>,

    /// Runtime capability snapshot — used to annotate `simd_tier_used` in hints.
    pub capabilities: Arc<Capabilities>,
}

impl std::fmt::Debug for TextDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextDeps")
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

/// The response envelope returned by every text handler.
///
/// The composition root (`substrate-mcp-server`) converts this into a proper
/// MCP `CallToolResult` with `content` + `structuredContent`.
#[derive(Debug, Clone)]
pub struct ToolResponse {
    /// Model-oriented text summary (≤80 tokens per ADR-0007 narrative arc).
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
