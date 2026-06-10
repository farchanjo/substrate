//! Transactional temp-file path management per ADR-0033.
//!
//! Every disk-write tool that creates or overwrites a file MUST route through
//! [`TmpPath`]:
//!
//! 1. Call [`TmpPath::new_for`] to obtain a sibling temp path
//!    `<target_parent>/<original_filename>.tmp.<crockford_uuid7>`.
//! 2. Write to [`TmpPath::tmp_path`].
//! 3. Call [`TmpPath::commit`] to atomically rename tmp → target.
//!
//! If the handler is cancelled, panics, or returns an error before `commit`,
//! the [`Drop`] impl removes the temp file on a best-effort basis.

use std::path::{Path, PathBuf};

use uuid::Uuid;

// ---- Crockford base32 encoding -----------------------------------------------

/// Encodes 16 raw bytes to a 26-character Crockford base-32 string.
///
/// Used to produce human-readable, filesystem-safe, sortable `UUIDv7` suffixes
/// for transactional temp files.
///
/// # Algorithm
///
/// Crockford base-32 uses alphabet `0123456789ABCDEFGHJKMNPQRSTVWXYZ`.
/// This function packs 5-bit groups from the most-significant side.
/// The 16-byte (128-bit) input maps to exactly ⌈128/5⌉ = 26 characters.
#[must_use]
pub fn crockford_base32(bytes: &[u8; 16]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut out = [0u8; 26];
    // Pack 128 bits into 26 × 5-bit groups (only 130 bits needed; top 2 are 0).
    let mut acc: u64 = 0;
    let mut bits: u32 = 0;
    let mut idx = 25usize;
    for &byte in bytes.iter().rev() {
        acc |= u64::from(byte) << bits;
        bits += 8;
        while bits >= 5 {
            out[idx] = ALPHABET[(acc & 0x1f) as usize];
            acc >>= 5;
            bits -= 5;
            idx = idx.saturating_sub(1);
        }
    }
    if bits > 0 {
        out[idx] = ALPHABET[(acc & 0x1f) as usize];
    }
    // SAFETY (UTF-8): every byte in `out` is a valid ASCII character from ALPHABET.
    String::from_utf8(out.to_vec())
        .unwrap_or_else(|_| unreachable!("crockford alphabet is all ASCII"))
}

// ---- TmpPath -----------------------------------------------------------------

/// RAII guard for a transactional temp file per ADR-0033.
///
/// Constructs a sibling path
/// `<target_dir>/<original_filename>.tmp.<crockford_uuid7>` next to the
/// final target. On drop, removes the temp file if [`commit`](TmpPath::commit)
/// was not called (best-effort; ignores errors).
#[derive(Debug)]
#[expect(
    clippy::struct_field_names,
    reason = "tmp_path field name is intentionally self-describing in the TmpPath RAII guard context"
)]
pub struct TmpPath {
    /// The intended final path.
    final_path: PathBuf,
    /// The working temp path (`<parent>/<original_filename>.tmp.<uuid7>`).
    tmp_path: PathBuf,
    /// Set to `true` after a successful [`commit`](TmpPath::commit).
    committed: bool,
}

impl TmpPath {
    /// Creates a new [`TmpPath`] for `target`.
    ///
    /// The temp path is `<target_parent>/<original_filename>.tmp.<crockford_uuid7>`.
    /// Preserving the original filename as a prefix satisfies ADR-0033 Step 2
    /// and makes temp files immediately identifiable by their origin.
    /// The UUID is generated at call time with [`Uuid::now_v7`].
    #[must_use]
    pub fn new_for(target: &Path) -> Self {
        let uuid = Uuid::now_v7();
        let suffix = crockford_base32(uuid.as_bytes());
        let base = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_owned());
        let file_name = format!("{base}.tmp.{suffix}");

        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        let tmp_path = parent.join(&file_name);

        Self {
            final_path: target.to_path_buf(),
            tmp_path,
            committed: false,
        }
    }

    /// Returns the working temp path to write to.
    #[must_use]
    pub fn tmp_path(&self) -> &Path {
        &self.tmp_path
    }

    /// Returns the intended final path.
    #[must_use]
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    /// Atomically renames the temp file to the final target path.
    ///
    /// After this returns `Ok(())`, the temp file no longer exists and the final
    /// path contains the new content. The [`Drop`] impl will not attempt cleanup.
    ///
    /// # Errors
    ///
    /// Propagates any [`std::io::Error`] from [`tokio::fs::rename`].
    pub async fn commit(mut self) -> std::io::Result<()> {
        tokio::fs::rename(&self.tmp_path, &self.final_path).await?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for TmpPath {
    /// Best-effort cleanup: removes the temp file if [`commit`](TmpPath::commit)
    /// was never called (cancelled or error path).
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.tmp_path);
        }
    }
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
    use tempfile::TempDir;

    /// `TmpPath::commit` moves the file; temp path is gone, final path exists.
    #[tokio::test]
    async fn commit_renames_file() {
        let dir = TempDir::new().expect("tempdir");
        let final_path = dir.path().join("output.txt");
        let tmp = TmpPath::new_for(&final_path);
        tokio::fs::write(tmp.tmp_path(), b"hello")
            .await
            .expect("write");
        assert!(tmp.tmp_path().exists(), "tmp must exist before commit");
        let tmp_path_clone = tmp.tmp_path().to_path_buf();
        tmp.commit().await.expect("commit");
        assert!(final_path.exists(), "final path must exist after commit");
        assert!(
            !tmp_path_clone.exists(),
            "tmp path must be gone after commit"
        );
    }

    /// Dropping without commit removes the temp file (cleanup-on-cancel).
    #[tokio::test]
    async fn drop_without_commit_cleans_up() {
        let dir = TempDir::new().expect("tempdir");
        let final_path = dir.path().join("output.txt");
        let tmp = TmpPath::new_for(&final_path);
        tokio::fs::write(tmp.tmp_path(), b"hello")
            .await
            .expect("write");
        let tmp_path_clone = tmp.tmp_path().to_path_buf();
        drop(tmp); // no commit
        assert!(
            !tmp_path_clone.exists(),
            "tmp path must be cleaned up after drop without commit"
        );
        assert!(!final_path.exists(), "final path must not exist");
    }

    /// Two `TmpPath`s for the same target produce distinct temp paths.
    #[test]
    fn distinct_tmp_paths() {
        let path = Path::new("/tmp/foo.txt");
        let a = TmpPath::new_for(path);
        let b = TmpPath::new_for(path);
        assert_ne!(a.tmp_path(), b.tmp_path(), "UUIDs must differ");
    }

    /// Crockford base32 output is always 26 ASCII characters.
    #[test]
    fn crockford_len() {
        let bytes = [0xFFu8; 16];
        let s = crockford_base32(&bytes);
        assert_eq!(s.len(), 26);
        assert!(s.is_ascii());
    }
}
