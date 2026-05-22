//! Transactional temp-file path management per ADR-0033.
//!
//! Re-implements the same pattern as `substrate-fs-mutation::tmp_path` for the
//! archive crate so it has no sibling-crate dependency. Every disk-write
//! handler in this crate MUST route through [`TmpPath`]:
//!
//! 1. Call [`TmpPath::new_for`] to obtain `<target_parent>/<uuid7>.tmp`.
//! 2. Write to [`TmpPath::tmp_path`].
//! 3. Call [`TmpPath::commit`] for an atomic rename to the final path.
//!
//! Dropping without `commit` removes the temp file on a best-effort basis.

use std::path::{Path, PathBuf};

use uuid::Uuid;

// ---- Crockford base-32 -------------------------------------------------------

/// Encodes 16 raw bytes to a 26-character Crockford base-32 string.
///
/// Alphabet: `0123456789ABCDEFGHJKMNPQRSTVWXYZ` (Douglas Crockford).
#[must_use]
pub fn crockford_base32(bytes: &[u8; 16]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut out = [0u8; 26];
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
    // Every byte in `out` is from the ASCII subset of ALPHABET.
    String::from_utf8(out.to_vec())
        .unwrap_or_else(|_| unreachable!("crockford alphabet is all ASCII"))
}

// ---- TmpPath -----------------------------------------------------------------

/// RAII guard for a transactional temp file per ADR-0033.
///
/// On [`drop`](Drop::drop), if [`commit`](TmpPath::commit) was never called,
/// the temp file is removed on a best-effort basis (ignores OS errors).
#[derive(Debug)]
#[expect(
    clippy::struct_field_names,
    reason = "tmp_path and final_path are the canonical names for this RAII guard; renaming would reduce clarity"
)]
pub struct TmpPath {
    final_path: PathBuf,
    tmp_path: PathBuf,
    committed: bool,
}

impl TmpPath {
    /// Creates a new [`TmpPath`] for `target`.
    ///
    /// The temp path is `<target_parent>/<crockford_uuid7>.tmp`.
    #[must_use]
    pub fn new_for(target: &Path) -> Self {
        let uuid = Uuid::now_v7();
        let suffix = crockford_base32(uuid.as_bytes());
        let file_name = format!("{suffix}.tmp");
        let parent = target.parent().unwrap_or_else(|| Path::new("."));
        let tmp_path = parent.join(&file_name);
        Self {
            final_path: target.to_path_buf(),
            tmp_path,
            committed: false,
        }
    }

    /// The working temp path to write to.
    #[must_use]
    pub fn tmp_path(&self) -> &Path {
        &self.tmp_path
    }

    /// The intended final path.
    #[must_use]
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    /// Atomically renames the temp file to the final target path.
    ///
    /// # Errors
    ///
    /// Propagates any `std::io::Error` from `tokio::fs::rename`.
    pub async fn commit(mut self) -> std::io::Result<()> {
        tokio::fs::rename(&self.tmp_path, &self.final_path).await?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for TmpPath {
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

    #[tokio::test]
    async fn commit_renames_file() {
        let dir = TempDir::new().expect("tempdir");
        let final_path = dir.path().join("output.tar");
        let tmp = TmpPath::new_for(&final_path);
        tokio::fs::write(tmp.tmp_path(), b"fake-tar")
            .await
            .expect("write");
        let tmp_path_clone = tmp.tmp_path().to_path_buf();
        tmp.commit().await.expect("commit");
        assert!(final_path.exists());
        assert!(!tmp_path_clone.exists());
    }

    #[tokio::test]
    async fn drop_without_commit_cleans_up() {
        let dir = TempDir::new().expect("tempdir");
        let final_path = dir.path().join("output.tar");
        let tmp = TmpPath::new_for(&final_path);
        tokio::fs::write(tmp.tmp_path(), b"data")
            .await
            .expect("write");
        let tmp_path_clone = tmp.tmp_path().to_path_buf();
        drop(tmp);
        assert!(!tmp_path_clone.exists());
        assert!(!final_path.exists());
    }

    #[test]
    fn distinct_tmp_paths_for_same_target() {
        let path = Path::new("/tmp/archive.tar");
        let a = TmpPath::new_for(path);
        let b = TmpPath::new_for(path);
        assert_ne!(a.tmp_path(), b.tmp_path());
    }
}
