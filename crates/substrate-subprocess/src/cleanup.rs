//! Explicit tmp-file cleanup per ADR-0033 and ADR-0014.
//!
//! `panic = "abort"` per ADR-0014 means `Drop` impls are not guaranteed to run.
//! All tmp-file cleanup in `substrate-subprocess` is explicit: called from the
//! async cancel path in [`crate::cascade::terminate_cascade`], NOT from any
//! `Drop` implementation.
//!
//! References: ADR-0033 §"Transactional Write Pattern", ADR-0014 §"panic=abort".

use std::path::PathBuf;

/// Removes each file in `paths`, returning a list of failures.
///
/// On success the file is silently removed. On failure, the `(path, error)` pair
/// is collected and returned. The caller is responsible for logging failures.
///
/// This function MUST NOT panic; errors are returned so the cascade continues
/// even when individual file removals fail (e.g., file already removed).
///
/// References: ADR-0033, ADR-0014.
pub async fn cleanup_tmp_files(paths: &[PathBuf]) -> Vec<(PathBuf, std::io::Error)> {
    let mut failures = Vec::new();
    for path in paths {
        if let Err(e) = tokio::fs::remove_file(path).await {
            // ENOENT is not an error: the file was already cleaned up.
            if e.kind() != std::io::ErrorKind::NotFound {
                failures.push((path.clone(), e));
            }
        }
    }
    failures
}
