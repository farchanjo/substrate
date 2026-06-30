//! Line-level secret redaction applied at the source before the event log (ADR-0066).
//!
//! Merges the global denylist from the operator config with the per-Service
//! `redact` patterns, then applies them to every output line before it reaches
//! [`substrate_domain::launch::event::LaunchEvent`].
//!
//! # Phase status
//!
//! **Phase 3 stub.** The following public items will be added in Phase 3:
//!
//! - `struct Redactor` — compiled pattern set (literal or regex, TBD per ADR-0066).
//! - `fn new(global: &[String], per_service: &[String]) -> Redactor`
//! - `fn redact_line(&self, line: &str) -> String`
//!
//! References: ADR-0066 §"redaction at source", ADR-0063 §"event log".
