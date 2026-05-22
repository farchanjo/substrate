//! `CorrelationId` — request-chain correlation identifier.
//!
//! Per `docs/arch/schemas/shared_kernel.cue`: `#CorrelationId` is an alias
//! for `#JobId`. In practice the triple-equality invariant from ADR-0040 means
//! a `CorrelationId` value IS the `JobId` — no mapping table is needed.

use crate::value_objects::job_id::JobId;

/// An alias for [`JobId`] used when the job ID serves as the request-chain
/// correlation identifier per ADR-0038 and ADR-0040.
///
/// Using a type alias rather than a newtype preserves the ADR-0040
/// triple-equality invariant: `job_id == progressToken == correlation_id`.
pub type CorrelationId = JobId;
