//! `StreamChunk` — value object carried in each `notifications/progress` event
//! for subprocess stdout and stderr output per ADR-0054.
//!
//! Each chunk carries raw bytes (up to 4 KiB decoded), a monotonic sequence
//! number, a byte offset for reassembly, and a timestamp. Base64 encoding for
//! the wire (`chunk_base64` in the JSON payload) is performed at the adapter
//! layer in `substrate-subprocess`; the domain stores raw bytes.
//!
//! References: ADR-0054 §"Stream Chunk Payload", ADR-0052 §"`StreamEvent`".

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::value_objects::JobId;

/// Identifies whether a chunk originates from standard output or standard error.
///
/// Serialized as `"stdout"` / `"stderr"` to match the CUE `#StreamChunk.stream` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    /// Standard output of the child process.
    Stdout,
    /// Standard error of the child process.
    Stderr,
}

impl std::fmt::Display for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdout => f.write_str("stdout"),
            Self::Stderr => f.write_str("stderr"),
        }
    }
}

/// A chunk of raw bytes read from stdout or stderr of a spawned child process.
///
/// Chunks are emitted by the reader task in `substrate-subprocess` and delivered
/// to the MCP client via `notifications/progress` per ADR-0054. The adapter is
/// responsible for base64-encoding `chunk` before putting it on the wire; the
/// domain always holds raw bytes.
///
/// `seq` is a zero-based monotonic counter per job per stream, reset to zero
/// at spawn. Clients use `seq` to detect dropped chunks (gaps) and to reorder
/// chunks if out-of-order delivery occurs.
///
/// See ADR-0054 §"Stream Chunk Payload".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    /// Correlates the chunk with its originating `SubprocessHandle`.
    pub job_id: JobId,

    /// Identifies whether the chunk originates from stdout or stderr.
    pub stream: Stream,

    /// Zero-based monotonic sequence number for this stream within this job.
    ///
    /// Gaps in `seq` indicate dropped chunks due to mpsc backpressure per ADR-0054.
    pub seq: u64,

    /// Raw bytes read from the OS pipe into the substrate capture buffer.
    ///
    /// Maximum 4 KiB per chunk (4096 bytes decoded). The adapter layer
    /// base64-encodes this to `chunk_base64` on the wire (RFC 4648 §4).
    pub chunk: Vec<u8>,

    /// Cumulative byte offset of the first byte in this chunk relative to the
    /// beginning of the stream, allowing ordered reassembly.
    pub byte_offset: u64,

    /// Wall-clock timestamp at which this chunk was read from the OS pipe.
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}
