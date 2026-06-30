//! Safe-open profile loading with the five-step TOFU gate (ADR-0064).
//!
//! Implements `load_trusted`, `load_untrusted`, and `write_scaffold` following
//! the open-nofollow → fstat → BLAKE3 hash → trust lookup → deserialize
//! pipeline that closes the TOCTOU window.
//!
//! # Phase status
//!
//! **Phase 3 stub.** All public functions will be added in Phase 3. See the
//! build plan for the full TOFU gate implementation.
//!
//! References: ADR-0064 §"profile loading", ADR-0035 §"safe-open".
