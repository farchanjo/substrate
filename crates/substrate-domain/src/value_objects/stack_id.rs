//! `StackId` — uniquely identifies a running launch Stack instance.
//!
//! Mirrors `#Stack.stack_id` in `docs/arch/schemas/launch.cue`: a `UUIDv7`
//! encoded as Crockford base32 (26 uppercase characters, `^[0-9A-HJKMNP-TV-Z]{26}$`).
//! Structurally identical to [`JobId`] and [`SubprocessId`]; semantically scoped
//! to the launch bounded context (ADR-0063).
//!
//! References: ADR-0063 (launch orchestration BC), ADR-0040 (`UUIDv7` identity).

use uuid::Uuid;

use crate::errors::{SubstrateError, SubstrateResult};
use crate::value_objects::job_id::JobId;

/// A `UUIDv7` that uniquely identifies a running launch Stack instance.
///
/// Reuses the Crockford base32 codec from [`JobId`] (26 uppercase chars) so the
/// wire representation matches the `#Stack.stack_id` CUE pattern. The launch BC
/// uses a distinct newtype to keep stack identity separate from job identity.
///
/// Serialization: Crockford base32 string (26 uppercase chars), matching
/// [`JobId`]'s `Serialize` impl -- a `#[derive(Serialize)]` on this newtype
/// would silently delegate to the inner [`Uuid`]'s own serde impl (standard
/// hyphenated form), diverging from this type's own [`std::fmt::Display`] and
/// from `#Stack.stack_id`'s documented wire contract.
///
/// See ADR-0063 §"`#Stack`".
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StackId(Uuid);

impl StackId {
    /// Generates a new `StackId` using `UUIDv7` (time-ordered).
    #[must_use]
    pub fn now_v7() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wraps an existing [`Uuid`] as a `StackId`.
    #[must_use]
    pub const fn from_uuid(u: Uuid) -> Self {
        Self(u)
    }

    /// Returns the inner [`Uuid`].
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Parses a 26-character Crockford base32 string into a `StackId`.
    ///
    /// Uses the same Crockford base32 encoding as [`JobId`].
    ///
    /// # Errors
    ///
    /// Returns [`SubstrateError::InvalidArgument`] on malformed input (wrong
    /// length or a character outside the Crockford alphabet).
    pub fn parse_crockford(s: &str) -> SubstrateResult<Self> {
        JobId::parse_crockford(s)
            .map(|j| Self(j.as_uuid()))
            .map_err(|_| SubstrateError::InvalidArgument {
                offending_field: "stack_id".to_owned(),
                reason: format!("stack_id must be 26 Crockford base32 chars; got '{s}'"),
                correlation_id: None,
            })
    }

    /// Encodes this `StackId` as a 26-character Crockford base32 string.
    #[must_use]
    pub fn to_crockford(&self) -> String {
        JobId::from_uuid(self.0).to_crockford()
    }
}

impl std::fmt::Display for StackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_crockford())
    }
}

impl std::str::FromStr for StackId {
    type Err = SubstrateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Accept Crockford base32 (26 chars) or standard UUID hyphenated format.
        if s.len() == 26 {
            return Self::parse_crockford(s);
        }
        s.parse::<Uuid>()
            .map(Self::from_uuid)
            .map_err(|e| SubstrateError::InvalidArgument {
                offending_field: "stack_id".to_owned(),
                reason: format!("invalid stack_id format: {e}"),
                correlation_id: None,
            })
    }
}

impl serde::Serialize for StackId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_crockford())
    }
}

impl<'de> serde::Deserialize<'de> for StackId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        s.parse::<Self>().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_crockford() {
        let id = StackId::now_v7();
        let s = id.to_crockford();
        assert_eq!(s.len(), 26, "Crockford encoding must produce exactly 26 chars");
        #[expect(
            clippy::expect_used,
            reason = "test assertion: parse_crockford of a freshly encoded string is infallible"
        )]
        let parsed = StackId::parse_crockford(&s).expect("round-trip must succeed");
        assert_eq!(id, parsed);
    }

    #[test]
    fn bad_char_rejected() {
        // 'U' is excluded from the Crockford alphabet.
        let err = StackId::parse_crockford("UUUUUUUUUUUUUUUUUUUUUUUUUU");
        assert!(matches!(err, Err(SubstrateError::InvalidArgument { .. })));
    }

    #[test]
    fn wrong_length_rejected() {
        assert!(matches!(
            StackId::parse_crockford("TOOSHORT"),
            Err(SubstrateError::InvalidArgument { .. })
        ));
    }
}
