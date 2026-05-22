//! `ReDoS` guard — compile-time size limits on regex DFA construction.
//!
//! The `regex` crate compiles patterns into a DFA. For pathological inputs
//! (e.g., `(a+)+b`), the NFA→DFA conversion can consume unbounded memory and
//! CPU time. Two limits enforced by `regex::RegexBuilder` prevent this:
//!
//! - `size_limit(10_000_000)` — caps NFA byte size (prevents gigantic NFAs).
//! - `dfa_size_limit(20_000_000)` — caps DFA byte size (prevents state explosion).
//!
//! When either limit is hit, `build()` returns an error that this module maps
//! to [`SubstrateError::InvalidArgument`] with code `SUBSTRATE_INVALID_ARGUMENT`
//! and a descriptive reason pointing the agent at the issue.
//!
//! Per the Gherkin spec `text-search-catastrophic-regex.feature`, the server
//! must remain responsive after a rejected pattern. Because the rejection
//! happens at compile time (inside `compile_regex`), no scanning is ever
//! attempted and server resources are not exhausted.

use regex::Regex;
use substrate_domain::{SubstrateError, SubstrateResult};

/// NFA byte-size limit applied to all compiled regex patterns.
const REGEX_NFA_SIZE_LIMIT: usize = 10_000_000;

/// DFA byte-size limit applied to all compiled regex patterns.
const REGEX_DFA_SIZE_LIMIT: usize = 20_000_000;

/// Compiles `pattern` into a [`Regex`] with ReDoS-prevention size limits.
///
/// # Errors
///
/// Returns [`SubstrateError::InvalidArgument`] with a reason string when:
///
/// - The pattern is syntactically invalid.
/// - The compiled NFA exceeds [`REGEX_NFA_SIZE_LIMIT`] bytes.
/// - The compiled DFA exceeds [`REGEX_DFA_SIZE_LIMIT`] bytes.
///
/// All three cases surface as `SUBSTRATE_INVALID_ARGUMENT` to the agent,
/// with the pattern name in `offending_field`.
pub fn compile_regex(pattern: &str) -> SubstrateResult<Regex> {
    regex::RegexBuilder::new(pattern)
        .size_limit(REGEX_NFA_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .build()
        .map_err(|e| SubstrateError::InvalidArgument {
            offending_field: "pattern".to_owned(),
            reason: format!(
                "regex compilation failed (possible catastrophic backtracking or size limit exceeded): {e}"
            ),
            correlation_id: None,
        })
}

// ---- Tests -------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use super::*;

    #[test]
    fn simple_regex_compiles() {
        let re = compile_regex("hello world");
        assert!(re.is_ok(), "simple literal pattern must compile");
    }

    #[test]
    fn anchored_regex_compiles() {
        let re = compile_regex("^[a-z]+$");
        assert!(re.is_ok(), "anchored character class must compile");
    }

    #[test]
    fn invalid_syntax_returns_error() {
        let re = compile_regex("[unclosed");
        assert!(re.is_err());
        let err = re.unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_INVALID_ARGUMENT");
    }

    #[test]
    fn valid_regex_matches_expected_input() {
        let re = compile_regex(r"\d+").expect("digit pattern must compile");
        assert!(re.is_match("abc123def"));
        assert!(!re.is_match("abcdef"));
    }
}
