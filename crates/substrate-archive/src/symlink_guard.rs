//! Symlink member guard for archive extraction.
//!
//! Archives may contain entries typed as symlinks.  Extracting them without
//! validation allows an attacker to plant a symlink that escapes the extraction
//! root, enabling subsequent archive entries to overwrite arbitrary paths.
//!
//! This module implements a two-tier guard per ADR-0004 and the Gherkin
//! scenarios in
//! `docs/arch/specs/features/archive/archive-symlink-member-blocked.feature`:
//!
//! 1. Symlinks whose **resolved target** stays within `extraction_root` are
//!    allowed and must be restored on disk.
//! 2. Symlinks whose target escapes `extraction_root` (absolute paths, `..`
//!    components, or targets that canonicalise outside the root) are rejected
//!    with `SUBSTRATE_PATH_TRAVERSAL_BLOCKED`.
//!
//! `reject_symlink_entry` is kept for the dry-run path and for entry types that
//! are not symlinks.

use std::path::{Component, Path};

use substrate_domain::{SubstrateError, SubstrateResult};
use uuid::Uuid;

/// Entry type passed to the guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// Regular file entry.
    File,
    /// Directory entry.
    Directory,
    /// Symbolic link entry — validated by `validate_symlink_target`.
    Symlink,
    /// Any other entry type (hard link, device, etc.).
    Other,
}

/// Rejects symlink entries unconditionally.
///
/// Used during the **dry-run** validation pass where creating the symlink on
/// disk would be incorrect, and during extraction of entry types that are never
/// expected to be symlinks.
///
/// # Errors
///
/// - `SUBSTRATE_SYMLINK_ESCAPE` — `kind == EntryKind::Symlink`.
pub fn reject_symlink_entry(kind: EntryKind, member_path: &str) -> SubstrateResult<()> {
    if kind == EntryKind::Symlink {
        return Err(SubstrateError::SymlinkEscape {
            path: member_path.to_owned(),
            correlation_id: Some(Uuid::now_v7()),
        });
    }
    Ok(())
}

/// Validates that the symlink `target` (the path the link points to) stays
/// within `extraction_root` when resolved relative to the directory that
/// contains the link itself (`link_dir`).
///
/// Rules (per ADR-0004 §symlink-validation):
///
/// - Absolute targets are always rejected (they point outside the root by
///   definition — we cannot know whether `/etc/passwd` is inside the root).
/// - Targets containing `..` components are rejected.
/// - Any target that, when joined with `link_dir`, traverses above
///   `extraction_root` is rejected.
///
/// # Errors
///
/// - `SUBSTRATE_PATH_TRAVERSAL_BLOCKED` — target escapes the extraction root.
pub fn validate_symlink_target(
    extraction_root: &Path,
    link_path: &Path,
    target: &Path,
) -> SubstrateResult<()> {
    // Absolute targets always escape — we cannot resolve them safely.
    if target.is_absolute() {
        return Err(SubstrateError::PathTraversalBlocked {
            path: target.to_string_lossy().into_owned(),
            correlation_id: Some(Uuid::now_v7()),
        });
    }

    // Any `..` component in the target is rejected to prevent traversal.
    for component in target.components() {
        if component == Component::ParentDir {
            return Err(SubstrateError::PathTraversalBlocked {
                path: target.to_string_lossy().into_owned(),
                correlation_id: Some(uuid::Uuid::now_v7()),
            });
        }
    }

    // Resolve `target` relative to the directory containing the link.
    let link_dir = link_path.parent().unwrap_or(extraction_root);
    let resolved = link_dir.join(target);
    // Normalise without hitting the filesystem (the target may not exist yet).
    let canonical = normalise_path(&resolved);
    // The canonical path must stay within extraction_root.
    if !canonical.starts_with(extraction_root) {
        return Err(SubstrateError::PathTraversalBlocked {
            path: target.to_string_lossy().into_owned(),
            correlation_id: Some(Uuid::now_v7()),
        });
    }
    Ok(())
}

/// Lexically normalises a path (resolves `.` and `..` without I/O).
///
/// This is used instead of `std::fs::canonicalize` so we can validate paths
/// that do not yet exist on disk (the symlink target is not yet created).
fn normalise_path(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

// ---- Tests -------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use super::*;

    #[test]
    fn file_entry_is_accepted() {
        assert!(reject_symlink_entry(EntryKind::File, "file.txt").is_ok());
    }

    #[test]
    fn directory_entry_is_accepted() {
        assert!(reject_symlink_entry(EntryKind::Directory, "subdir/").is_ok());
    }

    #[test]
    fn symlink_entry_is_rejected_by_reject_fn() {
        let err = reject_symlink_entry(EntryKind::Symlink, "link.txt").unwrap_err();
        assert!(matches!(err, SubstrateError::SymlinkEscape { .. }));
    }

    #[test]
    fn other_entry_kind_is_accepted() {
        assert!(reject_symlink_entry(EntryKind::Other, "device").is_ok());
    }

    #[test]
    fn validate_symlink_absolute_target_blocked() {
        let root = Path::new("/tmp/extraction");
        let link = Path::new("/tmp/extraction/a/link");
        let target = Path::new("/etc/passwd");
        let err = validate_symlink_target(root, link, target).unwrap_err();
        assert!(matches!(err, SubstrateError::PathTraversalBlocked { .. }));
    }

    #[test]
    fn validate_symlink_dotdot_target_blocked() {
        let root = Path::new("/tmp/extraction");
        let link = Path::new("/tmp/extraction/a/link");
        let target = Path::new("../../outside");
        let err = validate_symlink_target(root, link, target).unwrap_err();
        assert!(matches!(err, SubstrateError::PathTraversalBlocked { .. }));
    }

    #[test]
    fn validate_symlink_safe_relative_target_allowed() {
        let root = Path::new("/tmp/extraction");
        let link = Path::new("/tmp/extraction/a/link");
        let target = Path::new("a/target.txt"); // stays inside root
        assert!(validate_symlink_target(root, link, target).is_ok());
    }

    #[test]
    fn normalise_path_collapses_dot() {
        let p = std::path::PathBuf::from("/foo/./bar");
        assert_eq!(normalise_path(&p), std::path::PathBuf::from("/foo/bar"));
    }

    #[test]
    fn normalise_path_collapses_dotdot() {
        let p = std::path::PathBuf::from("/foo/bar/../baz");
        assert_eq!(normalise_path(&p), std::path::PathBuf::from("/foo/baz"));
    }
}

