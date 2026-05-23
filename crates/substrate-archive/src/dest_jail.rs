//! Shared destination path-jailing for create-side archive handlers.
//!
//! Create-side tools (`archive.tar.create`, `archive.zip.create`,
//! `archive.gzip.compress`, `archive.gzip.decompress`) write a NEW file whose
//! path does not exist yet. Both the macOS `O_NOFOLLOW_ANY` jail and the
//! userspace `strict-path` jail canonicalize via `open`/`stat`, which fails with
//! `SUBSTRATE_NOT_FOUND` on a non-existent leaf component. Jailing the
//! destination path directly is therefore a bug: the brand-new output file can
//! never be validated because it has not been created yet.
//!
//! The correct pattern (ADR-0033 transactional writes / ADR-0035 path safety)
//! is to jail the destination's PARENT directory — which must already exist —
//! and then reconstruct the destination beneath the verified parent. Appending
//! a single filename component to a `JailedPath` cannot escape the jail.

#![expect(
    clippy::redundant_pub_crate,
    reason = "private module: pub(crate) signals crate-internal intent explicitly"
)]

use std::path::Path;

use substrate_domain::{JailedPath, PathJailPort, SubstrateError, SubstrateResult};

/// Jails a not-yet-existing destination path by validating its parent directory.
///
/// Returns a [`JailedPath`] pointing at `<jailed_parent>/<filename>`, suitable
/// for a transactional temp-write + atomic rename via [`crate::tmp_path::TmpPath`].
///
/// This function is synchronous; callers that run inside a tokio runtime must
/// invoke it from within `spawn_blocking` because the underlying jail performs
/// blocking filesystem syscalls.
///
/// # Errors
///
/// - [`SubstrateError::InvalidArgument`] when `dest` has no parent directory or
///   no filename component.
/// - Any jail error propagated from validating the parent directory (e.g.
///   `SUBSTRATE_PATH_OUTSIDE_ALLOWLIST`, `SUBSTRATE_SYMLINK_ESCAPE`).
pub(crate) fn jail_dest_via_parent(
    jail: &dyn PathJailPort,
    dest: &Path,
) -> SubstrateResult<JailedPath> {
    let parent = dest
        .parent()
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "dest".to_owned(),
            reason: "destination path has no parent directory".to_owned(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
    let filename = dest
        .file_name()
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "dest".to_owned(),
            reason: "destination path has no filename component".to_owned(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;

    // The parent must already exist for the jail to canonicalize it.
    let jailed_parent = jail.jail(&JailedPath::new_jailed(parent.to_path_buf()), parent)?;

    // SAFETY (semantic): `jailed_parent` is verified within the allowlist;
    // appending a plain filename component cannot escape the jail.
    Ok(JailedPath::new_jailed(
        jailed_parent.as_path().join(filename),
    ))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Jail double mirroring real behaviour: `canonicalize` fails on a
    /// non-existent path, so jailing a brand-new dest file directly errors.
    struct CanonicalizeJail;
    impl PathJailPort for CanonicalizeJail {
        fn jail(&self, _: &JailedPath, raw: &Path) -> SubstrateResult<JailedPath> {
            let canon = std::fs::canonicalize(raw).map_err(|_| SubstrateError::NotFound {
                resource: raw.to_string_lossy().into_owned(),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })?;
            Ok(JailedPath::new_jailed(canon))
        }
    }

    #[test]
    fn jails_nonexistent_dest_via_existing_parent() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("brand-new.gz"); // does NOT exist yet
        let jailed = jail_dest_via_parent(&CanonicalizeJail, &dest)
            .expect("non-existent dest must jail via its existing parent");
        assert_eq!(
            jailed.as_path().file_name(),
            Some(std::ffi::OsStr::new("brand-new.gz"))
        );
        // Parent component is the canonicalized temp dir.
        assert!(jailed.as_path().parent().is_some());
    }

    #[test]
    fn rejects_dest_without_parent() {
        // The filesystem root has no parent.
        let err = jail_dest_via_parent(&CanonicalizeJail, Path::new("/")).unwrap_err();
        assert!(matches!(err, SubstrateError::InvalidArgument { .. }));
    }
}
