//! Helpers for building the `Hints` map returned with every archive tool response.
//!
//! Archive tools (Bucket C) always include a `job_id` in their hints and set
//! `confirm_destructive = true` for create/extract operations. The `archive.hash`
//! tool sets `confirm_destructive = false` as it is read-only.

use substrate_domain::{Capabilities, Hints};

/// Builds hints for a Bucket-C archive create/extract tool.
///
/// `job_id` is the `UUIDv7` string of the dispatched async job.
/// `simd_tier_used` is sourced from `caps.simd_tier`.
#[must_use]
pub fn build_job_hints(
    job_id: Option<&str>,
    next_action: Option<&'static str>,
    caps: &Capabilities,
    destructive: bool,
) -> Hints {
    Hints {
        next_action_suggested: next_action.map(ToOwned::to_owned),
        confirm_destructive: Some(destructive),
        simd_tier_used: Some(format!("{:?}", caps.simd_tier).to_lowercase()),
        job_id: job_id.map(ToOwned::to_owned),
        job_state: job_id.map(|_| "queued".to_owned()),
        polling_endpoint: job_id.map(|_| "job.result".to_owned()),
        ..Hints::default()
    }
}

/// Builds hints for a Bucket-B archive compress/decompress/hash tool.
///
/// `simd_tier_used` is sourced from `caps.simd_tier`.
#[must_use]
pub fn build_inline_hints(
    next_action: Option<&'static str>,
    alternative: Option<&'static str>,
    caps: &Capabilities,
    destructive: bool,
) -> Hints {
    Hints {
        next_action_suggested: next_action.map(ToOwned::to_owned),
        alternative_tool: alternative.map(ToOwned::to_owned),
        confirm_destructive: Some(destructive),
        simd_tier_used: Some(format!("{:?}", caps.simd_tier).to_lowercase()),
        ..Hints::default()
    }
}
