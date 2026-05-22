//! `IdempotencyKey` — client-generated deduplication token for job submissions.
//!
//! Mirrors `#IdempotencyKey` in `docs/arch/schemas/job.cue`:
//! a `UUIDv7` encoded as Crockford base32 (26 uppercase characters).
//!
//! The deduplication key is `(client_id, tool_name, idempotency_key, blake3_hash_of_args_json)`
//! per ADR-0040. Bounded to `result_ttl_secs` and evicted by the same GC as job entries.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::SubstrateResult;
use crate::value_objects::job_id::JobId;

/// A client-generated `UUIDv7` deduplication key for job submissions.
///
/// Structurally identical to [`JobId`] (both are `UUIDv7` Crockford base32)
/// but semantically distinct: a `JobId` is server-assigned; an `IdempotencyKey`
/// is client-supplied and must not be reused across distinct operations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdempotencyKey(Uuid);

impl IdempotencyKey {
    /// Generates a new idempotency key using `UUIDv7`.
    #[must_use]
    pub fn now_v7() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wraps an existing [`Uuid`].
    #[must_use]
    pub const fn from_uuid(u: Uuid) -> Self {
        Self(u)
    }

    /// Returns the inner [`Uuid`].
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Parses a 26-character Crockford base32 string.
    ///
    /// Delegates to `JobId::parse_crockford` for the shared encoding rules.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::SubstrateError::InvalidArgument`] on malformed input.
    pub fn parse_crockford(s: &str) -> SubstrateResult<Self> {
        let job_id = JobId::parse_crockford(s)?;
        Ok(Self(job_id.as_uuid()))
    }

    /// Encodes as a 26-character Crockford base32 string.
    #[must_use]
    pub fn to_crockford(&self) -> String {
        JobId::from_uuid(self.0).to_crockford()
    }
}

impl std::fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_crockford())
    }
}
