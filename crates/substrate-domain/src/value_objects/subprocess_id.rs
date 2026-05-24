//! `SubprocessId` — uniquely identifies a spawned subprocess within the job registry.
//!
//! Mirrors [`JobId`] structurally: a `UUIDv7` generated at spawn time.
//! By convention `SubprocessId` and the corresponding `JobId` share the same
//! UUID value — a single `JobId::now_v7()` call is used for both at spawn time.
//!
//! References: ADR-0052 (subprocess BC), ADR-0040 (job control-plane identity).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::SubstrateResult;
use crate::value_objects::job_id::JobId;

/// A `UUIDv7` that uniquely identifies a subprocess spawn invocation.
///
/// Structurally identical to [`JobId`] (both wrap a `UUIDv7`) but semantically
/// scoped to the subprocess bounded context. The value equals the `JobId`
/// assigned to the corresponding `JobEntry`, enabling correlation across the
/// job control-plane, MCP `progressToken`, and subprocess-specific audit events.
///
/// See ADR-0052 §"`SubprocessHandle`" and ADR-0040 §"triple-equality invariant".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubprocessId(Uuid);

impl SubprocessId {
    /// Generates a new `SubprocessId` using `UUIDv7` (time-ordered).
    #[must_use]
    pub fn now_v7() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wraps an existing [`Uuid`] as a `SubprocessId`.
    #[must_use]
    pub const fn from_uuid(u: Uuid) -> Self {
        Self(u)
    }

    /// Returns the inner [`Uuid`].
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Converts this `SubprocessId` to the equivalent [`JobId`].
    ///
    /// By ADR-0052 convention, `subprocess_id.as_job_id()` returns the same
    /// `JobId` as the one stored in the corresponding `SubprocessHandle.job_id`.
    #[must_use]
    pub const fn as_job_id(&self) -> JobId {
        JobId::from_uuid(self.0)
    }

    /// Parses a 26-character Crockford base32 string into a `SubprocessId`.
    ///
    /// Uses the same Crockford base32 encoding as [`JobId`].
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

impl std::fmt::Display for SubprocessId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_crockford())
    }
}

impl std::str::FromStr for SubprocessId {
    type Err = crate::errors::SubstrateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Accept Crockford base32 (26 chars) or standard UUID hyphenated format.
        if s.len() == 26 {
            return Self::parse_crockford(s);
        }
        s.parse::<Uuid>().map(Self::from_uuid).map_err(|e| {
            crate::errors::SubstrateError::InvalidArgument {
                offending_field: "subprocess_id".to_owned(),
                reason: format!("invalid subprocess_id format: {e}"),
                correlation_id: None,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_crockford() {
        let id = SubprocessId::now_v7();
        let s = id.to_crockford();
        assert_eq!(
            s.len(),
            26,
            "Crockford encoding must produce exactly 26 chars"
        );
        #[expect(
            clippy::expect_used,
            reason = "test assertion: parse_crockford of a freshly encoded string is infallible"
        )]
        let parsed = SubprocessId::parse_crockford(&s).expect("round-trip must succeed");
        assert_eq!(id, parsed);
    }

    #[test]
    fn job_id_round_trip() {
        let id = SubprocessId::now_v7();
        let job_id = id.as_job_id();
        assert_eq!(id.as_uuid(), job_id.as_uuid());
    }
}
