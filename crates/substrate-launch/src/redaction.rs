//! Line-level secret redaction applied at the source before the event log (ADR-0066).
//!
//! Merges the global denylist from the operator config with the per-Service
//! `redact` list, then applies the merged set to every output line before it
//! reaches [`substrate_domain::launch::event::LaunchEvent`]. Redaction happens at
//! the source: a secret a child prints is masked before the byte is written to
//! the event log or delivered to any client.
//!
//! # MVP matching strategy
//!
//! For the MVP this is a literal-substring denylist: every configured needle is
//! a literal token (typically the secret value itself), and each occurrence is
//! replaced with [`REDACTION_PLACEHOLDER`]. Regex / assignment-shaped patterns
//! (for example `API_KEY=\S+`) are deferred until ADR-0066 fixes the pattern
//! grammar; at that point a `regex` dependency would be declared locally in this
//! adapter crate (the pure domain crate forbids it).
//!
//! References: ADR-0066 §"redaction at source", ADR-0063 §"event log".

/// The token substituted in place of every redacted match.
pub const REDACTION_PLACEHOLDER: &str = "[REDACTED]";

/// A compiled set of literal denylist needles applied to every output line.
///
/// Construct with [`Redactor::new`], which merges the global and per-Service
/// lists and drops empty entries. Apply with [`Redactor::redact_line`].
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    /// Literal needles to mask; empty entries are filtered out at construction.
    /// Sorted longest-first so a longer secret is masked before any substring of
    /// it is considered.
    needles: Vec<String>,
}

impl Redactor {
    /// Builds a `Redactor` from the merged global and per-Service denylists.
    ///
    /// The two slices are concatenated, empty entries are dropped, duplicates are
    /// removed, and the result is ordered longest-needle-first so that a longer
    /// secret is redacted before any shorter substring of it.
    #[must_use]
    pub fn new(global: &[String], per_service: &[String]) -> Self {
        let mut needles: Vec<String> = global
            .iter()
            .chain(per_service.iter())
            .filter(|n| !n.is_empty())
            .cloned()
            .collect();
        needles.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        needles.dedup();
        Self { needles }
    }

    /// Returns `true` when no needle is configured (redaction is a no-op).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.needles.is_empty()
    }

    /// Returns `line` with every configured needle replaced by
    /// [`REDACTION_PLACEHOLDER`].
    ///
    /// Applied before the line reaches the event log or any client, so a secret a
    /// child prints never escapes the supervisor in the clear.
    #[must_use]
    pub fn redact_line(&self, line: &str) -> String {
        let mut out = line.to_owned();
        for needle in &self.needles {
            if out.contains(needle.as_str()) {
                out = out.replace(needle.as_str(), REDACTION_PLACEHOLDER);
            }
        }
        out
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use super::*;

    #[test]
    fn per_service_needle_is_redacted() {
        // launch-event-redaction-at-source: a per-Service redact pattern matching
        // a secret token masks it before it reaches any consumer.
        let r = Redactor::new(&[], &["s3cr3t-token".to_owned()]);
        let line = r.redact_line("connecting with s3cr3t-token now");
        assert_eq!(line, "connecting with [REDACTED] now");
        assert!(!line.contains("s3cr3t-token"));
    }

    #[test]
    fn global_denylist_redacts_when_per_service_empty() {
        // launch-global-redact-denylist-applied: the global denylist redacts even
        // when the per-Service list is empty.
        let r = Redactor::new(&["AKIAEXAMPLESECRET".to_owned()], &[]);
        let line = r.redact_line("API_KEY=AKIAEXAMPLESECRET");
        assert_eq!(line, "API_KEY=[REDACTED]");
        assert!(!line.contains("AKIAEXAMPLESECRET"));
    }

    #[test]
    fn merged_global_and_per_service_both_apply() {
        let r = Redactor::new(&["GLOBALSEC".to_owned()], &["LOCALSEC".to_owned()]);
        let line = r.redact_line("a=GLOBALSEC b=LOCALSEC");
        assert_eq!(line, "a=[REDACTED] b=[REDACTED]");
    }

    #[test]
    fn longest_needle_wins_over_substring() {
        // A longer secret that contains a shorter one is masked wholesale.
        let r = Redactor::new(&[], &["abc".to_owned(), "abcdef".to_owned()]);
        let line = r.redact_line("value=abcdef");
        assert_eq!(line, "value=[REDACTED]");
    }

    #[test]
    fn empty_needles_are_dropped() {
        let r = Redactor::new(&[String::new()], &[String::new()]);
        assert!(r.is_empty());
        assert_eq!(r.redact_line("nothing to redact"), "nothing to redact");
    }

    #[test]
    fn line_without_match_is_unchanged() {
        let r = Redactor::new(&["secret".to_owned()], &[]);
        assert_eq!(r.redact_line("plain log line"), "plain log line");
    }
}
