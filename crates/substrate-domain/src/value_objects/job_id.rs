//! `JobId` — uniquely identifies an async job.
//!
//! Mirrors `#JobId` in `docs/arch/schemas/job.cue`:
//! a `UUIDv7` encoded as Crockford base32 (26 uppercase characters).
//!
//! Per ADR-0040 triple-equality invariant: `job_id == progressToken == correlation_id`.
//! The Crockford base32 encoding is time-ordered and monotonic within a millisecond.

use serde::{Deserializer, Serialize};
use uuid::Uuid;

use crate::errors::{SubstrateError, SubstrateResult};

/// Crockford base32 alphabet (uppercase, no I/L/O/U per the spec).
const CROCKFORD_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A `UUIDv7` encoded as a 26-character Crockford base32 string.
///
/// Implements the ADR-0040 triple-equality invariant: a `JobId` is simultaneously
/// the job identifier, the MCP `progressToken`, and the `correlation_id` for
/// the request chain.
///
/// Serialization: Crockford base32 string (26 uppercase chars).
/// Deserialization: accepts Crockford base32 (26 chars) or standard UUID
/// hyphenated format (for interoperability with clients that use the UUID form).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobId(Uuid);

impl JobId {
    /// Generates a new `JobId` using a `UUIDv7` (time-ordered).
    ///
    /// Monotonicity within a millisecond is guaranteed by the `UUIDv7` spec.
    #[must_use]
    pub fn now_v7() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wraps an existing [`Uuid`] as a `JobId`.
    ///
    /// The caller is responsible for ensuring `u` is a valid `UUIDv7` when
    /// strict monotonicity matters; the domain does not validate the version.
    #[must_use]
    pub const fn from_uuid(u: Uuid) -> Self {
        Self(u)
    }

    /// Returns the inner [`Uuid`].
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Encodes the UUID as a 26-character Crockford base32 string.
    ///
    /// Encoding is performed on the 128-bit UUID value using the Crockford
    /// alphabet (no I/L/O/U). The result is always exactly 26 uppercase characters.
    ///
    /// # Panics
    ///
    /// Never panics in practice: the internal byte buffer is constructed exclusively
    /// from the `CROCKFORD_ALPHABET` constant (7-bit ASCII), making the
    /// `from_utf8` call infallible. The `expect` is present to satisfy the type
    /// system and is documented with a SAFETY comment.
    #[must_use]
    pub fn to_crockford(&self) -> String {
        let bytes = self.0.as_u128();
        // 128 bits / 5 bits per Crockford char = 25.6 -> ceil = 26 chars.
        let mut out = [0u8; 26];
        let mut n = bytes;
        for slot in out.iter_mut().rev() {
            *slot = CROCKFORD_ALPHABET[(n & 0x1F) as usize];
            n >>= 5;
        }
        // All bytes are drawn from `CROCKFORD_ALPHABET`, a 7-bit ASCII literal array.
        // The `from_utf8` call is therefore provably infallible; the `#[expect]`
        // silences `clippy::expect_used` without suppressing the safety comment.
        #[expect(
            clippy::expect_used,
            reason = "infallible: bytes come from a 7-bit ASCII constant array CROCKFORD_ALPHABET"
        )]
        String::from_utf8(out.to_vec()).expect("Crockford alphabet is valid ASCII")
    }

    /// Parses a 26-character Crockford base32 string back into a `JobId`.
    ///
    /// # Errors
    ///
    /// Returns [`SubstrateError::InvalidArgument`] when the string is not exactly
    /// 26 characters of the Crockford base32 alphabet.
    pub fn parse_crockford(s: &str) -> SubstrateResult<Self> {
        if s.len() != 26 {
            return Err(SubstrateError::InvalidArgument {
                offending_field: "job_id".to_owned(),
                reason: format!("job_id must be 26 Crockford base32 chars; got {}", s.len()),
                correlation_id: None,
            });
        }
        let mut value: u128 = 0;
        for c in s.bytes() {
            let digit = crockford_digit(c).ok_or_else(|| SubstrateError::InvalidArgument {
                offending_field: "job_id".to_owned(),
                reason: format!("invalid Crockford character: '{}'", c as char),
                correlation_id: None,
            })?;
            value = value
                .checked_shl(5)
                .ok_or_else(|| SubstrateError::InternalError {
                    reason: "Crockford decode overflow".to_owned(),
                    correlation_id: None,
                })?
                | u128::from(digit);
        }
        Ok(Self(Uuid::from_u128(value)))
    }
}

/// Maps a Crockford base32 byte to its 5-bit digit value.
///
/// Accepts both uppercase and lowercase input via `to_ascii_uppercase`.
/// Confusable characters are mapped per the Crockford spec: I/L → 1, O → 0.
const fn crockford_digit(b: u8) -> Option<u8> {
    // Accept both uppercase and lowercase; map common confusable chars.
    let b = b.to_ascii_uppercase();
    match b {
        b'0' | b'O' => Some(0),        // O -> 0 (confusable)
        b'1' | b'I' | b'L' => Some(1), // I -> 1, L -> 1 (confusables)
        b'2' => Some(2),
        b'3' => Some(3),
        b'4' => Some(4),
        b'5' => Some(5),
        b'6' => Some(6),
        b'7' => Some(7),
        b'8' => Some(8),
        b'9' => Some(9),
        b'A' => Some(10),
        b'B' => Some(11),
        b'C' => Some(12),
        b'D' => Some(13),
        b'E' => Some(14),
        b'F' => Some(15),
        b'G' => Some(16),
        b'H' => Some(17),
        b'J' => Some(18),
        b'K' => Some(19),
        b'M' => Some(20),
        b'N' => Some(21),
        b'P' => Some(22),
        b'Q' => Some(23),
        b'R' => Some(24),
        b'S' => Some(25),
        b'T' => Some(26),
        // U excluded from alphabet
        b'V' => Some(27),
        b'W' => Some(28),
        b'X' => Some(29),
        b'Y' => Some(30),
        b'Z' => Some(31),
        _ => None,
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_crockford())
    }
}

impl Serialize for JobId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_crockford())
    }
}

impl<'de> serde::Deserialize<'de> for JobId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        // Accept Crockford base32 (26 chars) — primary format.
        if s.len() == 26 {
            return Self::parse_crockford(&s).map_err(serde::de::Error::custom);
        }
        // Accept standard UUID hyphenated or compact format for interoperability.
        s.parse::<Uuid>()
            .map(Self::from_uuid)
            .map_err(|e| serde::de::Error::custom(format!("invalid job_id format: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_crockford() {
        let id = JobId::now_v7();
        let s = id.to_crockford();
        assert_eq!(s.len(), 26, "Crockford encoding must be 26 chars");
        #[expect(
            clippy::expect_used,
            reason = "test assertion: parse_crockford of a just-encoded string is infallible"
        )]
        let parsed = JobId::parse_crockford(&s).expect("round-trip must succeed");
        assert_eq!(id, parsed);
    }

    #[test]
    fn invalid_length_rejected() {
        assert!(JobId::parse_crockford("TOOSHORT").is_err());
    }
}
