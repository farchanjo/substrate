//! `substrate-fs-index` — optional in-process filesystem index adapter per ADR-0041.
//!
//! This crate implements the `FsIndexPort` defined in `substrate-domain`.
//! It is OFF by default; consumers must opt in via Cargo features:
//!
//! - `fs-index` — enables the in-process snapshot index and all supporting machinery.
//! - `fs-index-watch` — adds an external-change watcher layer (inotify on Linux,
//!   `FSEvents` on macOS) via the `notify` crate.
//! - `linux-iouring` — crate-internal flag; the actual `io_uring` walk backend lives
//!   in `substrate-fs-query`. Present here only for feature-dependency wiring.
//! - `macos-getattrlistbulk` — enables the macOS `getattrlistbulk(2)` batch-stat
//!   implementation in the index rebuild path.
//!
//! When `fs-index` is not compiled in, `FsIndexFactory::build` returns a
//! `NullFsIndex` (Null Object) that always returns an empty result, causing callers
//! to fall back to the `ignore`-crate walk path from ADR-0003.
//!
//! # Freshness layers
//!
//! ADR-0041 defines four complementary freshness layers:
//! - Layer 0: mandatory lazy lstat per candidate hit.
//! - Layer 1: write-through updates from mutation crates at atomic-rename commit.
//! - Layer 2: kernel watcher (`fs-index-watch` feature) for external changes.
//! - Layer 3: TTL-based periodic rebuild (default 60 s).
//!
//! # Safety
//!
//! `forbid(unsafe_code)` is enforced workspace-wide. The sole exception in this
//! crate is `src/macos/getattrlistbulk.rs`, which calls the macOS
//! `getattrlistbulk(2)` syscall directly and therefore requires a narrow local
//! `#![allow(unsafe_code)]` per ADR-0042 and the forthcoming ADR-0044 SIMD /
//! low-level syscall exception policy.

#![cfg_attr(not(test), forbid(unsafe_code))]
#![warn(missing_docs)]

mod factory;
mod null;
mod polling;
mod rebuild;
mod snapshot;
mod tmp_filter;
mod write_through;

#[cfg(all(feature = "fs-index", target_os = "linux"))]
mod linux;

#[cfg(all(feature = "fs-index", target_os = "macos"))]
mod macos;

#[cfg(feature = "fs-index-watch")]
mod watcher;

// ---- Public surface ---------------------------------------------------------

pub use factory::FsIndexFactory;
pub use snapshot::{IndexEntry, IndexSnapshot, SnapshotSlot};
pub use write_through::WriteThroughHandle;
