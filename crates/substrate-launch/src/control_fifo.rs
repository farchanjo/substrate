//! Control FIFO IPC for the detached supervisor (ADR-0068 "Registry and IPC
//! permission boundary" + "Lock-free multiplexed IPC").
//!
//! A detached Stack's supervisor exposes a single named pipe,
//! `<stack_dir>/control.fifo` (where `stack_dir` is the directory returned by
//! [`crate::supervisor_registry::open_stack_registry`]), as its command
//! channel. A live MCP server process forwards `launch.down` / `launch.reload`
//! requests into an already-running detached supervisor by writing one
//! newline-delimited [`ControlFrame`] JSON document per `write(2)` call via
//! [`write_control_frame`]; the supervisor's reactor loop consumes frames from
//! the [`tokio::sync::mpsc::Receiver<ControlFrame>`] returned by
//! [`spawn_control_reader`].
//!
//! # Framing (ADR-0068 "Lock-free multiplexed IPC")
//!
//! POSIX guarantees a `write(2)` of at most `PIPE_BUF` bytes to a pipe/FIFO is
//! atomic and never interleaved with a concurrent writer's bytes. This module
//! exploits that guarantee instead of adding a length-prefix framing layer:
//! each `write(2)` call is exactly one frame, terminated by a single trailing
//! `\n` that the reader splits on. [`MAX_COMMAND_FRAME_SIZE`] reserves one byte
//! of `PIPE_BUF` for that trailing newline, so `frame_json.len() + 1 <=
//! PIPE_BUF` always holds for an accepted frame, which is the precondition for
//! the kernel's atomicity guarantee. A writer that would break this bound is
//! rejected in [`write_control_frame`] *before* any `write(2)` call; a reader
//! that observes a line longer than the bound discards it (it is by
//! construction not a frame this module ever wrote) rather than attempting to
//! reassemble it into a [`ControlFrame`].
//!
//! # Registry and IPC permission boundary (ADR-0068)
//!
//! `control.fifo` is created via `mkfifo(2)` at mode `0600`. Before opening
//! the read end (and before every write), the path is `stat`-checked and
//! rejected with [`LaunchError::RegistryInsecure`] unless it is a FIFO
//! (`S_ISFIFO`), mode exactly `0600`, and owned by the invoking effective uid.
//! This guards against a hostile pre-created FIFO (for example, a symlink
//! swapped in at the same path, or a co-resident reader/writer racing to
//! create it first) — an insecure node is never silently re-created or
//! re-secured.
//!
//! # For the reactor-loop stage (Milestone 2 continuation)
//!
//! Call [`spawn_control_reader`] once per detached Stack, after
//! [`crate::supervisor_registry::open_stack_registry`] has returned the
//! Stack's registry directory. The returned `Receiver<ControlFrame>` is the
//! only consumer-facing surface: drive a `tokio::select!` loop against it
//! (alongside child-exit futures, TTL timers, etc.) and dispatch on the
//! [`ControlFrame`] variant. Dropping the `Receiver` stops the reader task on
//! its next loop iteration. The reader survives across multiple writer
//! sessions (it reopens the FIFO after each `EOF`, i.e. after every writer
//! closes its end), so a fresh MCP server process can attach and send a
//! command at any time after the supervisor starts.
//!
//! References: ADR-0033, ADR-0068.

use std::io::Write as _;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use nix::sys::stat::Mode;
use nix::unistd::mkfifo;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use substrate_domain::launch::errors::LaunchError;

use crate::supervisor_registry::{insecure, run_blocking};

/// File name of the control FIFO under a Stack's registry directory.
pub const CONTROL_FIFO_FILE: &str = "control.fifo";

/// Mode applied to (and required of) `control.fifo`.
const SECURE_FIFO_MODE: u32 = 0o600;

/// Maximum size, in bytes, of one [`ControlFrame`] JSON document (excluding
/// the trailing newline delimiter).
///
/// `PIPE_BUF - 1` reserves exactly one byte for the trailing `\n`, so a
/// frame at this bound plus its delimiter is `PIPE_BUF` bytes — the largest
/// `write(2)` POSIX still guarantees is atomic on a pipe/FIFO.
pub const MAX_COMMAND_FRAME_SIZE: usize = libc::PIPE_BUF - 1;

/// Bounded capacity of the channel returned by [`spawn_control_reader`].
///
/// Control commands (`down`, `reload`) are low-frequency operator actions;
/// this only needs enough slack to avoid blocking the reader thread if the
/// reactor loop is briefly busy.
const CONTROL_FRAME_CHANNEL_CAPACITY: usize = 16;

/// A single command sent over `control.fifo` to a detached supervisor.
///
/// Newline-delimited JSON, one [`ControlFrame`] per `write(2)` call (see the
/// module-level "Framing" section). Defined here
/// (`substrate_launch::control_fifo::ControlFrame`) as the contract the
/// supervisor's reactor loop consumes from the channel returned by
/// [`spawn_control_reader`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlFrame {
    /// Requests a graceful teardown of the named Stack.
    Down {
        /// The Stack id to tear down.
        stack_id: String,
    },
    /// Requests the supervisor reload the Stack from (optionally) a new
    /// Profile path, restarting the dependency-closure of changed Services.
    Reload {
        /// The Stack id to reload.
        stack_id: String,
        /// `Some(path)` to reload from a different `.substrate.toml`; `None`
        /// to re-read the Profile path already on file.
        profile_path: Option<String>,
    },
}

/// Creates (if absent) and security-verifies `<stack_dir>/control.fifo`,
/// returning its path.
///
/// # Errors
///
/// Returns [`LaunchError::RegistryInsecure`] when `mkfifo(2)` fails, or when
/// an existing node at the path is not a FIFO, is not mode `0600`, or is not
/// owned by the invoking effective uid.
pub async fn ensure_control_fifo(stack_dir: &Path) -> Result<PathBuf, LaunchError> {
    let stack_dir = stack_dir.to_path_buf();
    run_blocking(move || ensure_control_fifo_at(&stack_dir)).await
}

/// Spawns the long-lived control-FIFO reader task for `stack_dir` and returns
/// the [`mpsc::Receiver`] a reactor loop consumes [`ControlFrame`]s from.
///
/// The task creates/verifies `control.fifo` (per [`ensure_control_fifo`]),
/// blocks opening its read end (Zone B, ADR-0003 — `std::fs::File`, never
/// `tokio::net::unix::pipe`, per the STDIO-transport-only constraint), and
/// loops parsing newline-delimited frames until the returned `Receiver` is
/// dropped or an unrecoverable I/O error occurs.
#[must_use]
pub fn spawn_control_reader(stack_dir: PathBuf) -> mpsc::Receiver<ControlFrame> {
    let (tx, rx) = mpsc::channel(CONTROL_FRAME_CHANNEL_CAPACITY);
    let _join_handle = tokio::task::spawn_blocking(move || control_reader_loop(&stack_dir, &tx));
    rx
}

/// Serializes `frame`, rejects it if oversized, and writes it as one
/// newline-terminated `write(2)` call to `<stack_dir>/control.fifo`.
///
/// Opening a FIFO write-only blocks until a reader has the read end open;
/// since [`spawn_control_reader`]'s task keeps the read end open across the
/// supervisor's lifetime (reopening after every writer session), this call
/// only blocks if no supervisor reader is currently attached.
///
/// # Errors
///
/// Returns [`LaunchError::FrameTooLarge`] when the serialized `frame` (before
/// the trailing newline) exceeds [`MAX_COMMAND_FRAME_SIZE`] — checked and
/// rejected *before* any `write(2)` call. Returns
/// [`LaunchError::RegistryInsecure`] when the FIFO cannot be created/verified
/// or the write fails. Returns [`LaunchError::InvalidProfile`] if `frame`
/// somehow fails to serialize (infallible for the current variants, kept as a
/// typed error rather than a panic).
pub async fn write_control_frame(stack_dir: &Path, frame: &ControlFrame) -> Result<(), LaunchError> {
    let stack_dir = stack_dir.to_path_buf();
    let frame = frame.clone();
    run_blocking(move || write_control_frame_at(&stack_dir, &frame)).await
}

/// Synchronous implementation of [`ensure_control_fifo`].
///
/// Tolerates a racing concurrent creator (`EEXIST` from `mkfifo(2)`): the
/// node is still security-verified below regardless of who created it, so a
/// benign create-create race never silently skips the security check.
fn ensure_control_fifo_at(stack_dir: &Path) -> Result<PathBuf, LaunchError> {
    let path = stack_dir.join(CONTROL_FIFO_FILE);
    if !path.exists() {
        // `mode_t` is `u16` on Darwin and `u32` on Linux; `SECURE_FIFO_MODE`
        // is declared `u32` (it is also compared against `Metadata::mode()`,
        // which is always `u32`), so the narrowing cast is needed here only.
        #[expect(clippy::cast_possible_truncation, reason = "0o600 fits both u16 and u32 mode_t")]
        let fifo_mode = SECURE_FIFO_MODE as nix::sys::stat::mode_t;
        match mkfifo(&path, Mode::from_bits_truncate(fifo_mode)) {
            Ok(()) | Err(nix::Error::EEXIST) => {},
            Err(_) => return Err(insecure(&path)),
        }
    }
    verify_fifo_secure(&path)?;
    Ok(path)
}

/// `stat`-checks `path`: rejects anything that is not a FIFO at exactly
/// [`SECURE_FIFO_MODE`], owned by the invoking effective uid. An
/// already-insecure node is never silently re-created or re-secured.
fn verify_fifo_secure(path: &Path) -> Result<(), LaunchError> {
    let meta = std::fs::metadata(path).map_err(|_| insecure(path))?;
    if !meta.file_type().is_fifo() {
        return Err(insecure(path));
    }
    if meta.permissions().mode() & 0o777 != SECURE_FIFO_MODE {
        return Err(insecure(path));
    }
    if meta.uid() != nix::unistd::geteuid().as_raw() {
        return Err(insecure(path));
    }
    Ok(())
}

/// Synchronous implementation of [`write_control_frame`].
///
/// Serializes and size-checks `frame` *before* touching the filesystem at
/// all, so an oversized frame is rejected without creating `control.fifo` as
/// a side effect.
fn write_control_frame_at(stack_dir: &Path, frame: &ControlFrame) -> Result<(), LaunchError> {
    let mut buf = serde_json::to_vec(frame).map_err(|e| LaunchError::InvalidProfile {
        msg: format!("failed to serialize control frame: {e}"),
    })?;
    if buf.len() > MAX_COMMAND_FRAME_SIZE {
        return Err(LaunchError::FrameTooLarge { size: buf.len() });
    }
    buf.push(b'\n');

    let path = ensure_control_fifo_at(stack_dir)?;
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).map_err(|_| insecure(&path))?;
    file.write_all(&buf).map_err(|_| insecure(&path))
}

/// Body of the blocking task spawned by [`spawn_control_reader`].
///
/// Reopens `control.fifo` after every `EOF` (i.e. after the last writer of a
/// session closes its end) so the reader survives across multiple,
/// independent writer sessions for the supervisor's entire lifetime. Returns
/// when the FIFO cannot be created/verified/opened, or once the channel's
/// `Receiver` half is dropped.
fn control_reader_loop(stack_dir: &Path, tx: &mpsc::Sender<ControlFrame>) {
    loop {
        if tx.is_closed() {
            return;
        }
        let fifo_path = match ensure_control_fifo_at(stack_dir) {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(%error, "control.fifo: failed to create/verify control FIFO; reader task exiting");
                return;
            },
        };
        let file = match std::fs::File::open(&fifo_path) {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!(%error, "control.fifo: failed to open read end; reader task exiting");
                return;
            },
        };
        if !read_frames(&file, tx) {
            return;
        }
    }
}

/// Reads newline-delimited frames from `file` until `EOF`, dispatching each
/// to `tx`. Returns `false` when the channel's `Receiver` half has been
/// dropped (the caller must stop entirely); `true` on a clean `EOF` (the
/// caller should reopen the FIFO for the next writer session).
///
/// Accumulates at most [`MAX_COMMAND_FRAME_SIZE`] bytes per in-flight line: a
/// line that grows past the bound flips into a discard mode that keeps
/// draining (and dropping) bytes up to the next `\n` without ever holding the
/// oversized frame in memory, satisfying the "never attempt reassembly"
/// requirement (ADR-0068).
fn read_frames(file: &std::fs::File, tx: &mpsc::Sender<ControlFrame>) -> bool {
    use std::io::Read as _;

    let mut reader = std::io::BufReader::new(file);
    let mut line = Vec::with_capacity(MAX_COMMAND_FRAME_SIZE);
    let mut oversized = false;
    let mut byte = [0u8; 1];

    loop {
        match reader.read(&mut byte) {
            Ok(0) => return true,
            Ok(_) => {},
            Err(error) => {
                tracing::warn!(%error, "control.fifo: read error on control FIFO; reader task exiting");
                return true;
            },
        }

        if byte[0] != b'\n' {
            if !oversized && line.len() >= MAX_COMMAND_FRAME_SIZE {
                oversized = true;
                line.clear();
            }
            if !oversized {
                line.push(byte[0]);
            }
            continue;
        }

        let keep_going = if oversized {
            record_oversized_frame();
            true
        } else {
            dispatch_frame(&line, tx)
        };
        line.clear();
        oversized = false;
        if !keep_going {
            return false;
        }
    }
}

/// Parses one accumulated line as a [`ControlFrame`] and forwards it to `tx`.
///
/// Returns `false` only when `tx.blocking_send` fails because the
/// `Receiver` half was dropped (the caller must stop the reader loop).
/// Malformed JSON is logged and discarded without stopping the loop.
fn dispatch_frame(line: &[u8], tx: &mpsc::Sender<ControlFrame>) -> bool {
    match serde_json::from_slice::<ControlFrame>(line) {
        Ok(frame) => tx.blocking_send(frame).is_ok(),
        Err(error) => {
            tracing::warn!(%error, "control.fifo: discarding malformed command frame");
            true
        },
    }
}

/// Logs a discarded oversized control frame with the
/// [`LaunchError::FrameTooLarge`] code and a fresh correlation id, mirroring
/// how this error is recorded elsewhere in the launch BC.
fn record_oversized_frame() {
    let error = LaunchError::FrameTooLarge {
        size: MAX_COMMAND_FRAME_SIZE,
    };
    let correlation_id = Uuid::now_v7();
    tracing::warn!(
        code = error.code(),
        %correlation_id,
        "control.fifo: discarding oversized command frame (exceeded MAX_COMMAND_FRAME_SIZE)"
    );
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use tempfile::TempDir;
    use tokio::time::{Duration, timeout};

    use super::*;

    /// Runs `fut` to completion on a fresh current-thread runtime, then
    /// [`tokio::runtime::Runtime::shutdown_background`]s it instead of
    /// letting it `Drop`.
    ///
    /// [`spawn_control_reader`]'s task is *intentionally* eternal — per the
    /// module docs, it blocks in `open()` again after every `EOF`, for the
    /// supervisor's entire lifetime, and only stops once its `Receiver` is
    /// dropped *and* a writer happens to wake its next `open()` call. A
    /// plain `Runtime::drop()` waits for every outstanding `spawn_blocking`
    /// task to finish, so it would hang forever on that still-blocked
    /// reader thread; `shutdown_background()` returns immediately and lets
    /// the leftover OS thread die with the test process instead.
    fn block_on_with_background_shutdown<F: std::future::Future>(fut: F) -> F::Output {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build current-thread test runtime");
        let out = rt.block_on(fut);
        rt.shutdown_background();
        out
    }

    #[test]
    fn round_trips_a_frame_through_a_real_fifo() {
        block_on_with_background_shutdown(async {
            let dir = TempDir::new().expect("tempdir");
            let stack_dir = dir.path().to_path_buf();

            let mut rx = spawn_control_reader(stack_dir.clone());
            // Give the reader task time to create the FIFO and block on open().
            tokio::time::sleep(Duration::from_millis(50)).await;

            let sent = ControlFrame::Down {
                stack_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
            };
            write_control_frame(&stack_dir, &sent).await.expect("write frame");

            let received = timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("did not time out")
                .expect("channel not closed");
            assert_eq!(received, sent);
        });
    }

    #[test]
    fn round_trips_a_reload_frame_with_no_profile_path() {
        block_on_with_background_shutdown(async {
            let dir = TempDir::new().expect("tempdir");
            let stack_dir = dir.path().to_path_buf();

            let mut rx = spawn_control_reader(stack_dir.clone());
            tokio::time::sleep(Duration::from_millis(50)).await;

            let sent = ControlFrame::Reload {
                stack_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
                profile_path: None,
            };
            write_control_frame(&stack_dir, &sent).await.expect("write frame");

            let received = timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("did not time out")
                .expect("channel not closed");
            assert_eq!(received, sent);
        });
    }

    #[tokio::test]
    async fn oversize_frame_is_rejected_before_any_write() {
        let dir = TempDir::new().expect("tempdir");
        let stack_dir = dir.path().to_path_buf();

        let huge_path = "x".repeat(MAX_COMMAND_FRAME_SIZE * 2);
        let frame = ControlFrame::Reload {
            stack_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
            profile_path: Some(huge_path),
        };

        let err = write_control_frame(&stack_dir, &frame).await.expect_err("oversize frame rejected");
        assert!(matches!(err, LaunchError::FrameTooLarge { .. }), "got {err:?}");
        assert!(
            !stack_dir.join(CONTROL_FIFO_FILE).exists(),
            "oversize rejection must happen before the FIFO is even required to exist for the write"
        );
    }

    #[tokio::test]
    async fn ensure_control_fifo_creates_a_real_fifo_at_0600() {
        let dir = TempDir::new().expect("tempdir");
        let path = ensure_control_fifo(dir.path()).await.expect("create fifo");

        let meta = std::fs::metadata(&path).expect("stat fifo");
        assert!(meta.file_type().is_fifo(), "control.fifo must be a real FIFO");
        assert_eq!(meta.permissions().mode() & 0o777, SECURE_FIFO_MODE, "control.fifo must be 0600");
    }

    #[test]
    fn insecure_mode_is_rejected() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join(CONTROL_FIFO_FILE);
        mkfifo(&path, Mode::from_bits_truncate(0o644)).expect("mkfifo 0644");

        let err = verify_fifo_secure(&path).expect_err("0644 fifo rejected");
        assert!(matches!(err, LaunchError::RegistryInsecure { .. }), "got {err:?}");
    }

    #[test]
    fn non_fifo_node_is_rejected() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join(CONTROL_FIFO_FILE);
        std::fs::write(&path, b"not a fifo").expect("write regular file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(SECURE_FIFO_MODE)).expect("chmod 0600");

        let err = verify_fifo_secure(&path).expect_err("regular file rejected");
        assert!(matches!(err, LaunchError::RegistryInsecure { .. }), "got {err:?}");
    }
}
