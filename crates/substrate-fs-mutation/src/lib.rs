//! `substrate-fs-mutation` — write-side filesystem adapter per ADR-0022.
//!
//! Exposes eight MCP tools in the `filesystem-mutation` bounded context:
//! `fs.mkdir`, `fs.write`, `fs.copy`, `fs.rename`, `fs.remove`,
//! `fs.set_permissions`, `fs.symlink`, and `fs.touch`.
//!
//! # Async zone classification (ADR-0003)
//!
//! | Tool                | Zone | Mechanism                                            |
//! |---------------------|------|------------------------------------------------------|
//! | `fs.mkdir`          | A    | `tokio::fs::create_dir_all`                          |
//! | `fs.write`          | A    | `tokio::fs::write` + `TmpPath` atomic rename         |
//! | `fs.copy`           | B    | `tokio::fs::copy` + `TmpPath` via `spawn_blocking`   |
//! | `fs.rename`         | A    | `tokio::fs::rename`                                  |
//! | `fs.remove`         | B    | `spawn_blocking` + elicitation gate                  |
//! | `fs.set_permissions`| B    | `spawn_blocking` + `nix::sys::stat::chmod`           |
//! | `fs.symlink`        | A    | `tokio::fs::symlink`                                 |
//! | `fs.touch`          | A/B  | open/close + `nix::sys::stat::utimensat`             |
//!
//! # Security layers (ADR-0004)
//!
//! All handlers enforce the four security layers in order:
//! 1. Allowlist — via [`PathJailPort`](substrate_domain::PathJailPort).
//! 2. Path jail — kernel-enforced or userspace-degraded per ADR-0035.
//! 3. Dry-run gate — `fs.remove`, `fs.rename`, `fs.set_permissions` require dry-run first.
//! 4. Elicitation — `fs.remove`, `fs.set_permissions` (world-writable) require confirmation.
//!
//! # Transactional writes (ADR-0033)
//!
//! `fs.write` and `fs.copy` use [`TmpPath`] to guarantee atomicity:
//! write to `<target>.tmp.<crockford_uuid7>`, then rename to target.
//! The [`TmpPath`] Drop impl cleans up the temp file on cancellation or panic.

#![cfg_attr(not(test), forbid(unsafe_code))]
#![warn(missing_docs)]

pub mod copy;
pub mod elicitation;
pub mod hints_helpers;
pub mod mkdir;
pub mod preflight;
pub mod remove;
pub mod rename;
pub mod response;
pub mod set_permissions;
pub mod symlink;
pub mod tmp_path;
pub mod touch;
pub mod write;
pub mod write_through;

pub use copy::handle_fs_copy;
pub use mkdir::handle_fs_mkdir;
pub use remove::handle_fs_remove;
pub use rename::handle_fs_rename;
pub use response::{FsMutationDeps, ToolResponse};
pub use set_permissions::handle_fs_set_permissions;
pub use symlink::handle_fs_symlink;
pub use touch::handle_fs_touch;
pub use write::handle_fs_write;
