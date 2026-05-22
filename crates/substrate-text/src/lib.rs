//! `substrate-text` — text-processing BC adapter.
//!
//! Exposes four MCP tools that map to the `text-processing` bounded context:
//! `text.search`, `text.count_lines`, `text.head`, and `text.tail`.
//!
//! # Async zone classification (ADR-0003)
//!
//! | Tool               | Zone | Mechanism                                           |
//! |--------------------|------|-----------------------------------------------------|
//! | `text.search`      | B    | `spawn_blocking`; Bucket-B auto-mode (ADR-0040)     |
//! | `text.count_lines` | B    | `spawn_blocking`; Bucket-B auto-mode (ADR-0040)     |
//! | `text.head`        | A    | async-native `BufReader::lines()`, capped at 1000   |
//! | `text.tail`        | A/B  | small file: Zone A; large file: Zone B `spawn_blocking` |
//!
//! # SIMD acceleration (ADR-0043)
//!
//! - `memchr` — SIMD byte scan for newline detection and binary sniffing.
//! - `aho-corasick` — Teddy SIMD for multi-pattern search.
//! - `regex` — Teddy prefilter via internal `aho-corasick` linkage.
//! - `bytecount` — SIMD popcount for newline counting.
//! - `simdutf8` — SIMD UTF-8 validation for binary detection and encoding guard.
//!
//! # No-subprocess invariant (ADR-0044)
//!
//! This crate MUST NOT call `std::process::Command`, `tokio::process::Command`,
//! or any subprocess API. All search is pure-Rust via the crates listed above.

// SIMD intrinsic wrappers live in external SIMD crates only. No direct
// intrinsic call sites exist in this crate; unsafe is therefore fully
// disallowed except in the test harness where proptest may require it.
#![cfg_attr(not(test), forbid(unsafe_code))]
#![warn(missing_docs)]

pub mod binary_detect;
pub mod count_lines;
pub mod head;
pub mod hints_helpers;
pub mod pagination;
pub mod regex_guard;
pub mod search;
pub mod tail;

// ---- Re-exports for the composition root ------------------------------------

pub use count_lines::handle_text_count_lines;
pub use head::handle_text_head;
pub use response::{TextDeps, ToolResponse};
pub use search::handle_text_search;
pub use tail::handle_text_tail;

mod response;
