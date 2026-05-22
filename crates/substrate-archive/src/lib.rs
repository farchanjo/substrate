//! `substrate-archive` — archive BC adapter per ADR-0022.
//!
//! Exposes seven MCP tools for the `archive` bounded context:
//! `archive.tar.create`, `archive.tar.extract`, `archive.zip.create`,
//! `archive.zip.extract`, `archive.gzip.compress`, `archive.gzip.decompress`,
//! and `archive.hash`.
//!
//! # Async zone classification (ADR-0003 / ADR-0040)
//!
//! | Tool                      | Bucket | Zone | Mechanism                          |
//! |---------------------------|--------|------|------------------------------------|
//! | `archive.tar.create`      | C      | B+C  | always-async job; `spawn_blocking` |
//! | `archive.tar.extract`     | C      | B+C  | always-async job; `spawn_blocking` |
//! | `archive.zip.create`      | C      | B+C  | always-async job; `spawn_blocking` |
//! | `archive.zip.extract`     | C      | B+C  | always-async job; `spawn_blocking` |
//! | `archive.gzip.compress`   | B      | A/B  | inline or `spawn_blocking`         |
//! | `archive.gzip.decompress` | B      | A/B  | inline or `spawn_blocking`         |
//! | `archive.hash`            | B      | C    | `spawn_blocking` + `Semaphore`     |
//!
//! # Security layers (ADR-0004 / ADR-0035)
//!
//! All handlers apply path-jail validation. Extract handlers additionally apply
//! `zip_slip_guard` (Zip Slip / Tar Slip) and `symlink_guard` (symlink member
//! rejection) per every entry before any disk write.
//!
//! # Transactional writes (ADR-0033)
//!
//! All create/compress handlers write to a sibling temp path and commit via
//! atomic rename. On cancellation or error the temp file is removed by the RAII
//! guard's `Drop` impl.

#![cfg_attr(not(test), forbid(unsafe_code))]
#![warn(missing_docs)]

mod dest_jail;
pub mod gzip_compress;
pub mod gzip_decompress;
pub mod hash;
pub mod hints_helpers;
pub mod manifest;
pub mod resource_limit;
pub mod response;
pub mod symlink_guard;
pub mod tar_create;
pub mod tar_extract;
pub mod tmp_path;
pub mod zip_create;
pub mod zip_extract;
pub mod zip_slip_guard;

pub use gzip_compress::handle_archive_gzip_compress;
pub use gzip_decompress::handle_archive_gzip_decompress;
pub use hash::handle_archive_hash;
pub use response::{ArchiveDeps, ToolResponse};
pub use tar_create::handle_archive_tar_create;
pub use tar_extract::handle_archive_tar_extract;
pub use zip_create::handle_archive_zip_create;
pub use zip_extract::handle_archive_zip_extract;
