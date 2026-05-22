//! Helpers for constructing the `Hints` map returned with every text tool
//! response.
//!
//! Key groups follow ADR-0007 (tool-card keys) and ADR-0043 (SIMD tier
//! annotation via `simd_tier_used`). Only non-default keys are set;
//! `serde` `skip_serializing_if = "Option::is_none"` ensures absent keys
//! never reach the wire.

use substrate_domain::{Hints, SimdTier};

/// Builds a `Hints` map for successful `text.search` responses.
///
/// The `simd_tier` argument reflects the tier that was active at handler
/// invocation time, sourced from `TextDeps::capabilities.simd_tier`.
#[must_use]
pub fn build_search_hints(simd_tier: SimdTier, has_more_pages: bool) -> Hints {
    let next_action = if has_more_pages {
        Some("Call text.search again with the next_cursor value to retrieve additional matches.")
    } else {
        Some("Inspect matched lines; use fs.read for full context if needed.")
    };

    Hints {
        next_action_suggested: next_action.map(ToOwned::to_owned),
        alternative_tool: Some("fs.read".to_owned()),
        confirm_destructive: Some(false),
        simd_tier_used: Some(simd_tier_label(simd_tier)),
        ..Hints::default()
    }
}

/// Builds a `Hints` map for successful `text.count_lines` responses.
#[must_use]
pub fn build_count_lines_hints(simd_tier: SimdTier) -> Hints {
    Hints {
        next_action_suggested: Some(
            "Use text.search to retrieve matching lines, or text.head/tail for inspection."
                .to_owned(),
        ),
        alternative_tool: Some("text.search".to_owned()),
        confirm_destructive: Some(false),
        simd_tier_used: Some(simd_tier_label(simd_tier)),
        ..Hints::default()
    }
}

/// Builds a `Hints` map for successful `text.head` responses.
#[must_use]
pub fn build_head_hints(simd_tier: SimdTier) -> Hints {
    Hints {
        next_action_suggested: Some(
            "Use text.tail to inspect the end of the file, or text.search to locate patterns."
                .to_owned(),
        ),
        alternative_tool: Some("text.tail".to_owned()),
        confirm_destructive: Some(false),
        simd_tier_used: Some(simd_tier_label(simd_tier)),
        ..Hints::default()
    }
}

/// Builds a `Hints` map for successful `text.tail` responses.
#[must_use]
pub fn build_tail_hints(simd_tier: SimdTier) -> Hints {
    Hints {
        next_action_suggested: Some(
            "Use text.head to inspect the beginning of the file, or text.search for patterns."
                .to_owned(),
        ),
        alternative_tool: Some("text.head".to_owned()),
        confirm_destructive: Some(false),
        simd_tier_used: Some(simd_tier_label(simd_tier)),
        ..Hints::default()
    }
}

/// Builds a minimal `Hints` map for error responses.
#[must_use]
pub fn build_error_hints(recovery: &str) -> Hints {
    Hints {
        error_recovery: Some(recovery.to_owned()),
        confirm_destructive: Some(false),
        ..Hints::default()
    }
}

/// Returns the canonical lowercase string label for a SIMD tier.
///
/// These labels match the `#SimdTier` enum values in
/// `docs/arch/schemas/simd_capability.cue` and the audit event field
/// `simd_tier_used` from ADR-0043.
#[must_use]
pub fn simd_tier_label(tier: SimdTier) -> String {
    match tier {
        SimdTier::Avx512 => "avx512",
        SimdTier::Avx2 => "avx2",
        SimdTier::Sse42 => "sse42",
        SimdTier::Sse2 => "sse2",
        SimdTier::Neon => "neon",
        SimdTier::Portable => "portable",
    }
    .to_owned()
}
