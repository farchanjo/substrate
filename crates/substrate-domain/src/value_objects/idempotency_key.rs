//! `IdempotencyKey` — client-generated deduplication token for job submissions.
//!
//! Mirrors `#IdempotencyKey` in `docs/arch/schemas/job.cue`:
//! a `UUIDv7` encoded as Crockford base32 (26 uppercase characters).
//!
//! The deduplication key is `(client_id, tool_name, idempotency_key, blake3_hash_of_args_json)`
//! per ADR-0040. Bounded to `result_ttl_secs` and evicted by the same GC as job entries.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::{SubstrateError, SubstrateResult};
use crate::value_objects::job_id::JobId;

/// A client-generated `UUIDv7` deduplication key for job submissions.
///
/// Structurally identical to [`JobId`] (both are `UUIDv7` Crockford base32)
/// but semantically distinct: a `JobId` is server-assigned; an `IdempotencyKey`
/// is client-supplied and must not be reused across distinct operations.
///
/// Deserialization is routed through [`TryFrom<String>`] via
/// `#[serde(try_from = "String")]`, mirroring [`JobId`]'s dual acceptance of
/// Crockford base32 (26 chars) or standard hyphenated `Uuid` form, and
/// additionally rejects any UUID that is not version 7. A derived
/// `Deserialize` on the raw `Uuid` would accept any UUID version as
/// pre-validated, contradicting this type's documented `UUIDv7` contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct IdempotencyKey(Uuid);

impl TryFrom<String> for IdempotencyKey {
    type Error = SubstrateError;

    fn try_from(s: String) -> SubstrateResult<Self> {
        let uuid = if s.len() == 26 {
            JobId::parse_crockford(&s)?.as_uuid()
        } else {
            s.parse::<Uuid>()
                .map_err(|e| SubstrateError::InvalidArgument {
                    offending_field: "idempotency_key".to_owned(),
                    reason: format!("invalid idempotency_key format: {e}"),
                    correlation_id: None,
                })?
        };
        if uuid.get_version() != Some(uuid::Version::SortRand) {
            return Err(SubstrateError::InvalidArgument {
                offending_field: "idempotency_key".to_owned(),
                reason: "idempotency_key must be a UUIDv7 (got a different UUID version)"
                    .to_owned(),
                correlation_id: None,
            });
        }
        Ok(Self(uuid))
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed, version-4 UUID (hyphenated form). Used to prove that a
    /// syntactically valid UUID of the *wrong version* is still rejected.
    const V4_UUID_STR: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn deserialize_accepts_v7_hyphenated() {
        let key = IdempotencyKey::now_v7();
        let hyphenated = key.as_uuid().to_string();
        let json = format!("\"{hyphenated}\"");
        #[expect(
            clippy::expect_used,
            reason = "test assertion: valid v7 hyphenated form must deserialize"
        )]
        let back: IdempotencyKey =
            serde_json::from_str(&json).expect("valid v7 hyphenated UUID deserializes");
        assert_eq!(back, key);
    }

    #[test]
    fn deserialize_accepts_v7_crockford() {
        let key = IdempotencyKey::now_v7();
        let json = format!("\"{}\"", key.to_crockford());
        #[expect(
            clippy::expect_used,
            reason = "test assertion: valid v7 Crockford form must deserialize"
        )]
        let back: IdempotencyKey =
            serde_json::from_str(&json).expect("valid v7 Crockford UUID deserializes");
        assert_eq!(back, key);
    }

    #[test]
    fn deserialize_rejects_non_v7_hyphenated() {
        let json = format!("\"{V4_UUID_STR}\"");
        let result: Result<IdempotencyKey, _> = serde_json::from_str(&json);
        assert!(
            result.is_err(),
            "a well-formed but non-v7 UUID must be rejected on deserialize"
        );
    }

    #[test]
    fn deserialize_rejects_non_v7_crockford() {
        #[expect(
            clippy::expect_used,
            reason = "test setup: parsing a fixed, valid UUID literal is infallible"
        )]
        let v4: Uuid = V4_UUID_STR.parse().expect("fixed literal is a valid UUID");
        let crockford = JobId::from_uuid(v4).to_crockford();
        let json = format!("\"{crockford}\"");
        let result: Result<IdempotencyKey, _> = serde_json::from_str(&json);
        assert!(
            result.is_err(),
            "a non-v7 UUID encoded as Crockford base32 must still be rejected"
        );
    }

    #[test]
    fn deserialize_rejects_malformed_string() {
        let result: Result<IdempotencyKey, _> = serde_json::from_str(r#""not-a-uuid""#);
        assert!(result.is_err(), "malformed input must be rejected");
    }
}
