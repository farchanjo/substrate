//! Zip Slip / Tar Slip path validation per ADR-0004 and ADR-0035.
//!
//! Every archive member path MUST pass through [`validate_member_path`] before
//! any file-creation syscall during extraction. The guard rejects:
//!
//! - Absolute paths (e.g. `/etc/passwd`).
//! - Paths containing `..` components.
//! - Paths whose canonical parent resolves outside `dest_root` after joining.
//!
//! This matches the Gherkin scenario in
//! `docs/arch/specs/features/archive/archive-zip-extract-zip-slip-blocked.feature`.

use std::path::{Component, Path, PathBuf};

use substrate_domain::{SubstrateError, SubstrateResult};

/// Validates that `member` joined to `dest_root` stays within `dest_root`.
///
/// Returns the joined `PathBuf` (not yet written) on success.
///
/// # Errors
///
/// - `SUBSTRATE_PATH_TRAVERSAL_BLOCKED` — `member` is absolute, contains `..`,
///   or its canonical parent resolves outside `dest_root`.
pub fn validate_member_path(dest_root: &Path, member: &Path) -> SubstrateResult<PathBuf> {
    // Reject absolute member paths immediately.
    if member.is_absolute() {
        return Err(SubstrateError::PathTraversalBlocked {
            path: member.to_string_lossy().into_owned(),
            correlation_id: None,
        });
    }

    // Reject any `..` component — normalisation tricks are blocked eagerly.
    for component in member.components() {
        if component == Component::ParentDir {
            return Err(SubstrateError::PathTraversalBlocked {
                path: member.to_string_lossy().into_owned(),
                correlation_id: None,
            });
        }
    }

    // Join member to dest_root and verify the result stays inside dest_root.
    // We use lexical normalization (no canonicalize syscall, which would require
    // the file to exist) to detect traversals that slip through multi-component
    // paths such as `a/b/../../../outside`.
    let joined = dest_root.join(member);
    let normalised = normalize_lexical(&joined);
    let root_normalised = normalize_lexical(dest_root);

    if !normalised.starts_with(&root_normalised) {
        return Err(SubstrateError::PathTraversalBlocked {
            path: member.to_string_lossy().into_owned(),
            correlation_id: None,
        });
    }

    Ok(joined)
}

/// Lexically normalises a path by resolving `.` and `..` components without
/// issuing any syscall (no `canonicalize`).
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            },
            Component::CurDir => {},
            other => out.push(other.as_os_str()),
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

    fn root() -> &'static Path {
        Path::new("/work/repo/extracted")
    }

    #[test]
    fn safe_member_is_accepted() {
        let result = validate_member_path(root(), Path::new("subdir/file.txt"));
        assert!(result.is_ok());
    }

    #[test]
    fn dotdot_in_member_is_rejected() {
        let err = validate_member_path(root(), Path::new("../evil.txt")).unwrap_err();
        assert!(matches!(err, SubstrateError::PathTraversalBlocked { .. }));
    }

    #[test]
    fn absolute_member_is_rejected() {
        let err = validate_member_path(root(), Path::new("/etc/passwd")).unwrap_err();
        assert!(matches!(err, SubstrateError::PathTraversalBlocked { .. }));
    }

    #[test]
    fn nested_dotdot_escape_is_rejected() {
        // "a/../../outside.txt" resolves outside dest_root.
        let err = validate_member_path(root(), Path::new("a/../../outside.txt")).unwrap_err();
        assert!(matches!(err, SubstrateError::PathTraversalBlocked { .. }));
    }

    #[test]
    fn deep_but_valid_member_is_accepted() {
        let result = validate_member_path(root(), Path::new("a/b/c/file.txt"));
        assert!(result.is_ok());
    }
}
