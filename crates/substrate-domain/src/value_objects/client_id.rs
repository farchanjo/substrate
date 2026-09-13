//! `ClientId` — identifies the originating MCP client session.
//!
//! Mirrors `#ClientId` in `doc/arch/schemas/shared_kernel.cue`:
//! pattern `^[A-Za-z0-9._-]{1,64}$`.

use serde::{Deserialize, Serialize};

use crate::errors::{SubstrateError, SubstrateResult};

/// Identifies the originating MCP client session per ADR-0040.
///
/// Pattern: alphanumeric with dots, underscores, and hyphens; 1–64 characters.
/// Cross-client visibility is forbidden; each client sees only its own jobs.
///
/// Deserialization is routed through [`ClientId::parse`] via
/// `#[serde(try_from = "String")]` so the `^[A-Za-z0-9._-]{1,64}$` pattern
/// holds for every deserialized value, not just ones built through the
/// constructor. A derived `Deserialize` on the raw `String` would silently
/// accept any string as pre-validated.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ClientId(String);

impl TryFrom<String> for ClientId {
    type Error = SubstrateError;

    fn try_from(s: String) -> SubstrateResult<Self> {
        Self::parse(s)
    }
}

impl ClientId {
    /// The regex pattern enforced at construction time.
    pub const PATTERN: &'static str = r"^[A-Za-z0-9._-]{1,64}$";

    /// Parses and validates a raw string as a `ClientId`.
    ///
    /// # Errors
    ///
    /// Returns [`SubstrateError::InvalidArgument`] when the input does not
    /// match the pattern `^[A-Za-z0-9._-]{1,64}$`.
    pub fn parse(s: impl Into<String>) -> SubstrateResult<Self> {
        let s = s.into();
        if s.is_empty() || s.len() > 64 {
            return Err(SubstrateError::InvalidArgument {
                offending_field: "client_id".to_owned(),
                reason: format!(
                    "client_id must be 1–64 characters; got {} characters",
                    s.len()
                ),
                correlation_id: None,
            });
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            return Err(SubstrateError::InvalidArgument {
                offending_field: "client_id".to_owned(),
                reason: "client_id may only contain [A-Za-z0-9._-]".to_owned(),
                correlation_id: None,
            });
        }
        Ok(Self(s))
    }

    /// Returns the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ClientId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_client_id() {
        assert!(ClientId::parse("mcp-client_1.0").is_ok());
    }

    #[test]
    fn empty_client_id_rejected() {
        assert!(ClientId::parse("").is_err());
    }

    #[test]
    fn too_long_client_id_rejected() {
        assert!(ClientId::parse("a".repeat(65)).is_err());
    }

    #[test]
    fn invalid_chars_rejected() {
        assert!(ClientId::parse("bad/client").is_err());
    }

    #[test]
    fn deserialize_accepts_valid_client_id() {
        #[expect(
            clippy::expect_used,
            reason = "test assertion: valid JSON must deserialize"
        )]
        let id: ClientId =
            serde_json::from_str(r#""mcp-client_1.0""#).expect("valid client_id deserializes");
        assert_eq!(id.as_str(), "mcp-client_1.0");
    }

    #[test]
    fn deserialize_rejects_invalid_chars() {
        let result: Result<ClientId, _> = serde_json::from_str(r#""bad/client""#);
        assert!(
            result.is_err(),
            "invalid characters must be rejected on deserialize"
        );
    }

    #[test]
    fn deserialize_rejects_empty_string() {
        let result: Result<ClientId, _> = serde_json::from_str(r#""""#);
        assert!(
            result.is_err(),
            "empty client_id must be rejected on deserialize"
        );
    }
}
