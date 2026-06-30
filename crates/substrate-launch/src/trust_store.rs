//! TOFU trust-store I/O for the launch BC (ADR-0064).
//!
//! Implements the read, write, and permission-verification operations over
//! `~/.config/substrate/launch-trust.toml` (mode `0600`, user-owned).
//!
//! # Phase status
//!
//! **Phase 3 stub.** All public functions will be added in Phase 3 (TOFU gate,
//! profile load/merge, DAG, redaction). See the build plan for signatures.
//!
//! References: ADR-0064 §"trust store format", ADR-0035 §"safe-open".
