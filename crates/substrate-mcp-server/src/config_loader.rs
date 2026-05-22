//! Thin re-export layer — canonical config types and loader live in `substrate-config`.
//!
//! `substrate-mcp-server` imports from here so that internal call-sites need
//! not depend on `substrate_config` directly.

#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

pub(crate) use substrate_config::load;
