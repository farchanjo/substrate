//! `substrate-fs-query` — read-side filesystem adapter per ADR-0022.
//!
//! Exposes five MCP tools that map to the `filesystem-query` bounded context:
//! `fs.find`, `fs.read`, `fs.read_dir`, `fs.stat`, and `fs.hash`.
//!
//! # Async zone classification (ADR-0003)
//!
//! | Tool          | Zone | Mechanism                                  |
//! |---------------|------|--------------------------------------------|
//! | `fs.find`     | B    | `spawn_blocking` + `ignore::WalkBuilder`   |
//! | `fs.read`     | A/B  | `tokio::fs::read`; `spawn_blocking` >1 MiB |
//! | `fs.read_dir` | A    | `tokio::fs::read_dir`                       |
//! | `fs.stat`     | B    | `spawn_blocking` + `nix::sys::stat::lstat` |
//! | `fs.hash`     | C    | `spawn_blocking` + `Semaphore(num_cpus)`   |

// `unsafe_code` is `deny` (not `forbid`) here so that platform-specific
// walker modules (linux.rs, macos.rs) can opt-in with a narrow
// `#![allow(unsafe_code, reason = "...")]` per ADR-0042 + ADR-0044.
// All other modules remain effectively forbidden because the workspace lint
// is `deny` and no other module carries an `allow`.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod find;
pub mod hash;
pub mod hash_factory;
pub mod hint_helpers;
pub mod read;
pub mod read_dir;
pub mod response;
pub mod stat;
pub mod stat_factory;
mod symlink_chain;
pub mod walker;
pub mod walker_factory;

pub use find::handle_fs_find;
pub use hash::handle_fs_hash;
pub use read::handle_fs_read;
pub use read_dir::handle_fs_read_dir;
pub use stat::handle_fs_stat;

pub use response::{FsQueryDeps, ToolResponse};
