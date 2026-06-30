//! Safe-open profile loading with the five-step TOFU gate (ADR-0064).
//!
//! [`load_trusted`] runs the ordered gate that closes the time-of-check /
//! time-of-use window:
//!
//! 1. open the config **without following symlinks** (`O_NOFOLLOW`, plus an
//!    `lstat` leaf check that deterministically yields
//!    [`LaunchError::ConfigSymlinkRejected`]);
//! 2. `fstat` the **open descriptor** for the identity tuple `(dev, ino, uid, mode)`;
//! 3. stream BLAKE3 over the **same bytes** read from that descriptor;
//! 4. look the full tuple up in the trust store (`launch.trust` bless records);
//! 5. deserialize from the **in-memory bytes** — the file is never reopened.
//!
//! The parent directory is rejected up front when world-writable
//! ([`LaunchError::ConfigUntrustedDir`]). [`load_untrusted`] runs the same
//! safe-open and parse but applies **no** trust verdict, backing the read-only
//! `launch.list`.
//!
//! # Security deviations (MVP)
//!
//! - `substrate-policy`'s `openat2(RESOLVE_NO_SYMLINKS)` / `O_NOFOLLOW_ANY`
//!   helpers are private (`pub(crate)`), so they cannot be reused here. The MVP
//!   uses a `std`/`tokio` `O_NOFOLLOW` open, which rejects a symlinked **leaf**
//!   but not a symlinked **ancestor** component. Full-path symlink rejection and
//!   `RESOLVE_BENEATH` are deferred to a Milestone-2 `-sys` crate (this adapter
//!   stays free of `unsafe` and `libc`).
//! - The directory/owner check enforces the world-writable bit only; owner
//!   equality needs the process euid (no `std` API) and is deferred (see
//!   `trust_store`).
//!
//! References: ADR-0064 §"profile loading", ADR-0035 §"safe-open",
//! ADR-0033 §"atomic writes".

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use substrate_domain::launch::errors::LaunchError;
use substrate_domain::launch::profile::{LaunchOperatorConfig, LaunchProfile};
use substrate_domain::launch::trust::TrustRecord;

use crate::trust_store::{append_bless, auto_bless_allows, load_trust_store};

/// `O_NOFOLLOW` flag value for the open-without-symlink-follow step.
///
/// Defined per-target so the adapter needs neither `libc` nor `unsafe`. A `0`
/// fallback on other platforms degrades gracefully to the `lstat` leaf check.
#[cfg(target_os = "linux")]
const O_NOFOLLOW: i32 = 0o400_000;
#[cfg(target_os = "macos")]
const O_NOFOLLOW: i32 = 0x0100;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const O_NOFOLLOW: i32 = 0;

/// World-write permission bit; its presence on the parent directory is rejected.
const WORLD_WRITE_BIT: u32 = 0o002;
/// Mask isolating the permission bits stored in a [`TrustRecord`] (`0..=4095`).
const PERMISSION_MASK: u32 = 0o7777;

/// A Profile parsed from disk together with its content hash and identity tuple.
#[derive(Debug, Clone)]
pub struct LoadedProfile {
    /// The deserialized Profile value object.
    pub profile: LaunchProfile,
    /// Prefixed content hash `blake3:<hex>` over the exact bytes read.
    pub config_hash: String,
    /// Identity tuple `(dev, ino, uid, mode)` captured by `fstat` on the open fd.
    /// `mode` is masked to `0o7777`.
    pub identity: (u64, u64, u32, u32),
}

/// Result of the safe-open step: the file bytes plus the descriptor's identity.
struct SafeOpen {
    bytes: Vec<u8>,
    dev: u64,
    ino: u64,
    uid: u32,
    mode: u32,
}

/// Loads a Profile through the full TOFU gate, returning it only when trusted.
///
/// Runs safe-open → fstat → BLAKE3 → trust lookup → deserialize. A tuple absent
/// from the trust store is rejected unless `op_cfg` auto-blesses the Profile's
/// canonical path, in which case the new bless record is written to
/// `trust_store` and the load proceeds.
///
/// # Errors
///
/// - [`LaunchError::ConfigSymlinkRejected`] — the config is a symlink.
/// - [`LaunchError::ConfigUntrustedDir`] — the parent directory is world-writable.
/// - [`LaunchError::TrustStoreInsecure`] — the trust store has loose permissions.
/// - [`LaunchError::ProfileNotTrusted`] — no matching bless record and no auto-bless.
/// - [`LaunchError::InvalidProfile`] — the bytes are not valid Profile TOML.
pub async fn load_trusted(
    profile_path: &Path,
    trust_store: &Path,
    op_cfg: &LaunchOperatorConfig,
) -> Result<LoadedProfile, LaunchError> {
    let opened = safe_open(profile_path).await?;
    let config_hash = hash_bytes(&opened.bytes);
    let mode = opened.mode & PERMISSION_MASK;
    let canonical = canonicalize(profile_path)?;
    let canonical_str = canonical.display().to_string();

    let records = load_trust_store(trust_store).await?;
    let trusted = records.iter().any(|r| {
        r.path == canonical_str && r.matches(opened.dev, opened.ino, opened.uid, mode, &config_hash)
    });

    if !trusted {
        if auto_bless_allows(op_cfg, &canonical) {
            let record = build_trust_record(
                &canonical_str,
                (opened.dev, opened.ino, opened.uid, mode),
                &config_hash,
            );
            append_bless(trust_store, record).await?;
        } else {
            return Err(LaunchError::ProfileNotTrusted { path: canonical_str });
        }
    }

    let profile = deserialize(&opened.bytes)?;
    Ok(LoadedProfile {
        profile,
        config_hash,
        identity: (opened.dev, opened.ino, opened.uid, mode),
    })
}

/// Loads a Profile read-only with **no** trust verdict, for `launch.list`.
///
/// Performs the same safe-open and parse as [`load_trusted`] but never consults
/// the trust store, so an unblessed Profile can still be enumerated
/// (`launch-list-no-trust-required`).
///
/// # Errors
///
/// - [`LaunchError::ConfigSymlinkRejected`] — the config is a symlink.
/// - [`LaunchError::ConfigUntrustedDir`] — the parent directory is world-writable.
/// - [`LaunchError::InvalidProfile`] — the bytes are not valid Profile TOML.
pub async fn load_untrusted(profile_path: &Path) -> Result<LoadedProfile, LaunchError> {
    let opened = safe_open(profile_path).await?;
    let config_hash = hash_bytes(&opened.bytes);
    let profile = deserialize(&opened.bytes)?;
    Ok(LoadedProfile {
        profile,
        config_hash,
        identity: (
            opened.dev,
            opened.ino,
            opened.uid,
            opened.mode & PERMISSION_MASK,
        ),
    })
}

/// Builds a [`TrustRecord`] from a canonical path, identity tuple, and content hash.
///
/// Used by the auto-bless path and by `launch.trust` (Phase 4) to mint a fresh
/// bless record stamped with the current RFC 3339 time.
#[must_use]
pub fn build_trust_record(
    canonical_path: &str,
    identity: (u64, u64, u32, u32),
    content_hash: &str,
) -> TrustRecord {
    let (dev, ino, uid, mode) = identity;
    TrustRecord {
        path: canonical_path.to_owned(),
        dev,
        ino,
        uid,
        mode,
        content: content_hash.to_owned(),
        blessed_at: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned()),
    }
}

/// Writes a default `.substrate.toml` scaffold for `launch.init`.
///
/// Picks a starter command from `hint` (for example `rust` or `node`) and writes
/// the file atomically. Refuses to clobber an existing Profile.
///
/// # Errors
///
/// Returns [`LaunchError::InvalidProfile`] when the target already exists or the
/// write fails.
pub async fn write_scaffold(
    profile_path: &Path,
    hint: Option<&str>,
) -> Result<PathBuf, LaunchError> {
    if tokio::fs::try_exists(profile_path).await.unwrap_or(false) {
        return Err(LaunchError::InvalidProfile {
            msg: format!("{} already exists; refusing to overwrite", profile_path.display()),
        });
    }
    let body = scaffold_body(hint);
    let parent = profile_path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(".substrate.toml.tmp.{}", Uuid::now_v7().simple()));
    tokio::fs::write(&tmp, body.as_bytes())
        .await
        .map_err(|e| LaunchError::InvalidProfile {
            msg: format!("failed to write scaffold: {e}"),
        })?;
    if tokio::fs::rename(&tmp, profile_path).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(LaunchError::InvalidProfile {
            msg: format!("failed to commit scaffold to {}", profile_path.display()),
        });
    }
    Ok(profile_path.to_path_buf())
}

/// Runs the symlink-safe open + parent-dir guard, returning bytes and identity.
///
/// Order matters: the parent-directory world-writable check and the `lstat` leaf
/// symlink check both run before the file is opened so neither a hash nor a spawn
/// can happen on an untrusted path.
async fn safe_open(profile_path: &Path) -> Result<SafeOpen, LaunchError> {
    reject_world_writable_parent(profile_path).await?;
    reject_symlink_leaf(profile_path).await?;

    let file = open_nofollow(profile_path).await?;
    let meta = file.metadata().await.map_err(|_| LaunchError::ConfigSymlinkRejected {
        path: profile_path.display().to_string(),
    })?;
    let (dev, ino, uid, mode) = (meta.dev(), meta.ino(), meta.uid(), meta.mode());

    let bytes = read_all(file, profile_path).await?;
    Ok(SafeOpen { bytes, dev, ino, uid, mode })
}

/// Rejects a Profile whose parent directory has the world-write bit set.
async fn reject_world_writable_parent(profile_path: &Path) -> Result<(), LaunchError> {
    let Some(parent) = profile_path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    let meta = tokio::fs::metadata(parent).await.map_err(|_| LaunchError::ConfigUntrustedDir {
        path: parent.display().to_string(),
    })?;
    if meta.mode() & WORLD_WRITE_BIT != 0 {
        return Err(LaunchError::ConfigUntrustedDir {
            path: parent.display().to_string(),
        });
    }
    Ok(())
}

/// Deterministically rejects a symlinked config leaf via `lstat`.
async fn reject_symlink_leaf(profile_path: &Path) -> Result<(), LaunchError> {
    let meta = tokio::fs::symlink_metadata(profile_path).await.map_err(|_| {
        LaunchError::ConfigSymlinkRejected {
            path: profile_path.display().to_string(),
        }
    })?;
    if meta.file_type().is_symlink() {
        return Err(LaunchError::ConfigSymlinkRejected {
            path: profile_path.display().to_string(),
        });
    }
    Ok(())
}

/// Opens `profile_path` read-only with `O_NOFOLLOW`, mapping a follow-failure to
/// [`LaunchError::ConfigSymlinkRejected`].
async fn open_nofollow(profile_path: &Path) -> Result<tokio::fs::File, LaunchError> {
    tokio::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(profile_path)
        .await
        .map_err(|_| LaunchError::ConfigSymlinkRejected {
            path: profile_path.display().to_string(),
        })
}

/// Reads every byte from the already-open descriptor (never reopening the path).
async fn read_all(mut file: tokio::fs::File, profile_path: &Path) -> Result<Vec<u8>, LaunchError> {
    use tokio::io::AsyncReadExt as _;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).await.map_err(|e| LaunchError::InvalidProfile {
        msg: format!("failed to read {}: {e}", profile_path.display()),
    })?;
    Ok(bytes)
}

/// Returns the canonical absolute path, mapping failure to an untrusted-dir error.
fn canonicalize(profile_path: &Path) -> Result<PathBuf, LaunchError> {
    std::fs::canonicalize(profile_path).map_err(|_| LaunchError::ConfigUntrustedDir {
        path: profile_path.display().to_string(),
    })
}

/// Returns the prefixed BLAKE3 content hash `blake3:<hex>` over `bytes`.
fn hash_bytes(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// Deserializes Profile TOML from in-memory bytes (the file is never reopened).
fn deserialize(bytes: &[u8]) -> Result<LaunchProfile, LaunchError> {
    let text = std::str::from_utf8(bytes).map_err(|_| LaunchError::InvalidProfile {
        msg: "profile is not valid UTF-8".to_owned(),
    })?;
    toml::from_str::<LaunchProfile>(text).map_err(|e| LaunchError::InvalidProfile {
        msg: format!("profile is not valid TOML: {e}"),
    })
}

/// Renders the scaffold TOML body for a project-type `hint`.
fn scaffold_body(hint: Option<&str>) -> String {
    let command = match hint {
        Some("rust") => r#"["cargo", "run"]"#,
        Some("node") => r#"["pnpm", "dev"]"#,
        Some("python") => r#"["python", "-m", "app"]"#,
        _ => r#"["echo", "configure your service command"]"#,
    };
    format!(
        "version = 1\n\n[services.app]\ncommand = {command}\nrequired = true\n"
    )
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
    use std::os::unix::fs::PermissionsExt as _;

    use tempfile::TempDir;

    use super::*;

    const VALID_PROFILE: &str = "version = 1\n\n[services.web]\ncommand = [\"echo\", \"hi\"]\n";

    /// Writes a Profile into a freshly created secure tempdir and returns
    /// (`dir`, `profile_path`, `trust_store_path`).
    async fn fixture(body: &str) -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let profile = dir.path().join(".substrate.toml");
        tokio::fs::write(&profile, body.as_bytes()).await.expect("write profile");
        let store = dir.path().join("launch-trust.toml");
        (dir, profile, store)
    }

    #[tokio::test]
    async fn symlinked_config_is_rejected() {
        // launch-profile-symlink-rejected
        let dir = TempDir::new().expect("tempdir");
        let real = dir.path().join("real.toml");
        tokio::fs::write(&real, VALID_PROFILE.as_bytes()).await.expect("write real");
        let link = dir.path().join(".substrate.toml");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let store = dir.path().join("launch-trust.toml");
        let cfg = LaunchOperatorConfig::default();
        let err = load_trusted(&link, &store, &cfg).await.expect_err("symlink rejected");
        assert!(matches!(err, LaunchError::ConfigSymlinkRejected { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn world_writable_parent_is_rejected() {
        // launch-config-untrusted-dir
        let (dir, profile, store) = fixture(VALID_PROFILE).await;
        tokio::fs::set_permissions(dir.path(), Permissions::from_mode(0o777))
            .await
            .expect("chmod dir world-writable");
        let cfg = LaunchOperatorConfig::default();
        let err = load_trusted(&profile, &store, &cfg).await.expect_err("untrusted dir");
        assert!(matches!(err, LaunchError::ConfigUntrustedDir { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn unblessed_profile_is_not_trusted() {
        // launch-profile-untrusted-rejected
        let (_dir, profile, store) = fixture(VALID_PROFILE).await;
        let cfg = LaunchOperatorConfig::default();
        let err = load_trusted(&profile, &store, &cfg).await.expect_err("not trusted");
        assert!(matches!(err, LaunchError::ProfileNotTrusted { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn cloned_local_toml_is_not_trusted() {
        // launch-local-toml-not-trusted-on-clone: a committed .substrate.local.toml
        // still requires the same TOFU bless on first load.
        let dir = TempDir::new().expect("tempdir");
        let local = dir.path().join(".substrate.local.toml");
        tokio::fs::write(&local, VALID_PROFILE.as_bytes()).await.expect("write local");
        let store = dir.path().join("launch-trust.toml");
        let cfg = LaunchOperatorConfig::default();
        let err = load_trusted(&local, &store, &cfg).await.expect_err("local not trusted");
        assert!(matches!(err, LaunchError::ProfileNotTrusted { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn hostile_inline_auto_bless_field_is_ignored() {
        // launch-hostile-auto-bless-field-rejected: a repo-controlled auto_bless
        // key cannot self-authorize; it is an unknown field and stays untrusted.
        let body = format!("auto_bless = true\n{VALID_PROFILE}");
        let (_dir, profile, store) = fixture(&body).await;
        let cfg = LaunchOperatorConfig::default();
        let err = load_trusted(&profile, &store, &cfg).await.expect_err("hostile auto_bless");
        assert!(matches!(err, LaunchError::ProfileNotTrusted { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn operator_scope_auto_bless_proceeds_and_writes_record() {
        // launch-auto-bless-operator-scope
        let (dir, profile, store) = fixture(VALID_PROFILE).await;
        let canonical = std::fs::canonicalize(&profile).expect("canonicalize");
        let cfg = LaunchOperatorConfig {
            auto_bless_paths: vec![canonical.display().to_string()],
        };
        let loaded = load_trusted(&profile, &store, &cfg).await.expect("auto-bless proceeds");
        assert!(loaded.profile.services.contains_key("web"));
        // The bless record is now persisted in the user-scope trust store.
        let records = load_trust_store(&store).await.expect("load store");
        assert_eq!(records.len(), 1, "auto-bless must write a record");
        assert_eq!(records[0].path, canonical.display().to_string());
        drop(dir);
    }

    #[tokio::test]
    async fn blessed_profile_loads_then_edit_invalidates_trust() {
        // launch-trust-invalidated-on-edit: bless via auto-bless, confirm trusted,
        // then edit content -> hash mismatch -> ProfileNotTrusted.
        let (_dir, profile, store) = fixture(VALID_PROFILE).await;
        let canonical = std::fs::canonicalize(&profile).expect("canonicalize");
        let cfg = LaunchOperatorConfig {
            auto_bless_paths: vec![canonical.display().to_string()],
        };
        // First load auto-blesses and pins the current content.
        load_trusted(&profile, &store, &cfg).await.expect("initial bless");

        // A second load with an empty op-config must still be trusted (record matches).
        let empty_cfg = LaunchOperatorConfig::default();
        load_trusted(&profile, &store, &empty_cfg).await.expect("still trusted after bless");

        // Editing the content changes the BLAKE3 hash -> tuple no longer matches.
        tokio::fs::write(&profile, b"version = 1\n\n[services.web]\ncommand = [\"echo\", \"EDITED\"]\n")
            .await
            .expect("edit profile");
        let err = load_trusted(&profile, &store, &empty_cfg)
            .await
            .expect_err("edited profile is untrusted");
        assert!(matches!(err, LaunchError::ProfileNotTrusted { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn list_loads_without_trust_gate() {
        // launch-list-no-trust-required
        let profile_body = "version = 1\n\n[services.db]\ncommand = [\"dbd\"]\n\n[services.api]\ncommand = [\"apid\"]\ndepends_on = [\"db\"]\n\n[services.web]\ncommand = [\"webd\"]\ndepends_on = [\"api\"]\n";
        let (_dir, profile, _store) = fixture(profile_body).await;
        let loaded = load_untrusted(&profile).await.expect("list loads read-only");
        assert!(loaded.profile.services.contains_key("db"));
        assert!(loaded.profile.services.contains_key("api"));
        assert!(loaded.profile.services.contains_key("web"));
    }

    #[tokio::test]
    async fn invalid_toml_is_invalid_profile() {
        let (_dir, profile, _store) = fixture("this is = = not toml").await;
        let err = load_untrusted(&profile).await.expect_err("invalid toml");
        assert!(matches!(err, LaunchError::InvalidProfile { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn write_scaffold_creates_then_refuses_overwrite() {
        let dir = TempDir::new().expect("tempdir");
        let profile = dir.path().join(".substrate.toml");
        let written = write_scaffold(&profile, Some("rust")).await.expect("scaffold");
        assert_eq!(written, profile);
        let text = tokio::fs::read_to_string(&profile).await.expect("read back");
        assert!(text.contains("cargo"), "rust hint seeds a cargo command");
        // A second call must not clobber.
        let err = write_scaffold(&profile, None).await.expect_err("no overwrite");
        assert!(matches!(err, LaunchError::InvalidProfile { .. }));
    }
}
