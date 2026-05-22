//! `JailedPath` — a filesystem path that has been validated as safe.
//!
//! Mirrors `#JailedPath` in `docs/arch/schemas/shared_kernel.cue`.
//! Construction of a `JailedPath` asserts both invariants:
//! 1. The path is a fully-resolved, canonical absolute path (no `..` or symlink escapes).
//! 2. The path starts with one of the configured allowlist roots.
//!
//! The validating constructor lives in `substrate-policy` (adapter crate).
//! This domain type only exposes the opaque new-type; it never validates.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// An opaque wrapper around [`PathBuf`] representing a jail-validated path.
///
/// # Invariants (enforced by `substrate-policy`, not this crate)
///
/// - The inner path is absolute and canonical (no `..`, no unresolved symlinks).
/// - The inner path starts with one of the configured allowlist roots.
///
/// Domain code that receives a `JailedPath` may treat these invariants as
/// holding. The domain never re-validates; re-validation is the policy adapter's
/// responsibility.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JailedPath(PathBuf);

impl JailedPath {
    /// Constructs a `JailedPath` without any validation.
    ///
    /// # Safety (semantic, not `unsafe`)
    ///
    /// Callers MUST guarantee that `p` satisfies both `JailedPath` invariants
    /// before calling this constructor. Only `substrate-policy` should call this
    /// function; all other crates should receive `JailedPath` values from the
    /// policy adapter.
    #[must_use]
    pub(crate) const fn new_unchecked(p: PathBuf) -> Self {
        Self(p)
    }

    /// Constructs a `JailedPath` after allowlist + path-jail validation has been
    /// performed by the caller.
    ///
    /// # When to call
    ///
    /// This constructor MUST only be called from `substrate-policy`.
    /// All other crates must receive `JailedPath` values through the
    /// [`PathJailPort`](crate::ports::path_jail::PathJailPort) abstraction.
    ///
    /// Misuse is detected by the Rego policy `policies/path_jail_construction.rego`
    /// (Wave 6 CI); direct callers outside `substrate-policy` fail CI lint.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Only valid inside substrate-policy after validation passes:
    /// let jailed = JailedPath::new_jailed(canonical_path);
    /// ```
    #[must_use]
    pub const fn new_jailed(path: PathBuf) -> Self {
        Self::new_unchecked(path)
    }

    /// Returns the inner path as a [`Path`] reference.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    /// Consumes the `JailedPath` and returns the inner [`PathBuf`].
    #[must_use]
    pub fn into_inner(self) -> PathBuf {
        self.0
    }
}

impl AsRef<Path> for JailedPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl std::fmt::Display for JailedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.display().fmt(f)
    }
}
