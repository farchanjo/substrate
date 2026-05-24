// substrate_stdout_writer — minimal test fixture binary for subprocess cucumber tests.
//
// Usage:
//   subprocess_stdout_writer --stdout-bytes N --stderr-bytes M --exit-code K
//   subprocess_stdout_writer --stdout-bytes N --stderr-bytes M --exit-code K --sleep-secs S
//
// Writes exactly N bytes of 'A' to stdout and M bytes of 'B' to stderr, then
// exits with the supplied exit code.  When --sleep-secs is given it sleeps for
// S seconds BEFORE writing any output, allowing quota tests to hold the process
// alive while a 5th spawn is attempted.
//
// All flags are optional; defaults: stdout-bytes=0, stderr-bytes=0, exit-code=0.
//
// The --watchdog-aware flag is accepted but is a no-op in this fixture.
// Full watchdog pipe integration requires SUBSTRATE_WATCHDOG_FD env var handling
// that is exercised by the dedicated subprocess_sleeper fixture (Wave 2.5b).

// Missing docs is acceptable for a test fixture binary.
#![allow(missing_docs, reason = "test fixture binary — no public API")]

use std::io::{Write, stderr, stdout};
use std::{env, process, thread, time::Duration};

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut stdout_bytes: usize = 0;
    let mut stderr_bytes: usize = 0;
    let mut exit_code: i32 = 0;
    let mut sleep_secs: u64 = 0;
    let mut watchdog_aware = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--stdout-bytes" => {
                i += 1;
                stdout_bytes = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
            },
            "--stderr-bytes" => {
                i += 1;
                stderr_bytes = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
            },
            "--exit-code" => {
                i += 1;
                exit_code = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
            },
            "--sleep-secs" => {
                i += 1;
                sleep_secs = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
            },
            "--watchdog-aware" => {
                watchdog_aware = true;
            },
            _ => {},
        }
        i += 1;
    }

    // The --watchdog-aware flag is accepted for compatibility but is a no-op here.
    // Watchdog pipe (SUBSTRATE_WATCHDOG_FD) handling is implemented in the
    // subprocess_sleeper fixture binary (Wave 2.5b) which uses it for lifecycle tests.
    let _ = watchdog_aware;

    // Optional sleep before writing output (used by quota tests).
    if sleep_secs > 0 {
        thread::sleep(Duration::from_secs(sleep_secs));
    }

    // Write stdout payload: N bytes of 'A', in 4 KiB chunks.
    if stdout_bytes > 0 {
        let mut out = stdout();
        let chunk = vec![b'A'; stdout_bytes.min(4096)];
        let mut remaining = stdout_bytes;
        while remaining > 0 {
            let to_write = remaining.min(chunk.len());
            if out.write_all(&chunk[..to_write]).is_err() {
                break;
            }
            remaining -= to_write;
        }
        let _ = out.flush();
    }

    // Write stderr payload: M bytes of 'B', in 4 KiB chunks.
    if stderr_bytes > 0 {
        let mut err = stderr();
        let chunk = vec![b'B'; stderr_bytes.min(4096)];
        let mut remaining = stderr_bytes;
        while remaining > 0 {
            let to_write = remaining.min(chunk.len());
            if err.write_all(&chunk[..to_write]).is_err() {
                break;
            }
            remaining -= to_write;
        }
        let _ = err.flush();
    }

    process::exit(exit_code);
}
