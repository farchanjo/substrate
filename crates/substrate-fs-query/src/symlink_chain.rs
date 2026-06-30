//! Shared symlink-chain disposition walker (ADR-0035).
//!
//! On macOS, `ONoFollowAnyJail` (`O_NOFOLLOW_ANY`) returns `SymlinkEscape` for
//! ANY path that traverses a symlink component, regardless of where that
//! symlink ultimately resolves — including a symlink whose target sits
//! squarely inside an allowlisted root (e.g. `/tmp` -> `/private/tmp`, or an
//! operator's own dotfile symlinks). A bare `SymlinkEscape` therefore does not
//! by itself distinguish a benign internal symlink from a genuine escape or a
//! dangling link; callers that need to keep serving the request for the
//! benign case probe further with [`symlink_chain_disposition`].

use std::path::{Path, PathBuf};

use substrate_domain::{JailedPath, PathJailPort, SubstrateError};

/// Maximum symlink hops before treating the chain as an escape (loop guard).
#[expect(
    clippy::redundant_pub_crate,
    reason = "clippy::redundant_pub_crate and clippy::unreachable_pub directly \
              contradict each other for a pub(crate) item inside a private \
              module (each lint's suggested fix is what the other flags); \
              pub(crate) is the semantically correct choice — this is shared \
              across find.rs and stat.rs but intentionally not part of this \
              crate's public API. See substrate-policy/nfc.rs for the same \
              precedent."
)]
pub(crate) const MAX_SYMLINK_HOPS: u8 = 40;

/// Disposition for a path when the jail returns `SymlinkEscape`.
#[expect(
    clippy::redundant_pub_crate,
    reason = "see MAX_SYMLINK_HOPS above — clippy::redundant_pub_crate and \
              clippy::unreachable_pub contradict each other for a pub(crate) \
              item inside a private module"
)]
pub(crate) enum SymlinkDisposition {
    /// Symlink exists but target is missing (dangling symlink).
    Broken,
    /// Symlink exists and its canonical target is within the allowlist.
    Internal {
        /// `lstat` of the original (outermost) symlink node.
        lstat: std::fs::Metadata,
        /// Fully-resolved, symlink-free canonical target path.
        resolved: PathBuf,
    },
    /// Symlink resolves outside the allowlist (genuine escape).
    Escape,
}

/// Recursively walks the symlink chain starting at `path` to determine its
/// disposition relative to the allowlist enforced by `jail`.
///
/// - Returns `Internal` when all hops are within the allowlist and the final
///   target exists (or the path is not a symlink).
/// - Returns `Broken` when all hops are within the allowlist but the final
///   target does not exist (dangling symlink within the sandbox).
/// - Returns `Escape` when any hop resolves to a path outside the allowlist.
#[expect(
    clippy::redundant_pub_crate,
    reason = "see MAX_SYMLINK_HOPS above — clippy::redundant_pub_crate and \
              clippy::unreachable_pub contradict each other for a pub(crate) \
              item inside a private module"
)]
pub(crate) fn symlink_chain_disposition(
    path: &Path,
    jail: &dyn PathJailPort,
    lstat_of_start: &std::fs::Metadata,
    depth: u8,
) -> SymlinkDisposition {
    if depth >= MAX_SYMLINK_HOPS {
        return SymlinkDisposition::Escape;
    }

    // Read the immediate link target.
    let Ok(direct_target) = std::fs::read_link(path) else {
        // Not a symlink or cannot read link — check if it exists.
        return if std::fs::symlink_metadata(path).is_ok() {
            SymlinkDisposition::Internal {
                lstat: lstat_of_start.clone(),
                resolved: path.to_path_buf(),
            }
        } else {
            SymlinkDisposition::Broken
        };
    };

    // Resolve relative targets relative to the symlink's parent directory.
    let resolved_target = if direct_target.is_absolute() {
        direct_target
    } else {
        path.parent()
            .map(|p| p.join(&direct_target))
            .unwrap_or(direct_target)
    };

    // Use the jail to check if `resolved_target` is within an allowed root.
    // `JailedPath::new_jailed` + `jail.jail` checks WITHOUT following symlinks.
    // Return Escape only when the jail reports a security boundary violation
    // (PathOutsideAllowlist, SymlinkEscape, etc.).  NotFound / IoError mean the
    // path is absent but the prefix is within the allowlist — fall through to
    // the symlink_metadata check below.
    let jailed_target = JailedPath::new_jailed(resolved_target.clone());
    if let Err(e) = jail.jail(&jailed_target, &resolved_target) {
        // NotFound / IoError mean the path is absent but the prefix is within
        // the allowlist — fall through.  All other errors (PathOutsideAllowlist,
        // SymlinkEscape, …) mean the hop crosses a security boundary.
        let absent_ok = matches!(
            e,
            SubstrateError::NotFound { .. } | SubstrateError::IoError { .. }
        );
        if !absent_ok {
            return SymlinkDisposition::Escape;
        }
    }

    // Target is within the allowlist. Check if it exists (lstat).
    match std::fs::symlink_metadata(&resolved_target) {
        Err(_) => SymlinkDisposition::Broken,
        Ok(target_meta) if target_meta.file_type().is_symlink() => {
            // Target is itself a symlink — recurse.
            symlink_chain_disposition(&resolved_target, jail, lstat_of_start, depth + 1)
        },
        Ok(_) => SymlinkDisposition::Internal {
            lstat: lstat_of_start.clone(),
            resolved: resolved_target,
        },
    }
}
