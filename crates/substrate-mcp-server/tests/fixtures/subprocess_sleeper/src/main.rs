// subprocess_sleeper — test fixture binary for subprocess lifecycle tests.
// See top-level `#![allow(unsafe_code)]` — this is test-only code; libc signal
// handlers and raw fd I/O require unsafe blocks that are forbidden by the
// workspace `unsafe_code = "deny"` lint applied to production targets.
#![allow(
    unsafe_code,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo,
    missing_docs,
    rustdoc::all,
    reason = "fixture binary for cucumber tests: libc signal handlers and raw fd I/O require unsafe; \
              test-only code exempt from workspace lint baselines"
)]
//
//
// Usage:
//   subprocess_sleeper --sleep-secs N [--on-sigterm-cleanup] [--watchdog-aware]
//
// Arguments:
//   --sleep-secs N          Required. Sleep for N seconds then exit 0.
//   --on-sigterm-cleanup    Optional. Install a SIGTERM handler that writes
//                           "SIGTERM_RECEIVED\n" to stdout then exits 0.
//                           Without this flag the default SIGTERM disposition
//                           (process termination) applies.
//   --watchdog-aware        Optional. Read the SUBSTRATE_WATCHDOG_FD environment
//                           variable. If set, start a watchdog thread that reads
//                           from that fd until EOF, then calls _exit(0).
//                           Demonstrates the cooperative macOS watchdog pattern
//                           from ADR-0053 §"macOS Watchdog Pipe Pattern".
//
// Exit codes:
//   0   Normal exit (sleep completed, SIGTERM handler invoked, or watchdog EOF).
//   1   Argument error (--sleep-secs missing or invalid).
//
// References: ADR-0053 (cascade contract), ADR-0052 (subprocess BC).

use std::env;
use std::time::Duration;

fn main() {
    let args: Vec<String> = env::args().collect();

    // --- Parse arguments -------------------------------------------------------

    let mut sleep_secs: Option<u64> = None;
    let mut on_sigterm_cleanup = false;
    let mut watchdog_aware = false;

    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--sleep-secs" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("subprocess_sleeper: --sleep-secs requires a value");
                    std::process::exit(1);
                }
                match args[i].parse::<u64>() {
                    Ok(n) => sleep_secs = Some(n),
                    Err(e) => {
                        eprintln!("subprocess_sleeper: --sleep-secs value invalid: {e}");
                        std::process::exit(1);
                    },
                }
            },
            "--on-sigterm-cleanup" => {
                on_sigterm_cleanup = true;
            },
            "--watchdog-aware" => {
                watchdog_aware = true;
            },
            other => {
                eprintln!("subprocess_sleeper: unknown argument: {other}");
                std::process::exit(1);
            },
        }
        i += 1;
    }

    let sleep_secs = match sleep_secs {
        Some(n) => n,
        None => {
            eprintln!("subprocess_sleeper: --sleep-secs N is required");
            std::process::exit(1);
        },
    };

    // Suppress unused-variable warnings on non-Unix builds.
    let _ = on_sigterm_cleanup;
    let _ = watchdog_aware;

    // --- Install SIGTERM handler (cooperative cleanup mode) -------------------

    #[cfg(unix)]
    if on_sigterm_cleanup {
        // SAFETY: signal(2) is async-signal-safe. The handler only calls
        // write(2) and _exit(2), both on the POSIX async-signal-safe list.
        unsafe {
            libc::signal(
                libc::SIGTERM,
                sigterm_handler as *const () as libc::sighandler_t,
            );
        }
    }

    // --- Watchdog thread (cooperative EOF detection) --------------------------

    #[cfg(unix)]
    if watchdog_aware {
        if let Ok(fd_str) = env::var("SUBSTRATE_WATCHDOG_FD") {
            if let Ok(fd) = fd_str.parse::<i32>() {
                std::thread::spawn(move || watchdog_thread(fd));
            }
        }
    }

    // --- Main sleep -----------------------------------------------------------

    std::thread::sleep(Duration::from_secs(sleep_secs));
}

// ---------------------------------------------------------------------------
// SIGTERM handler (cooperative cleanup mode)
// ---------------------------------------------------------------------------

#[cfg(unix)]
extern "C" fn sigterm_handler(_sig: libc::c_int) {
    // SAFETY: write(2) and _exit(2) are async-signal-safe per POSIX §2.4.3.
    // We write a marker to stdout (fd 1) so tests can detect SIGTERM receipt.
    // No allocation, no lock acquisition, no format! macro.
    let msg = b"SIGTERM_RECEIVED\n";
    unsafe {
        libc::write(1, msg.as_ptr().cast(), msg.len());
        libc::_exit(0);
    }
}

// ---------------------------------------------------------------------------
// Watchdog thread (cooperative EOF detection)
// ---------------------------------------------------------------------------

/// Blocks reading from `fd` until EOF, then calls `_exit(0)`.
///
/// This is the substrate-aware side of the watchdog pipe pattern from
/// ADR-0053 §"macOS Watchdog Pipe Pattern". When substrate dies (for any
/// reason), the write end of the pipe is closed and read() here returns 0.
#[cfg(unix)]
fn watchdog_thread(fd: i32) {
    let mut buf = [0u8; 1];
    loop {
        // SAFETY: read(2) is safe to call from a background thread.
        // `fd` is a valid open file descriptor inherited from the parent via exec.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), 1) };
        if n == 0 {
            // EOF: parent (substrate) exited or closed the write end.
            // Exit immediately per ADR-0053 §"macOS Watchdog Pipe Pattern".
            unsafe { libc::_exit(0) };
        }
        if n < 0 {
            // n < 0 means an error. EINTR → retry; anything else → treat as EOF.
            // Use libc::errno() which is portable across Linux and macOS.
            // SAFETY: errno() reads a thread-local variable; safe in any thread.
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::Interrupted {
                unsafe { libc::_exit(0) };
            }
        }
    }
}
