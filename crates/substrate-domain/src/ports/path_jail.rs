//! `PathJailPort` — inbound port for kernel-enforced path confinement per ADR-0035.
//!
//! Tier is selected by `PathJailFactory` at startup (`LinuxOpenat2`,
//! `MacosONoFollowAny`, or `UserspaceDegraded`). When `UserspaceDegraded` is
//! selected and `security.refuse_degraded_jail = true` (default), the composition
//! root aborts startup with `SUBSTRATE_JAIL_DEGRADED_REFUSED`.
//!
//! This port is CPU-bound (path resolution does not block on I/O). Adapters
//! implement the trait synchronously; callers wrap invocations in
//! `spawn_blocking` if the jailed open is needed on the async path.

use std::path::Path;

use crate::errors::SubstrateResult;
use crate::value_objects::JailedPath;

/// Inbound port for kernel-enforced path confinement per ADR-0035.
///
/// The port validates that a raw caller-supplied path stays within the
/// configured allowlist roots, resolving symlinks through the kernel jailing
/// primitive rather than userspace string manipulation.
///
/// Synchronous by design: path resolution is CPU-bound. The composition root
/// wraps calls in `tokio::task::spawn_blocking` when invoked from async context.
pub trait PathJailPort: Send + Sync {
    /// Validates `raw_path` and returns a `JailedPath` if both invariants hold:
    ///
    /// 1. The resolved path stays within `allowlist_root`.
    /// 2. No symlink escape is possible (kernel-enforced on tier 1).
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_PATH_OUTSIDE_ALLOWLIST` — resolved path exits the root.
    /// - `SUBSTRATE_SYMLINK_ESCAPE` — symlink resolution would exit the root.
    /// - `SUBSTRATE_PATH_TRAVERSAL_BLOCKED` — `..` or encoded traversal detected.
    /// - `SUBSTRATE_PERMISSION_DENIED` — OS-level `EPERM` / `EACCES`.
    /// - `SUBSTRATE_NOT_FOUND` — a path component does not exist on disk.
    fn jail(&self, allowlist_root: &JailedPath, raw_path: &Path) -> SubstrateResult<JailedPath>;
}
