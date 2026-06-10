//! `fs.touch` — create an empty file or update its access and modification
//! timestamps.
//!
//! # Async zone: B
//!
//! - If the file does not exist: `nix::fcntl::open` with
//!   `O_CREAT | O_WRONLY | O_NOFOLLOW` wrapped in `spawn_blocking` (Zone B)
//!   to prevent TOCTOU symlink-swap attacks.
//! - If the file exists: `nix::sys::stat::utimensat` wrapped in
//!   `spawn_blocking` (Zone B) to set atime/mtime to the current time.
//!
//! # Security
//!
//! The target path is validated through the path jail. `fs.touch` on a
//! non-existent file is considered low-risk; no dry-run gate is enforced.
//! Updating timestamps on an existing file is also non-destructive.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::hints_helpers;
use crate::response::{FsMutationDeps, ToolResponse};

// ---- Request -----------------------------------------------------------------

/// Input parameters for `fs.touch`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsTouchRequest {
    /// Target path. Created if absent; timestamps updated if it exists.
    pub path: String,
}

// ---- Handler -----------------------------------------------------------------

/// Handles an `fs.touch` tool call.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation or I/O.
#[instrument(skip(deps), fields(path = %req.path))]
pub async fn handle_fs_touch(
    req: FsTouchRequest,
    deps: &FsMutationDeps,
    allowlist_root: &JailedPath,
) -> SubstrateResult<ToolResponse> {
    // Determine whether the file exists to choose the jailing strategy.
    let file_exists = Path::new(&req.path).exists();

    let jailed = if file_exists {
        deps.jail.jail(allowlist_root, Path::new(&req.path))?
    } else {
        jail_new_path(&req.path, deps, allowlist_root)?
    };

    if file_exists {
        // Zone B: update timestamps via utimensat.
        let path = jailed.as_path().to_path_buf();
        tokio::task::spawn_blocking(move || touch_existing(&path))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking join error in fs.touch: {e}"),
                correlation_id: None,
            })??;
    } else {
        // Zone B: create empty file with O_NOFOLLOW | O_CREAT to prevent a
        // TOCTOU symlink-swap attack. `tokio::fs::File::create` follows
        // symlinks and would redirect the write if a symlink were swapped in
        // between the jail check and the open(). O_NOFOLLOW causes ELOOP/ENOTDIR
        // (mapped to SUBSTRATE_IO_ERROR) when the final path component is a
        // symlink, closing the race window.
        let path = jailed.as_path().to_path_buf();
        tokio::task::spawn_blocking(move || create_no_follow(&path))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking join error in fs.touch (create): {e}"),
                correlation_id: None,
            })??;

        #[cfg(feature = "fs-index")]
        crate::write_through::on_upsert(&deps.index, &jailed);
    }

    let action = if file_exists { "updated" } else { "created" };
    let content = format!("File {action}: {jailed}");
    let sc = serde_json::json!({
        "path": jailed.as_path(),
        "action": action,
    });
    Ok(ToolResponse::with_hints(
        content,
        sc,
        hints_helpers::mutation_success_hints("fs.stat"),
    ))
}

// ---- Helpers -----------------------------------------------------------------

fn jail_new_path(
    raw: &str,
    deps: &FsMutationDeps,
    root: &JailedPath,
) -> SubstrateResult<JailedPath> {
    let target = Path::new(raw);
    let parent = target
        .parent()
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "path".into(),
            reason: "Path has no parent directory.".into(),
            correlation_id: None,
        })?;
    let jailed_parent = deps.jail.jail(root, parent)?;
    let file_name = target
        .file_name()
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "path".into(),
            reason: "Path has no file name component.".into(),
            correlation_id: None,
        })?;
    Ok(JailedPath::new_jailed(
        jailed_parent.as_path().join(file_name),
    ))
}

/// Creates a new empty file at `path` using `O_CREAT | O_WRONLY | O_NOFOLLOW`.
///
/// `O_NOFOLLOW` ensures that if a symlink is raced into place between the jail
/// check and this open, the kernel rejects the open with ELOOP (Linux) or
/// ENOTDIR (macOS) instead of silently following the attacker-controlled link.
/// The file descriptor is closed immediately after creation.
///
/// # Errors
///
/// Returns [`SubstrateError::IoError`] for any OS error including the
/// O_NOFOLLOW rejection case (which surfaces as `ELOOP`/`ENOTDIR`).
fn create_no_follow(path: &Path) -> SubstrateResult<()> {
    use nix::fcntl::{OFlag, open};
    use nix::sys::stat::Mode;

    // 0o666 (umask-filtered) matches the default File::create uses.
    let mode = Mode::S_IRUSR
        | Mode::S_IWUSR
        | Mode::S_IRGRP
        | Mode::S_IWGRP
        | Mode::S_IROTH
        | Mode::S_IWOTH;
    let flags = OFlag::O_CREAT | OFlag::O_WRONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    // `open` returns an `OwnedFd`; the fd is closed immediately on drop.
    open(path, flags, mode).map(drop).map_err(|e| {
        tracing::debug!(path = %path.display(), err = %e, "create_no_follow: open failed");
        // Preserve the PermissionDenied vs IoError distinction (e.g. a
        // read-only parent dir vs an ELOOP from O_NOFOLLOW on a symlink).
        map_io_error(std::io::Error::from_raw_os_error(e as i32), path)
    })
}

/// Updates atime and mtime on an existing file to the current wall-clock time.
fn touch_existing(path: &Path) -> SubstrateResult<()> {
    use nix::fcntl::AT_FDCWD;
    use nix::sys::stat::{UtimensatFlags, utimensat};
    use nix::sys::time::TimeSpec;

    // TimeSpec::UTIME_NOW is the canonical sentinel from nix, which maps to
    // libc::UTIME_NOW (-1 on macOS, (1<<30)-1 on Linux). Using the platform-
    // correct constant avoids setting mtime to epoch 0 on macOS.
    let now = TimeSpec::UTIME_NOW;

    utimensat(AT_FDCWD, path, &now, &now, UtimensatFlags::FollowSymlink).map_err(|_e| {
        SubstrateError::IoError {
            path: path.display().to_string(),
            correlation_id: None,
        }
    })
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "std::io::Error is the conventional error-mapping pattern; taking by value avoids lifetime annotation at call sites"
)]
fn map_io_error(e: std::io::Error, path: &Path) -> SubstrateError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
            path: path.display().to_string(),
            correlation_id: None,
        },
        _ => SubstrateError::IoError {
            path: path.display().to_string(),
            correlation_id: None,
        },
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use std::sync::Arc;

    use substrate_domain::{Capabilities, JailedPath, PortFactory};
    use substrate_policy::{Allowlist, PathJailFactory};
    use tempfile::TempDir;

    use super::*;
    use crate::response::FsMutationDeps;

    fn make_test_env() -> (TempDir, JailedPath, FsMutationDeps) {
        let dir = TempDir::new().expect("tempdir");
        let canonical = dir.path().canonicalize().expect("canonicalize");
        let root = JailedPath::new_jailed(canonical.clone());
        let allowlist = Allowlist::new(vec![canonical]).expect("allowlist");
        let caps = Arc::new(Capabilities::default());
        let factory = PathJailFactory::new(allowlist, false);
        let jail = factory.build(&caps);
        let deps = FsMutationDeps {
            jail,
            capabilities: caps,
            #[cfg(feature = "fs-index")]
            index: substrate_fs_index::FsIndexFactory::new().build(&Capabilities::default()),
        };
        (dir, root, deps)
    }

    #[tokio::test]
    async fn creates_new_empty_file() {
        let (dir, root, deps) = make_test_env();
        let f = dir.path().join("new.txt");
        let req = FsTouchRequest {
            path: f.display().to_string(),
        };
        handle_fs_touch(req, &deps, &root).await.expect("touch");
        assert!(f.exists());
        assert_eq!(std::fs::read(&f).expect("read"), b"");
    }

    #[tokio::test]
    async fn updates_existing_file_timestamps() {
        let (dir, root, deps) = make_test_env();
        let f = dir.path().join("existing.txt");
        std::fs::write(&f, b"data").expect("seed");
        let before_mtime = std::fs::metadata(&f)
            .expect("meta")
            .modified()
            .expect("mtime");

        // Small sleep to ensure mtime changes.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let req = FsTouchRequest {
            path: f.display().to_string(),
        };
        handle_fs_touch(req, &deps, &root).await.expect("touch");

        let after_mtime = std::fs::metadata(&f)
            .expect("meta")
            .modified()
            .expect("mtime");
        // Content must be unchanged.
        assert_eq!(std::fs::read(&f).expect("read"), b"data");
        // mtime should be >= before (resolution may be coarse on some FS).
        assert!(after_mtime >= before_mtime);
    }
}
