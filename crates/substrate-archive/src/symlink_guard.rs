//! Symlink member guard for archive extraction.
//!
//! Archives may contain entries typed as symlinks. Extracting them without
//! validation allows an attacker to create a symlink to an arbitrary destination
//! that subsequent entries overwrite, escaping the extraction root.
//!
//! This guard rejects all symlink entries during extraction per ADR-0004 and the
//! Gherkin scenario in
//! `docs/arch/specs/features/archive/archive-symlink-member-blocked.feature`.

use substrate_domain::{SubstrateError, SubstrateResult};

/// Entry type passed to the guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// Regular file entry.
    File,
    /// Directory entry.
    Directory,
    /// Symbolic link entry — always rejected by this guard.
    Symlink,
    /// Any other entry type (hard link, device, etc.).
    Other,
}

/// Validates that `kind` is not a symlink.
///
/// # Errors
///
/// - `SUBSTRATE_SYMLINK_ESCAPE` — `kind == EntryKind::Symlink`.
pub fn reject_symlink_entry(kind: EntryKind, member_path: &str) -> SubstrateResult<()> {
    if kind == EntryKind::Symlink {
        return Err(SubstrateError::SymlinkEscape {
            path: member_path.to_owned(),
            correlation_id: None,
        });
    }
    Ok(())
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
    fn symlink_entry_is_rejected() {
        let err = reject_symlink_entry(EntryKind::Symlink, "link.txt").unwrap_err();
        assert!(matches!(err, SubstrateError::SymlinkEscape { .. }));
    }

    #[test]
    fn other_entry_kind_is_accepted() {
        assert!(reject_symlink_entry(EntryKind::Other, "device").is_ok());
    }
}
