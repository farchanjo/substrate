//! Orphan tmp-file reaper per ADR-0055.
//!
//! Substrate may crash hard (SIGKILL, panic with `panic = "abort"`, OS OOM)
//! leaving transit files `.substrate-subprocess-stream-<job_id>.<stream>.tmp.<uuid7>`
//! behind in `tmp_root`. On next startup, this module scans `tmp_root` for those
//! files and removes any whose mtime is older than `max_age`.
//!
//! Naming convention is owned by `crate::tmp_file`:
//!   transit: `<tmp_root>/.substrate-subprocess-stream-<job_id>.<stream>.tmp.<uuid7>`
//!   final:   `<tmp_root>/.substrate-subprocess-stream-<job_id>.<stream>`
//!
//! The reaper ONLY removes transit files (those with `.tmp.<uuid7>` segment).
//! Final files are preserved — they represent persisted captures whose owning
//! handle exited cleanly. A separate operator-level retention policy may apply
//! to final files.
//!
//! References: ADR-0055, ADR-0033, ADR-0052.

use std::path::Path;
use std::time::{Duration, SystemTime};

use tracing::{info, warn};

/// Result of one reaper invocation.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReaperStats {
    /// Number of orphan transit files successfully removed.
    pub reaped: usize,
    /// Number of transit files inspected but skipped (mtime within `max_age`).
    pub skipped_young: usize,
    /// Number of non-transit entries skipped (final files, unrelated content).
    pub skipped_unrelated: usize,
    /// Number of failures (permission denied, IO errors). Caller logs details.
    pub errors: usize,
}

/// Removes orphan transit files in `tmp_root` older than `max_age`.
///
/// Returns `Ok(ReaperStats)` always; individual entry failures are counted in
/// `stats.errors` and logged via `tracing::warn`. A failure to read `tmp_root`
/// itself is the only `Err` path.
///
/// References: ADR-0055.
///
/// # Errors
///
/// Returns `io::Error` only when `tmp_root` cannot be read (missing dir or
/// permission denied at directory level). Per-file errors are accumulated in
/// the returned stats.
pub async fn run_once(tmp_root: &Path, max_age: Duration) -> std::io::Result<ReaperStats> {
    let mut stats = ReaperStats::default();

    let mut read_dir = match tokio::fs::read_dir(tmp_root).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // tmp_root not yet created — nothing to reap.
            info!(
                target: "substrate_audit",
                event = "SUBSTRATE_ORPHAN_REAPER_NOOP",
                reason = "tmp_root_missing",
                "orphan reaper: tmp_root not yet created — nothing to reap"
            );
            return Ok(stats);
        }
        Err(e) => return Err(e),
    };

    let now = SystemTime::now();

    while let Some(entry) = read_dir.next_entry().await? {
        let file_name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => {
                stats.skipped_unrelated += 1;
                continue;
            }
        };

        if !is_transit_filename(&file_name) {
            stats.skipped_unrelated += 1;
            continue;
        }

        let metadata = match entry.metadata().await {
            Ok(m) => m,
            Err(e) => {
                warn!(
                    target: "substrate_audit",
                    event = "SUBSTRATE_ORPHAN_REAPER_STAT_FAILED",
                    path = %entry.path().display(),
                    error = %e,
                    "orphan reaper: stat failed (non-fatal)"
                );
                stats.errors += 1;
                continue;
            }
        };

        let mtime = match metadata.modified() {
            Ok(t) => t,
            Err(e) => {
                warn!(
                    target: "substrate_audit",
                    event = "SUBSTRATE_ORPHAN_REAPER_MTIME_FAILED",
                    path = %entry.path().display(),
                    error = %e,
                    "orphan reaper: mtime read failed (non-fatal)"
                );
                stats.errors += 1;
                continue;
            }
        };

        let age = now.duration_since(mtime).unwrap_or_default();
        if age < max_age {
            stats.skipped_young += 1;
            continue;
        }

        let path = entry.path();
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                info!(
                    target: "substrate_audit",
                    event = "SUBSTRATE_ORPHAN_TMP_REAPED",
                    path = %path.display(),
                    age_secs = age.as_secs(),
                    "orphan reaper: removed stale transit file"
                );
                stats.reaped += 1;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Race with another reaper or operator; treat as reaped.
                stats.reaped += 1;
            }
            Err(e) => {
                warn!(
                    target: "substrate_audit",
                    event = "SUBSTRATE_ORPHAN_REAPER_UNLINK_FAILED",
                    path = %path.display(),
                    error = %e,
                    "orphan reaper: unlink failed (non-fatal)"
                );
                stats.errors += 1;
            }
        }
    }

    info!(
        target: "substrate_audit",
        event = "SUBSTRATE_ORPHAN_REAPER_DONE",
        reaped = stats.reaped,
        skipped_young = stats.skipped_young,
        skipped_unrelated = stats.skipped_unrelated,
        errors = stats.errors,
        "orphan reaper: pass complete"
    );

    Ok(stats)
}

/// Returns true when `name` matches the transit filename pattern
/// `.substrate-subprocess-stream-<job_id>.<stream>.tmp.<uuid7>`.
///
/// Final files of the form `.substrate-subprocess-stream-<job_id>.<stream>`
/// (without `.tmp.<suffix>`) do NOT match — they are persisted captures.
fn is_transit_filename(name: &str) -> bool {
    // Required prefix per tmp_file.rs:128.
    const PREFIX: &str = ".substrate-subprocess-stream-";
    const TRANSIT_MARKER: &str = ".tmp.";

    if !name.starts_with(PREFIX) {
        return false;
    }
    // The `.tmp.<uuid7>` segment is what distinguishes transit from final.
    let Some(marker_idx) = name.find(TRANSIT_MARKER) else {
        return false;
    };
    // Must have at least one character after `.tmp.` (the suffix).
    name.len() > marker_idx + TRANSIT_MARKER.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn transit_pattern_matches_uuid_suffix() {
        assert!(is_transit_filename(
            ".substrate-subprocess-stream-abc123.stdout.tmp.0192f000-7c0e-7000-8000-000000000001"
        ));
        assert!(is_transit_filename(
            ".substrate-subprocess-stream-job.stderr.tmp.x"
        ));
    }

    #[test]
    fn final_pattern_rejected() {
        assert!(!is_transit_filename(
            ".substrate-subprocess-stream-abc123.stdout"
        ));
        assert!(!is_transit_filename(
            ".substrate-subprocess-stream-job.stderr"
        ));
    }

    #[test]
    fn unrelated_files_rejected() {
        assert!(!is_transit_filename("foo.log"));
        assert!(!is_transit_filename(".DS_Store"));
        assert!(!is_transit_filename("substrate-subprocess-stream-x.stdout.tmp.1"));
    }

    #[tokio::test]
    async fn run_once_reaps_stale_transit_file() {
        let tmp = std::env::temp_dir().join(format!(
            "substrate-reaper-test-{}",
            uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))
        ));
        fs::create_dir_all(&tmp).expect("mkdir test root");

        let stale = tmp.join(".substrate-subprocess-stream-job.stdout.tmp.deadbeef");
        fs::write(&stale, b"stale").expect("write stale");
        // Set mtime 1h in the past.
        let one_hour_ago = SystemTime::now() - Duration::from_secs(3600);
        filetime::set_file_mtime(&stale, one_hour_ago.into()).expect("set mtime");

        let stats = run_once(&tmp, Duration::from_secs(600))
            .await
            .expect("reaper run");

        assert_eq!(stats.reaped, 1, "expected 1 reaped, got {stats:?}");
        assert!(!stale.exists(), "stale file must be removed");

        fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn run_once_skips_young_transit_file() {
        let tmp = std::env::temp_dir().join(format!(
            "substrate-reaper-young-{}",
            uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))
        ));
        fs::create_dir_all(&tmp).expect("mkdir");
        let young = tmp.join(".substrate-subprocess-stream-job.stdout.tmp.feedface");
        fs::write(&young, b"recent").expect("write");

        let stats = run_once(&tmp, Duration::from_secs(600))
            .await
            .expect("reaper run");

        assert_eq!(stats.reaped, 0);
        assert_eq!(stats.skipped_young, 1);
        assert!(young.exists(), "young file must NOT be removed");

        fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn run_once_skips_final_file() {
        let tmp = std::env::temp_dir().join(format!(
            "substrate-reaper-final-{}",
            uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))
        ));
        fs::create_dir_all(&tmp).expect("mkdir");
        let final_file = tmp.join(".substrate-subprocess-stream-job.stdout");
        fs::write(&final_file, b"persisted").expect("write");
        let one_hour_ago = SystemTime::now() - Duration::from_secs(3600);
        filetime::set_file_mtime(&final_file, one_hour_ago.into()).expect("set mtime");

        let stats = run_once(&tmp, Duration::from_secs(600))
            .await
            .expect("reaper run");

        assert_eq!(stats.reaped, 0);
        assert_eq!(stats.skipped_unrelated, 1);
        assert!(final_file.exists(), "final file must be preserved");

        fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn run_once_missing_tmp_root_is_noop() {
        let tmp = std::env::temp_dir().join(format!(
            "substrate-reaper-missing-{}",
            uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))
        ));
        // Do NOT create the directory.

        let stats = run_once(&tmp, Duration::from_secs(600))
            .await
            .expect("reaper run");

        assert_eq!(stats.reaped, 0);
        assert_eq!(stats.errors, 0);
    }
}
