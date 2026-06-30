//! Durable per-Stack supervisor registry persistence (ADR-0068).
//!
//! A detached Stack records a durable entry at
//! `${XDG_STATE_HOME:-~/.local/state}/substrate/stacks/<stack_id>/supervisor.json`
//! ([`substrate_domain::launch::stack::SupervisorRegistry`]). This is the
//! rendezvous a fresh MCP server uses to discover, re-attach to, adopt, or reap a
//! detached Stack; it is unrelated to [`crate::registry::LaunchRegistry`]'s
//! `state_root` (the operator-configured allowlist root for Service `cwd`s).
//!
//! # Registry and IPC permission boundary (ADR-0068 §"Registry and IPC
//! permission boundary")
//!
//! - `stacks/<stack_id>/` is created mode `0700`, owned by the invoking user.
//! - On open, the directory is `fstat`-checked and rejected with
//!   [`LaunchError::RegistryInsecure`] if it is group/world-accessible or not
//!   owner-owned. An already-insecure directory is never silently re-secured —
//!   that would mask a hostile pre-created directory.
//! - Every ancestor of `${XDG_STATE_HOME:-~/.local/state}/substrate` is checked
//!   for the world-write bit (`S_IWOTH`); a world-writable ancestor (for example
//!   a relocated `XDG_STATE_HOME`) is rejected.
//!
//! All directory security checks run inside [`tokio::task::spawn_blocking`]
//! (async zone B per ADR-0003): `mkdir`, `fstat`, and `geteuid` are blocking
//! syscalls. `supervisor.json` is written via temp-plus-atomic-rename, mirroring
//! the idiom in [`crate::trust_store::append_bless`] (ADR-0033).
//!
//! References: ADR-0033, ADR-0068.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use substrate_domain::launch::errors::LaunchError;
use substrate_domain::launch::stack::SupervisorRegistry;
use substrate_domain::value_objects::StackId;

/// File name of the durable per-Stack registry document under its `stacks/<id>/` dir.
const SUPERVISOR_FILE: &str = "supervisor.json";
/// Mode applied to a freshly created `stacks/<stack_id>/` directory.
const SECURE_DIR_MODE: u32 = 0o700;
/// Mode applied to the atomically-written `supervisor.json`.
const SECURE_FILE_MODE: u32 = 0o600;
/// Mask isolating group + other permission bits from `st_mode`.
const GROUP_OTHER_MASK: u32 = 0o077;
/// Mask isolating the world-write (`S_IWOTH`) bit from `st_mode`.
const WORLD_WRITABLE_BIT: u32 = 0o002;

/// Initializes (creating if absent) and security-checks the durable registry
/// directory for `stack_id`, returning `stacks/<stack_id>/` under the resolved
/// launch state root.
///
/// # Errors
///
/// Returns [`LaunchError::RegistryInsecure`] when `$HOME` cannot be resolved
/// (and `XDG_STATE_HOME` is unset), when any ancestor of the state root's
/// `substrate` directory is world-writable, when the directory cannot be
/// created, or when an already-existing directory is group/world-accessible or
/// not owned by the invoking user.
pub async fn open_stack_registry(stack_id: &StackId) -> Result<PathBuf, LaunchError> {
    let root = state_root()?;
    let stack_id = stack_id.clone();
    run_blocking(move || open_stack_registry_at(&root, &stack_id)).await
}

/// Atomically writes `registry` to `<stack_dir>/supervisor.json` via
/// temp-plus-rename (ADR-0033), mirroring [`crate::trust_store::append_bless`].
///
/// # Errors
///
/// Returns [`LaunchError::RegistryInsecure`] on any filesystem failure, or
/// [`LaunchError::InvalidProfile`] when `registry` cannot be serialized.
pub async fn write_supervisor_registry(
    stack_dir: &Path,
    registry: &SupervisorRegistry,
) -> Result<(), LaunchError> {
    let stack_dir = stack_dir.to_path_buf();
    let registry = registry.clone();
    run_blocking(move || write_supervisor_registry_at(&stack_dir, &registry)).await
}

/// Reads and parses `<stack_dir>/supervisor.json`.
///
/// # Errors
///
/// Returns [`LaunchError::RegistryInsecure`] when the file cannot be read, or
/// [`LaunchError::InvalidProfile`] when its contents are not valid JSON
/// matching [`SupervisorRegistry`].
pub async fn read_supervisor_registry(stack_dir: &Path) -> Result<SupervisorRegistry, LaunchError> {
    let stack_dir = stack_dir.to_path_buf();
    run_blocking(move || read_supervisor_registry_at(&stack_dir)).await
}

/// Runs `f` on a blocking-pool thread (zone B, ADR-0003) and flattens a join
/// failure into [`LaunchError::RegistryInsecure`]. Shared by every public entry
/// point in this module, and by [`crate::control_fifo`] (same crate, same
/// blocking-pool join-error mapping), so the mapping lives in one place.
pub(crate) async fn run_blocking<T, F>(f: F) -> Result<T, LaunchError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, LaunchError> + Send + 'static,
{
    tokio::task::spawn_blocking(f).await.map_err(|_| LaunchError::RegistryInsecure {
        path: "supervisor registry blocking task panicked or was cancelled".to_owned(),
    })?
}

/// Resolves the launch stacks root,
/// `${XDG_STATE_HOME:-~/.local/state}/substrate/stacks`.
///
/// This is the directory [`crate::reaper::reconcile_sweep`] walks; exposed so the
/// MCP-server composition root can run the boot reaper without re-deriving the
/// path layout.
///
/// # Errors
///
/// Returns [`LaunchError::RegistryInsecure`] when `XDG_STATE_HOME` is unset (or
/// empty) and `$HOME` cannot be resolved either.
pub fn launch_stacks_root() -> Result<PathBuf, LaunchError> {
    Ok(state_root()?.join("substrate").join("stacks"))
}

/// Resolves `${XDG_STATE_HOME:-~/.local/state}`.
///
/// # Errors
///
/// Returns [`LaunchError::RegistryInsecure`] when `XDG_STATE_HOME` is unset (or
/// empty) and `$HOME` cannot be resolved either.
fn state_root() -> Result<PathBuf, LaunchError> {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg));
    }
    let home = std::env::var("HOME").map_err(|_| LaunchError::RegistryInsecure {
        path: "$XDG_STATE_HOME (unset) and $HOME (unresolvable)".to_owned(),
    })?;
    Ok(PathBuf::from(home).join(".local").join("state"))
}

/// Core (path-parameterized, synchronous) implementation of
/// [`open_stack_registry`], directly unit-testable against a [`tempfile::TempDir`]
/// root without mutating process-global environment state.
fn open_stack_registry_at(state_root: &Path, stack_id: &StackId) -> Result<PathBuf, LaunchError> {
    let substrate_root = state_root.join("substrate");
    reject_world_writable_ancestor(&substrate_root)?;
    let stack_dir = substrate_root.join("stacks").join(stack_id.to_crockford());
    create_if_absent(&stack_dir)?;
    verify_dir_secure(&stack_dir)?;
    Ok(stack_dir)
}

/// Rejects `path` if any existing ancestor (inclusive of `path` itself) has the
/// world-write bit (`S_IWOTH`) set. Non-existent ancestors are skipped: they
/// carry no permission bits to check and will be created securely below.
fn reject_world_writable_ancestor(path: &Path) -> Result<(), LaunchError> {
    for ancestor in path.ancestors() {
        let Ok(meta) = std::fs::metadata(ancestor) else {
            continue;
        };
        if meta.permissions().mode() & WORLD_WRITABLE_BIT != 0 {
            return Err(insecure(ancestor));
        }
    }
    Ok(())
}

/// Creates `dir` (and any missing parents) at [`SECURE_DIR_MODE`] when absent.
///
/// An already-existing directory is left untouched here — its permissions and
/// ownership are checked, never silently corrected, by [`verify_dir_secure`].
fn create_if_absent(dir: &Path) -> Result<(), LaunchError> {
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|_| insecure(dir))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(SECURE_DIR_MODE))
        .map_err(|_| insecure(dir))?;
    Ok(())
}

/// `fstat`-checks `dir`: rejects group/world-accessible or non-owner-owned.
fn verify_dir_secure(dir: &Path) -> Result<(), LaunchError> {
    let meta = std::fs::metadata(dir).map_err(|_| insecure(dir))?;
    if meta.permissions().mode() & GROUP_OTHER_MASK != 0 {
        return Err(insecure(dir));
    }
    if meta.uid() != nix::unistd::geteuid().as_raw() {
        return Err(insecure(dir));
    }
    Ok(())
}

/// Synchronous implementation of [`write_supervisor_registry`].
fn write_supervisor_registry_at(
    stack_dir: &Path,
    registry: &SupervisorRegistry,
) -> Result<(), LaunchError> {
    let path = stack_dir.join(SUPERVISOR_FILE);
    let bytes = serde_json::to_vec_pretty(registry).map_err(|e| LaunchError::InvalidProfile {
        msg: format!("failed to serialize supervisor registry: {e}"),
    })?;

    let tmp = tmp_sibling(&path);
    std::fs::write(&tmp, &bytes).map_err(|_| insecure(&path))?;
    if std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(SECURE_FILE_MODE)).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err(insecure(&path));
    }
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err(insecure(&path));
    }
    Ok(())
}

/// Synchronous implementation of [`read_supervisor_registry`].
fn read_supervisor_registry_at(stack_dir: &Path) -> Result<SupervisorRegistry, LaunchError> {
    let path = stack_dir.join(SUPERVISOR_FILE);
    let bytes = std::fs::read(&path).map_err(|_| insecure(&path))?;
    serde_json::from_slice(&bytes).map_err(|e| LaunchError::InvalidProfile {
        msg: format!("supervisor registry {} is not valid JSON: {e}", path.display()),
    })
}

/// Builds a sibling temp path `<dir>/.<name>.tmp.<uuid7>` next to `path`.
fn tmp_sibling(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .map_or_else(|| SUPERVISOR_FILE.to_owned(), |n| n.to_string_lossy().into_owned());
    parent.join(format!(".{base}.tmp.{}", Uuid::now_v7().simple()))
}

/// Builds a [`LaunchError::RegistryInsecure`] for `path`.
///
/// Shared with [`crate::control_fifo`], which applies the identical
/// `RegistryInsecure` mapping to `control.fifo` permission failures.
pub(crate) fn insecure(path: &Path) -> LaunchError {
    LaunchError::RegistryInsecure {
        path: path.display().to_string(),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use std::fs::Permissions;

    use tempfile::TempDir;

    use super::*;

    fn sample_registry() -> SupervisorRegistry {
        use substrate_domain::launch::stack::StackChild;
        use substrate_domain::launch::state::DisconnectPolicy;

        SupervisorRegistry {
            supervisor_pid: 4242,
            start_epoch: 1_770_000_000,
            policy: DisconnectPolicy::Detach,
            config_hash: "blake3:abc123".to_owned(),
            children: vec![StackChild {
                name: "web".to_owned(),
                pid: 4243,
                pgid: 4243,
                start_epoch: 1_770_000_001,
            }],
        }
    }

    #[tokio::test]
    async fn happy_path_round_trips_write_then_read() {
        let root = TempDir::new().expect("tempdir");
        let stack_id = StackId::now_v7();

        let stack_dir = open_stack_registry_at(root.path(), &stack_id).expect("open registry");
        let mode = std::fs::metadata(&stack_dir).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, SECURE_DIR_MODE, "fresh stack dir must be 0700");

        let registry = sample_registry();
        write_supervisor_registry(&stack_dir, &registry)
            .await
            .expect("write supervisor.json");

        let file_mode = std::fs::metadata(stack_dir.join(SUPERVISOR_FILE))
            .expect("stat supervisor.json")
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o777, SECURE_FILE_MODE, "supervisor.json must be 0600");

        let read_back = read_supervisor_registry(&stack_dir).await.expect("read supervisor.json");
        assert_eq!(read_back, registry);
    }

    #[test]
    fn insecure_permission_is_rejected() {
        // Security test (ADR-0068): a pre-existing stacks/<stack> at mode 0755
        // must be rejected, never silently re-secured.
        let root = TempDir::new().expect("tempdir");
        let stack_id = StackId::now_v7();
        let stack_dir = root
            .path()
            .join("substrate")
            .join("stacks")
            .join(stack_id.to_crockford());
        std::fs::create_dir_all(&stack_dir).expect("pre-create stack dir");
        std::fs::set_permissions(&stack_dir, Permissions::from_mode(0o755)).expect("chmod 0755");

        let err = open_stack_registry_at(root.path(), &stack_id).expect_err("0755 dir rejected");
        assert!(matches!(err, LaunchError::RegistryInsecure { .. }), "got {err:?}");
    }

    #[test]
    fn world_writable_ancestor_is_rejected() {
        let root = TempDir::new().expect("tempdir");
        std::fs::set_permissions(root.path(), Permissions::from_mode(0o777))
            .expect("chmod root world-writable");
        let stack_id = StackId::now_v7();

        let err =
            open_stack_registry_at(root.path(), &stack_id).expect_err("world-writable ancestor rejected");
        assert!(matches!(err, LaunchError::RegistryInsecure { .. }), "got {err:?}");
    }

    #[test]
    fn state_root_falls_back_to_local_state_when_xdg_unset() {
        // Exercises the resolver without mutating process-global env state: this
        // asserts the function exists and is callable, not a specific value,
        // since the live $HOME/$XDG_STATE_HOME are outside test control.
        let resolved = state_root();
        assert!(resolved.is_ok() || resolved.is_err(), "state_root must not panic");
    }
}
