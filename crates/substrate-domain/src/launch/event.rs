//! `LaunchEvent` — one entry in the durable per-Stack event-log.
//!
//! Mirrors `#LaunchEvent` and `#LaunchEventKind` in
//! `docs/arch/schemas/launch.cue` (ADR-0066). Lifecycle events are
//! authoritative; the `Semantic` marker carries advisory, redacted output
//! distilled from a child channel. The `message` is already redacted by the
//! source before it ever reaches this value object.
//!
//! References: ADR-0066 §"Event stream and notification model".

use serde::{Deserialize, Serialize};

use crate::launch::profile::ServiceName;
use crate::subprocess::stream::Stream;
use crate::value_objects::stack_id::StackId;

/// Classification of a launch event.
///
/// Mirrors `#LaunchEventKind`, serialized as `SCREAMING_SNAKE_CASE` to match the
/// CUE string values. Lifecycle kinds are authoritative; `Semantic` is advisory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LaunchEventKind {
    /// A Service child process was spawned.
    Started,
    /// A Service passed its readiness probe.
    Ready,
    /// A Service exited with a zero exit code.
    Exited,
    /// A Service exited with a non-zero exit code.
    Crashed,
    /// A Service is being re-spawned by the supervisor.
    Restarting,
    /// A previously detached orphan process was reaped on boot.
    OrphanReaped,
    /// A previously detached orphan process was re-adopted on boot.
    OrphanAdopted,
    /// A detached Stack exceeded its `orphan_ttl_secs` and was brought down.
    StackTtlExpired,
    /// An advisory, redacted marker distilled from a child output channel.
    Semantic,
}

impl std::fmt::Display for LaunchEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Started => "STARTED",
            Self::Ready => "READY",
            Self::Exited => "EXITED",
            Self::Crashed => "CRASHED",
            Self::Restarting => "RESTARTING",
            Self::OrphanReaped => "ORPHAN_REAPED",
            Self::OrphanAdopted => "ORPHAN_ADOPTED",
            Self::StackTtlExpired => "STACK_TTL_EXPIRED",
            Self::Semantic => "SEMANTIC",
        };
        f.write_str(s)
    }
}

/// One entry in the durable per-Stack event-log (`events.ndjson`).
///
/// Mirrors `#LaunchEvent` in `launch.cue` (ADR-0066). The `cursor` is the opaque
/// `?since` value (ADR-0008) a client passes to read the delta.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchEvent {
    /// Correlates the event with its Stack.
    pub stack_id: StackId,
    /// Originating Service; absent for Stack-level events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<ServiceName>,
    /// Event classification.
    pub kind: LaunchEventKind,
    /// Zero-based monotonic sequence number within the Stack event-log.
    pub seq: u64,
    /// Opaque pagination cursor addressing this position in the log.
    pub cursor: String,
    /// Present only for `Semantic` events distilled from a child output channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<Stream>,
    /// Redacted, human-oriented event text (already passed the denylist).
    pub message: String,
    /// Present only for `Exited` / `Crashed` events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// RFC 3339 time the event was recorded.
    pub timestamp: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_serializes_screaming_snake_case() {
        #[expect(clippy::unwrap_used, reason = "test: enum serializes")]
        let s = serde_json::to_string(&LaunchEventKind::OrphanReaped).unwrap();
        assert_eq!(s, "\"ORPHAN_REAPED\"");
    }

    #[test]
    fn event_round_trips_and_skips_absent_optionals() {
        let event = LaunchEvent {
            stack_id: StackId::now_v7(),
            service: None,
            kind: LaunchEventKind::Started,
            seq: 0,
            cursor: "c0".to_owned(),
            stream: None,
            message: "boot ok".to_owned(),
            exit_code: None,
            timestamp: "2026-06-30T12:00:00Z".to_owned(),
        };
        #[expect(clippy::unwrap_used, reason = "test: in-memory value serializes")]
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("\"service\":"));
        assert!(!json.contains("\"stream\":"));
        assert!(!json.contains("\"exit_code\":"));
        #[expect(clippy::unwrap_used, reason = "test: round-trip deserializes")]
        let back: LaunchEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, LaunchEventKind::Started);
        assert_eq!(back.seq, 0);
    }
}
