//! Allowlist — canonical set of approved path roots per ADR-0004.
//!
//! Roots are canonicalized at construction time. Any root that is a symlink,
//! does not exist, or is otherwise unresolvable causes construction to fail
//! with the appropriate `SubstrateError` variant.

use std::path::{Path, PathBuf};

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::nfc;

/// Validated, canonicalized set of allowlist roots.
///
/// Constructed once at startup from the operator-supplied `[security] roots`
/// TOML list. All subsequent path-jail checks reference this set.
#[derive(Debug, Clone)]
pub struct Allowlist {
    /// Canonicalized absolute roots. Sorted for deterministic iteration.
    roots: Vec<PathBuf>,
}

impl Allowlist {
    /// Constructs an `Allowlist` from the given raw root paths.
    ///
    /// Each root is canonicalized via [`std::fs::canonicalize`]. The
    /// function returns an error on the first root that:
    ///
    /// - Does not exist (`AllowlistRootMissing`).
    /// - Is a symlink (`ConfigInvalid` — allowlist roots must not be symlinks
    ///   per ADR-0035 §Decision 5).
    /// - Cannot be read (`AllowlistRootUnreadable`).
    ///
    /// An empty `roots` slice is rejected with `ConfigInvalid`.
    ///
    /// # Errors
    ///
    /// Returns a [`SubstrateError`] if any root fails validation.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Vec<PathBuf> matches the public API used at startup; callers own the roots Vec"
    )]
    pub fn new(roots: Vec<PathBuf>) -> SubstrateResult<Self> {
        if roots.is_empty() {
            return Err(SubstrateError::ConfigInvalid {
                offending_field: "security.roots".to_owned(),
                correlation_id: None,
            });
        }

        let mut canonical_roots = Vec::with_capacity(roots.len());

        for root in &roots {
            // Reject symlinks in allowlist roots per ADR-0035 §Decision 5.
            let metadata = root.symlink_metadata().map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    SubstrateError::AllowlistRootMissing {
                        root: root.display().to_string(),
                        correlation_id: None,
                    }
                } else {
                    SubstrateError::AllowlistRootUnreadable {
                        root: root.display().to_string(),
                        correlation_id: None,
                    }
                }
            })?;

            if metadata.file_type().is_symlink() {
                return Err(SubstrateError::ConfigInvalid {
                    offending_field: format!(
                        "security.roots entry '{}' is a symlink; allowlist roots must be canonical directories",
                        root.display()
                    ),
                    correlation_id: None,
                });
            }

            let canonical = std::fs::canonicalize(root).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    SubstrateError::AllowlistRootMissing {
                        root: root.display().to_string(),
                        correlation_id: None,
                    }
                } else {
                    SubstrateError::AllowlistRootUnreadable {
                        root: root.display().to_string(),
                        correlation_id: None,
                    }
                }
            })?;

            // Normalize the canonicalized root to NFC per ADR-0035 §Decision 6
            // so prefix containment is stable against NFC/NFD divergence on
            // macOS APFS/HFS+ volumes. Both stored roots and incoming
            // candidates are compared in NFC form.
            canonical_roots.push(nfc::normalize_path(&canonical));
        }

        canonical_roots.sort();
        Ok(Self {
            roots: canonical_roots,
        })
    }

    /// Returns `true` when `candidate` is a descendant of (or equal to) any
    /// configured allowlist root.
    ///
    /// The candidate is normalized to NFC (ADR-0035 §Decision 6) before the
    /// prefix comparison so that an NFD-encoded argument matches an
    /// NFC-encoded root on macOS APFS/HFS+ volumes. Callers must canonicalize
    /// `candidate` before invoking this method; this method only handles
    /// Unicode normalization and prefix containment.
    #[must_use]
    pub fn contains(&self, candidate: &Path) -> bool {
        let normalized = nfc::normalize_path(candidate);
        self.roots.iter().any(|root| normalized.starts_with(root))
    }

    /// Validates `candidate` against the allowlist and returns a [`JailedPath`].
    ///
    /// The caller is responsible for ensuring `candidate` is already
    /// canonicalized (no `..` segments, no unresolved symlinks). This method
    /// only performs the prefix containment check; deeper kernel-level jailing
    /// is performed by the [`PathJailPort`](substrate_domain::PathJailPort)
    /// implementation selected by the factory.
    ///
    /// The returned [`JailedPath`] carries the NFC-normalized form of
    /// `candidate` (ADR-0035 §Decision 6) so that all subsequent internal path
    /// operations use a single, consistent encoding.
    ///
    /// # Errors
    ///
    /// - `PathOutsideAllowlist` — `candidate` is not under any root.
    pub fn jail(&self, candidate: PathBuf) -> SubstrateResult<JailedPath> {
        let normalized = nfc::normalize_path(&candidate);
        if self.contains(&normalized) {
            // SAFETY (semantic): `JailedPath::new_jailed` is documented as
            // `substrate-policy`-only. We have verified the path is within an
            // allowlist root; the kernel-level jail check (openat2 /
            // O_NOFOLLOW_ANY) is the caller's responsibility before invoking
            // this method. Misuse by other crates is caught by
            // `policies/path_jail_construction.rego` in CI.
            Ok(JailedPath::new_jailed(normalized))
        } else {
            Err(SubstrateError::PathOutsideAllowlist {
                path: normalized.display().to_string(),
                correlation_id: None,
            })
        }
    }

    /// Returns an iterator over the canonicalized allowlist roots.
    pub fn iter_roots(&self) -> impl Iterator<Item = &Path> {
        self.roots.iter().map(PathBuf::as_path)
    }
}

// ---- Tests ------------------------------------------------------------------

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
    use tempfile::TempDir;

    fn make_tmpdir() -> TempDir {
        tempfile::tempdir().expect("tempdir creation must succeed in tests")
    }

    #[test]
    fn rejects_empty_roots() {
        let result = Allowlist::new(vec![]);
        assert!(result.is_err(), "empty root list must be rejected");
        let err = result.unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_CONFIG_INVALID");
    }

    #[test]
    fn contains_child_path() {
        let dir = make_tmpdir();
        // Canonicalize the tempdir root so it matches the path stored inside
        // `Allowlist` (on macOS `/var/folders/...` resolves to
        // `/private/var/folders/...`).
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        let allowlist = Allowlist::new(vec![root.clone()]).expect("valid root");

        let child = root.join("subdir").join("file.txt");
        assert!(allowlist.contains(&child));
    }

    #[test]
    fn rejects_path_outside_root() {
        let dir = make_tmpdir();
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        let allowlist = Allowlist::new(vec![root.clone()]).expect("valid root");

        // A sibling directory must not match the root.
        let outside = root
            .parent()
            .expect("tempdir must have a parent")
            .join("other_dir");
        assert!(!allowlist.contains(&outside));
    }

    #[test]
    fn jail_returns_jailed_path_for_allowed() {
        let dir = make_tmpdir();
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        let allowlist = Allowlist::new(vec![root.clone()]).expect("valid root");

        let child = root.join("readme.txt");
        let result = allowlist.jail(child.clone());
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_path(), child.as_path());
    }

    #[test]
    fn jail_rejects_outside_path() {
        let dir = make_tmpdir();
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        let allowlist = Allowlist::new(vec![root.clone()]).expect("valid root");

        let outside = root
            .parent()
            .expect("tempdir must have a parent")
            .join("escape.txt");
        let result = allowlist.jail(outside);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code(),
            "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"
        );
    }

    #[test]
    fn contains_matches_nfd_candidate_under_nfc_root() {
        let dir = make_tmpdir();
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        let allowlist = Allowlist::new(vec![root.clone()]).expect("valid root");

        // NFC root component "café" vs NFD child "cafe\u{0301}" must match
        // after NFC normalization (ADR-0035 §Decision 6).
        let nfc_child = root.join("caf\u{00e9}").join("file.txt");
        let nfd_child = root.join("cafe\u{0301}").join("file.txt");
        assert_ne!(nfc_child, nfd_child, "precondition: byte strings differ");
        assert!(allowlist.contains(&nfc_child));
        assert!(
            allowlist.contains(&nfd_child),
            "NFD-encoded candidate must match after NFC normalization"
        );
    }

    #[test]
    fn rejects_missing_root() {
        let non_existent = PathBuf::from("/tmp/__substrate_policy_nonexistent_test_root__");
        let result = Allowlist::new(vec![non_existent]);
        assert!(result.is_err());
        let code = result.unwrap_err().code();
        assert!(
            code == "SUBSTRATE_ALLOWLIST_ROOT_MISSING"
                || code == "SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE",
            "unexpected error code: {code}"
        );
    }
}
