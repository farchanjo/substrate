//! TOFU trust-store I/O for the launch BC (ADR-0064).
//!
//! Reads, permission-verifies, and atomically appends to the user-scope trust
//! store (`~/.config/substrate/launch-trust.toml`). Each entry is a
//! [`TrustRecord`] binding a canonical Profile path to its inode/content
//! identity tuple. The store MUST be mode `0600` (no group/other bits); a looser
//! mode is rejected with [`LaunchError::TrustStoreInsecure`] before any bless
//! lookup proceeds.
//!
//! # Security deviation (MVP)
//!
//! The owner-equality half of the "0600 + owner" rule needs the process effective
//! uid, for which `std` exposes no API; obtaining it would require `libc`, which
//! this adapter crate deliberately avoids (raw syscalls live in `-sys` crates per
//! ADR-0042). The MVP therefore enforces the permission-bit half — no
//! group/other access — which is what the `launch-trust-store-insecure-permissions`
//! feature exercises. Owner-equality enforcement is deferred to Milestone 2.
//!
//! References: ADR-0064 §"trust store format", ADR-0033 §"atomic writes".

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use substrate_domain::launch::errors::LaunchError;
use substrate_domain::launch::profile::LaunchOperatorConfig;
use substrate_domain::launch::trust::TrustRecord;

/// Required mode for the trust store: no group or other permission bits.
const SECURE_MODE: u32 = 0o600;
/// Mask isolating group + other permission bits from `st_mode`.
const GROUP_OTHER_MASK: u32 = 0o077;
/// Mode applied to a freshly created trust-store directory.
const SECURE_DIR_MODE: u32 = 0o700;

/// On-disk TOML envelope for the trust store: an array of `[[record]]` tables.
#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustStoreFile {
    /// The blessed records; absent in a brand-new store.
    #[serde(default)]
    record: Vec<TrustRecord>,
}

/// Loads and permission-verifies the trust store at `path`.
///
/// A non-existent store is not an error: it yields an empty record set (nothing
/// has been blessed yet). An existing store with any group/other permission bit
/// set is rejected before parsing.
///
/// # Errors
///
/// - [`LaunchError::TrustStoreInsecure`] when the store's mode permits group or
///   other access, or when its metadata cannot be read.
/// - [`LaunchError::InvalidProfile`] when the store file is not valid TOML.
pub async fn load_trust_store(path: &Path) -> Result<Vec<TrustRecord>, LaunchError> {
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => {
            return Err(LaunchError::TrustStoreInsecure {
                path: path.display().to_string(),
            });
        },
    };

    if meta.permissions().mode() & GROUP_OTHER_MASK != 0 {
        return Err(LaunchError::TrustStoreInsecure {
            path: path.display().to_string(),
        });
    }

    let bytes = tokio::fs::read(path).await.map_err(|_| LaunchError::TrustStoreInsecure {
        path: path.display().to_string(),
    })?;
    let text = String::from_utf8(bytes).map_err(|_| LaunchError::InvalidProfile {
        msg: format!("trust store {} is not valid UTF-8", path.display()),
    })?;
    let parsed: TrustStoreFile = toml::from_str(&text).map_err(|e| LaunchError::InvalidProfile {
        msg: format!("trust store {} is not valid TOML: {e}", path.display()),
    })?;
    Ok(parsed.record)
}

/// Appends `rec` to the trust store at `path`, writing securely and atomically.
///
/// Existing records are preserved. The new file is written to a sibling temp
/// path, chmod-ed to `0600`, then atomically renamed over `path` (ADR-0033). The
/// parent directory is created `0700` if absent.
///
/// # Errors
///
/// Returns [`LaunchError::TrustStoreInsecure`] when any filesystem operation on
/// the store path fails, or [`LaunchError::InvalidProfile`] when the existing
/// store cannot be parsed.
pub async fn append_bless(path: &Path, rec: TrustRecord) -> Result<(), LaunchError> {
    let insecure = || LaunchError::TrustStoreInsecure {
        path: path.display().to_string(),
    };

    let mut records = load_existing_lax(path).await?;
    records.push(rec);
    let envelope = TrustStoreFile { record: records };
    let text = toml::to_string_pretty(&envelope).map_err(|e| LaunchError::InvalidProfile {
        msg: format!("failed to serialize trust store: {e}"),
    })?;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent).await.map_err(|_| insecure())?;
    tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(SECURE_DIR_MODE))
        .await
        .map_err(|_| insecure())?;

    let tmp = tmp_sibling(path);
    tokio::fs::write(&tmp, text.as_bytes()).await.map_err(|_| insecure())?;
    if tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(SECURE_MODE))
        .await
        .is_err()
    {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(insecure());
    }
    if tokio::fs::rename(&tmp, path).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(insecure());
    }
    Ok(())
}

/// Returns `true` when `canonical` falls under an operator-blessed auto-bless prefix.
///
/// The prefixes come from the user-scope `launch.toml` (`auto_bless_paths`), never
/// from a repository-controlled Profile, so a cloned Profile cannot authorize its
/// own blessing (`launch-hostile-auto-bless-field-rejected`). A prefix matches
/// when it equals `canonical` or is one of its ancestor directories.
#[must_use]
pub fn auto_bless_allows(cfg: &LaunchOperatorConfig, canonical: &Path) -> bool {
    cfg.auto_bless_paths
        .iter()
        .filter(|p| !p.is_empty())
        .any(|prefix| canonical.starts_with(Path::new(prefix)))
}

/// Reads existing records tolerantly for the append path (missing store is empty).
async fn load_existing_lax(path: &Path) -> Result<Vec<TrustRecord>, LaunchError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let text = String::from_utf8(bytes).map_err(|_| LaunchError::InvalidProfile {
                msg: format!("trust store {} is not valid UTF-8", path.display()),
            })?;
            let parsed: TrustStoreFile =
                toml::from_str(&text).map_err(|e| LaunchError::InvalidProfile {
                    msg: format!("trust store {} is not valid TOML: {e}", path.display()),
                })?;
            Ok(parsed.record)
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err(LaunchError::TrustStoreInsecure {
            path: path.display().to_string(),
        }),
    }
}

/// Builds a sibling temp path `<dir>/.<name>.tmp.<uuid7>` next to `path`.
fn tmp_sibling(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .map_or_else(|| "store".to_owned(), |n| n.to_string_lossy().into_owned());
    parent.join(format!(".{base}.tmp.{}", Uuid::now_v7().simple()))
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

    fn record(path: &str) -> TrustRecord {
        TrustRecord {
            path: path.to_owned(),
            dev: 66,
            ino: 1234,
            uid: 1000,
            mode: 0o644,
            content: "blake3:abc123".to_owned(),
            blessed_at: "2026-06-30T12:00:00Z".to_owned(),
        }
    }

    #[tokio::test]
    async fn missing_store_loads_empty() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("launch-trust.toml");
        let records = load_trust_store(&path).await.expect("missing store is empty");
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn mode_0644_store_is_rejected() {
        // launch-trust-store-insecure-permissions: a mode-0644 store is rejected
        // before any bless lookup.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("launch-trust.toml");
        tokio::fs::write(&path, b"").await.expect("write store");
        tokio::fs::set_permissions(&path, Permissions::from_mode(0o644))
            .await
            .expect("chmod");
        let err = load_trust_store(&path).await.expect_err("0644 must be rejected");
        assert!(matches!(err, LaunchError::TrustStoreInsecure { .. }));
    }

    #[tokio::test]
    async fn append_then_load_round_trips_and_is_0600() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("launch-trust.toml");
        append_bless(&path, record("/repo/.substrate.toml"))
            .await
            .expect("append");

        let mode = tokio::fs::metadata(&path).await.expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, SECURE_MODE, "appended store must be 0600");

        let records = load_trust_store(&path).await.expect("load");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].path, "/repo/.substrate.toml");
    }

    #[tokio::test]
    async fn append_preserves_existing_records() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("launch-trust.toml");
        append_bless(&path, record("/a/.substrate.toml")).await.expect("first");
        append_bless(&path, record("/b/.substrate.toml")).await.expect("second");
        let records = load_trust_store(&path).await.expect("load");
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn auto_bless_matches_prefix() {
        let cfg = LaunchOperatorConfig {
            auto_bless_paths: vec!["/home/dev/projects".to_owned()],
        };
        assert!(auto_bless_allows(
            &cfg,
            Path::new("/home/dev/projects/app/.substrate.toml")
        ));
    }

    #[test]
    fn auto_bless_rejects_unlisted_path() {
        let cfg = LaunchOperatorConfig {
            auto_bless_paths: vec!["/home/dev/projects".to_owned()],
        };
        assert!(!auto_bless_allows(&cfg, Path::new("/tmp/evil/.substrate.toml")));
    }

    #[test]
    fn empty_auto_bless_list_allows_nothing() {
        let cfg = LaunchOperatorConfig::default();
        assert!(!auto_bless_allows(&cfg, Path::new("/anything/.substrate.toml")));
    }
}
