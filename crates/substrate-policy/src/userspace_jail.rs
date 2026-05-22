//! Userspace-degraded `PathJail` tier using `strict-path` + post-check.
//!
//! This tier is used when neither `openat2` (Linux ≥ 5.6) nor
//! `O_NOFOLLOW_ANY` (macOS ≥ 12) is available. It does NOT atomically close
//! the TOCTOU window; a symlink swap between `canonicalize` and the eventual
//! `open(2)` can redirect an operation outside the allowlist.
//!
//! When `security.refuse_degraded_jail = true` (default per ADR-0035
//! amendment), the composition root aborts startup with
//! `SUBSTRATE_JAIL_DEGRADED_REFUSED` before any tool call is accepted.
//!
//! A `tracing::warn!` is emitted at construction time per ADR-0042.

use std::path::Path;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::allowlist::Allowlist;

/// Userspace path-jail adapter using `strict-path` canonicalization and an
/// allowlist prefix check.
///
/// # Security posture
///
/// Non-zero TOCTOU window. Only use when kernel tier 1 is unavailable and
/// the operator has explicitly set `security.refuse_degraded_jail = false`.
#[expect(
    clippy::redundant_pub_crate,
    reason = "pub(crate) documents intentional crate-internal visibility for cross-module use"
)]
pub(crate) struct UserspaceJail {
    allowlist: Allowlist,
}

impl UserspaceJail {
    /// Creates a new `UserspaceJail` wrapping the given allowlist.
    ///
    /// Emits `tracing::warn!` at construction time per ADR-0042 degraded-tier
    /// policy to make the security regression visible in startup logs.
    #[must_use]
    pub(crate) fn new(allowlist: Allowlist) -> Self {
        tracing::warn!(
            tier = "userspace-degraded",
            "PathJail running in degraded userspace tier — TOCTOU window is not atomically closed. \
             Upgrade to Linux ≥ 5.6 or macOS ≥ 12, or set security.refuse_degraded_jail = true \
             to refuse degraded startup."
        );
        Self { allowlist }
    }
}

impl substrate_domain::PathJailPort for UserspaceJail {
    fn jail(&self, allowlist_root: &JailedPath, raw_path: &Path) -> SubstrateResult<JailedPath> {
        // Blanket rejection of /proc paths on Linux per ADR-0035 §Decision 8.
        #[cfg(target_os = "linux")]
        if raw_path.starts_with("/proc") {
            return Err(SubstrateError::PathOutsideAllowlist {
                path: raw_path.display().to_string(),
                correlation_id: None,
            });
        }

        // PATH_MAX validation per ADR-0035 §Decision 10.
        // libc::PATH_MAX: 4096 on Linux (including NUL), 1024 on macOS.
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            // We bound at 4095 (Linux) / 1023 (macOS) usable bytes.
            #[cfg(target_os = "linux")]
            const PATH_MAX_USABLE: usize = 4095;
            #[cfg(target_os = "macos")]
            const PATH_MAX_USABLE: usize = 1023;

            let byte_len = raw_path.as_os_str().len();
            if byte_len > PATH_MAX_USABLE {
                return Err(SubstrateError::InvalidArgument {
                    offending_field: "path".to_owned(),
                    reason: format!("path length {byte_len} exceeds PATH_MAX ({PATH_MAX_USABLE})"),
                    correlation_id: None,
                });
            }
        }

        // Canonicalize using the standard library (userspace — TOCTOU risk).
        let canonical = std::fs::canonicalize(raw_path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => SubstrateError::NotFound {
                resource: raw_path.display().to_string(),
                correlation_id: None,
            },
            std::io::ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
                path: raw_path.display().to_string(),
                correlation_id: None,
            },
            _ => SubstrateError::IoError {
                path: raw_path.display().to_string(),
                correlation_id: None,
            },
        })?;

        // Verify the canonicalized path is beneath the specific root passed to
        // this call (may be a subset of the full allowlist).
        if !canonical.starts_with(allowlist_root.as_path()) {
            return Err(SubstrateError::PathOutsideAllowlist {
                path: canonical.display().to_string(),
                correlation_id: None,
            });
        }

        // Cross-check against the full allowlist.
        self.allowlist.jail(canonical)
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
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::allowlist::Allowlist;

    fn setup() -> (TempDir, UserspaceJail, JailedPath) {
        let dir = tempfile::tempdir().expect("tempdir creation must succeed in tests");
        let root_buf = dir
            .path()
            .canonicalize()
            .expect("tempdir canonicalization must succeed");
        let allowlist = Allowlist::new(vec![root_buf.clone()]).expect("valid allowlist");
        // SAFETY (semantic): constructing a root JailedPath directly here only
        // for test setup; the root itself was just validated above.
        let root_jailed = JailedPath::new_jailed(root_buf);
        // UserspaceJail::new emits a tracing::warn! — that is expected in tests.
        let jail = UserspaceJail::new(allowlist);
        (dir, jail, root_jailed)
    }

    #[test]
    fn allows_file_within_root() {
        use substrate_domain::PathJailPort as _;

        let (dir, jail, root_jailed) = setup();
        // Create a real file inside the tmpdir so canonicalize succeeds.
        let file_path = dir.path().join("allowed.txt");
        std::fs::write(&file_path, b"ok").expect("write must succeed");

        let result = jail.jail(&root_jailed, &file_path);
        assert!(
            result.is_ok(),
            "file within root must be allowed: {result:?}"
        );
    }

    #[test]
    fn rejects_path_outside_root() {
        use substrate_domain::PathJailPort as _;

        let (_dir, jail, root_jailed) = setup();
        // /tmp itself is outside the specific tempdir root.
        let outside = PathBuf::from("/tmp");

        let result = jail.jail(&root_jailed, &outside);
        assert!(result.is_err(), "path outside root must be rejected");
        let code = result.unwrap_err().code();
        assert!(
            code == "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST" || code == "SUBSTRATE_NOT_FOUND",
            "unexpected error code: {code}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_proc_path() {
        use substrate_domain::PathJailPort as _;

        let (_dir, jail, root_jailed) = setup();
        let proc_path = PathBuf::from("/proc/self/cwd");

        let result = jail.jail(&root_jailed, &proc_path);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code(),
            "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"
        );
    }
}
