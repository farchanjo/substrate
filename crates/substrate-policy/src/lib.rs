//! `substrate-policy` — allowlist enforcement and tiered `PathJail` implementation.
//!
//! # Tier selection (ADR-0035 amendment, ADR-0042)
//!
//! The factory probes the `Capabilities` snapshot and selects the highest
//! available tier at composition-root startup:
//!
//! | Platform       | Tier 1 (kernel-enforced)                        | Tier degraded (userspace) |
//! |----------------|-------------------------------------------------|--------------------------|
//! | Linux ≥ 5.6    | `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)` | `strict-path` + post-check |
//! | macOS ≥ 12     | `openat O_NOFOLLOW_ANY`                         | `strict-path` + post-check |
//! | other / older  | —                                               | `strict-path` + post-check |
//!
//! The degraded tier has a non-zero TOCTOU window. When
//! `security.refuse_degraded_jail = true` (default), the composition root
//! aborts startup with `SUBSTRATE_JAIL_DEGRADED_REFUSED` before accepting
//! any MCP requests.
//!
//! # `JailedPath` construction contract
//!
//! Only this crate constructs [`JailedPath`] values. Domain code and all
//! adapter crates receive `JailedPath` through the
//! [`PathJailPort`](substrate_domain::PathJailPort) abstraction. The function
//! [`JailedPath::new_jailed`](substrate_domain::JailedPath::new_jailed) is
//! documented as `substrate-policy`-only; misuse is caught by the Rego policy
//! `policies/path_jail_construction.rego` in CI.

// unsafe_code is forbidden throughout this crate EXCEPT in the platform-specific
// path-jail modules (linux/mod.rs, macos/mod.rs) which require direct syscall
// wiring (openat2, O_NOFOLLOW_ANY). Those modules override with
// #[allow(unsafe_code, reason = "...")] citing ADR-0042 + ADR-0035.
// We use `deny` (not `forbid`) so that the per-module allow is valid.
#![deny(unsafe_code)]
#![warn(missing_docs)]

mod allowlist;
mod jail_factory;
mod userspace_jail;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

pub use allowlist::Allowlist;
pub use jail_factory::PathJailFactory;
