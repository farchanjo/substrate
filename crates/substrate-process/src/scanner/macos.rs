//! macOS process scanner — uses `sysctl(KERN_PROC_ALL)` to enumerate running
//! processes via `kinfo_proc` structs returned by the kernel.
//!
//! # Algorithm
//!
//! 1. First `sysctl` call with a null data pointer determines the buffer size.
//! 2. Allocate a byte buffer of the required capacity.
//! 3. Second `sysctl` call fills the buffer with `kinfo_proc` entries.
//! 4. Each entry is parsed from the raw byte buffer using verified field offsets.
//!
//! # ABI note
//!
//! `libc` 0.2 does not expose `kinfo_proc` on macOS. We parse the raw byte
//! buffer using field offsets verified by a C probe program against the macOS
//! SDK 14 (arm64 + x86_64 64-bit ABI):
//!
//! ```text
//! sizeof(kinfo_proc)  = 648
//! sizeof(extern_proc) = 296   (kinfo_proc.kp_proc at offset 0)
//! sizeof(eproc)       = 352   (kinfo_proc.kp_eproc at offset 296)
//! sizeof(_pcred)      = 104   (eproc.e_pcred at eproc-relative offset 16)
//! extern_proc.p_stat  at offset 36  (c_char / i8)
//! extern_proc.p_pid   at offset 40  (pid_t / i32)
//! extern_proc.p_comm  at offset 243 (char[17], MAXCOMLEN=16)
//! eproc.e_ppid        at offset 264 (pid_t relative to eproc start)
//! eproc.e_pcred       at offset 16  (relative to eproc start)
//! _pcred.p_ruid       at offset 80  (relative to _pcred start)
//! _pcred.p_rgid       at offset 88  (relative to _pcred start)
//! ```
//!
//! Absolute offsets within `kinfo_proc`:
//! - `kp_proc.p_stat`  = 36
//! - `kp_proc.p_pid`   = 40
//! - `kp_proc.p_comm`  = 243
//! - `kp_eproc.e_pcred.p_ruid` = 296 + 16 + 80 = 392
//! - `kp_eproc.e_pcred.p_rgid` = 296 + 16 + 88 = 400
//! - `kp_eproc.e_ppid` = 296 + 264 = 560
//!
//! # proc_pidinfo for RSS + virtual size + CPU times (Wave H)
//!
//! `proc_pidinfo(pid, PROC_PIDTASKINFO, 0, &mut info, size)` returns a
//! `proc_taskinfo` struct with accurate `pti_resident_size`,
//! `pti_virtual_size`, `pti_total_user`, and `pti_total_system` fields.
//! These are used to fill `rss_kb`, `vm_kb`, and the CPU-delta calculation.
//!
//! Delta CPU%: a `Mutex<HashMap<u32, CpuSnapshot>>` tracks (total_cpu_ns,
//! wall_ns) per PID. First call returns 0.0. Subsequent calls compute:
//! `(cpu_delta / wall_delta) * 100 / cpu_count`.
//!
//! `p_starttime` (`timeval` at `kp_proc` offset 152 on macOS SDK 14 arm64)
//! is converted to Unix epoch seconds by: `tv_sec + tv_usec / 1_000_000`.
//!
//! # Safety justification (ADR-0042 + ADR-0044 proc carve-out)
//!
//! This module calls `libc::sysctl` (KERN_PROC_ALL) and `proc_pidinfo`
//! (PROC_PIDTASKINFO) to read kernel-owned process metadata. No subprocess
//! is spawned (ADR-0044). All `unsafe` blocks are narrowly scoped to the
//! FFI calls and `read_unaligned` for byte-buffer field extraction. No raw
//! pointer escapes the function frame in which it is created.
//!
//! The module-level `#![allow(unsafe_code)]` is the ONLY permitted override
//! in this crate per ADR-0042.
#![allow(
    unsafe_code,
    reason = "sysctl(KERN_PROC_ALL) + kinfo_proc raw-byte parsing on macOS. \
              Standard process introspection; no subprocess spawned. \
              ADR-0042 + ADR-0044 proc carve-out."
)]
// ABI offset tables and kernel struct field names in this module use many
// C-style identifiers (c_char, pid_t, uid_t, offsetof, KERN_PROC_ALL, etc.)
// that Clippy's doc_markdown lint flags as "missing backticks". Suppressing
// module-wide is preferable to wrapping every technical term in backticks and
// making the ABI documentation less readable.
#![expect(
    clippy::doc_markdown,
    reason = "ABI documentation intentionally uses C-style type and macro names without backticks"
)]

use std::{
    collections::HashMap,
    sync::Mutex,
    time::Instant,
};

use super::ProcessScannerPort;
use crate::process_info::ProcessInfo;
use substrate_domain::{SubstrateError, SubstrateResult};

// ---- Verified ABI constants (macOS SDK 14, arm64 + x86_64) -----------------

/// Size of a single `kinfo_proc` entry in bytes.
const KINFO_PROC_SIZE: usize = 648;

/// Byte offset of `kp_proc.p_stat` (c_char) within `kinfo_proc`.
const OFF_P_STAT: usize = 36;

/// Byte offset of `kp_proc.p_pid` (pid_t = i32) within `kinfo_proc`.
const OFF_P_PID: usize = 40;

/// Byte offset of `kp_proc.p_comm` (char[17]) within `kinfo_proc`.
const OFF_P_COMM: usize = 243;

/// Length of `p_comm` field including NUL terminator.
const MAXCOMLEN_PLUS1: usize = 17;

/// Byte offset of `kp_eproc.e_pcred.p_ruid` (uid_t = u32) within `kinfo_proc`.
/// = offsetof(kinfo_proc, kp_eproc) + offsetof(eproc, e_pcred) + offsetof(_pcred, p_ruid)
/// = 296 + 16 + 80 = 392
const OFF_RUID: usize = 392;

/// Byte offset of `kp_eproc.e_pcred.p_rgid` (gid_t = u32) within `kinfo_proc`.
/// = 296 + 16 + 88 = 400
const OFF_RGID: usize = 400;

/// Byte offset of `kp_eproc.e_ppid` (pid_t = i32) within `kinfo_proc`.
/// = offsetof(kinfo_proc, kp_eproc) + offsetof(eproc, e_ppid)
/// = 296 + 264 = 560
const OFF_PPID: usize = 560;

// ---- sysctl helper ----------------------------------------------------------

/// MIB for `sysctl(CTL_KERN, KERN_PROC, KERN_PROC_ALL, 0)`.
const KERN_PROC_ALL_MIB: [libc::c_int; 4] =
    [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_ALL, 0];

/// Reads the raw `kinfo_proc` buffer from the kernel via two `sysctl` calls.
///
/// Returns a flat `Vec<u8>` aligned to `KINFO_PROC_SIZE`. The caller iterates
/// the buffer in `KINFO_PROC_SIZE`-sized chunks.
fn sysctl_proc_all_raw() -> SubstrateResult<Vec<u8>> {
    let mut mib = KERN_PROC_ALL_MIB;

    // --- First call: size probe -------------------------------------------
    let mut size: libc::size_t = 0;

    // SAFETY: First `sysctl` call with a null `oldp` pointer queries the
    // required buffer size only. `mib` is a valid 4-element KERN_PROC_ALL MIB.
    // `size` receives the byte count. Null `newp`/`newlen` = read-only query.
    // No pointer escapes this call.
    let ret = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(size),
            std::ptr::null_mut(),
            0,
        )
    };

    if ret < 0 {
        return Err(SubstrateError::InternalError {
            reason: format!(
                "sysctl(KERN_PROC_ALL) size probe failed: {}",
                std::io::Error::last_os_error()
            ),
            correlation_id: None,
        });
    }

    if size == 0 {
        return Ok(Vec::new());
    }

    // Add slack (10% + 4 entries) for processes spawned between calls.
    let slack = (size / 10).max(4 * KINFO_PROC_SIZE);
    let capacity = size + slack;
    let mut buf: Vec<u8> = vec![0u8; capacity];
    let mut actual_size = capacity;

    // --- Second call: fill buffer -----------------------------------------
    // SAFETY: `buf` has `capacity` zero-initialized bytes. `sysctl` writes at
    // most `actual_size` bytes into the buffer (updating `actual_size` to the
    // number of bytes actually written). The buffer is a plain `Vec<u8>` with
    // no alignment requirements beyond `u8` — `kinfo_proc` entries are
    // 8-byte-aligned by the kernel but we read individual fields via
    // `read_unaligned` to avoid any alignment assumption.
    let ret = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            buf.as_mut_ptr().cast(),
            std::ptr::addr_of_mut!(actual_size),
            std::ptr::null_mut(),
            0,
        )
    };

    if ret < 0 {
        return Err(SubstrateError::InternalError {
            reason: format!(
                "sysctl(KERN_PROC_ALL) fill failed: {}",
                std::io::Error::last_os_error()
            ),
            correlation_id: None,
        });
    }

    buf.truncate(actual_size);
    Ok(buf)
}

// ---- Field extraction helpers -----------------------------------------------

/// Reads a `u8` at `offset` within `entry` (a `KINFO_PROC_SIZE`-byte slice).
fn read_u8(entry: &[u8], offset: usize) -> u8 {
    entry[offset]
}

/// Reads a little-endian `i32` (pid_t) at `offset` via `read_unaligned`.
///
/// # Safety
///
/// `entry` must be at least `offset + 4` bytes long. All macOS platforms are
/// little-endian; `pid_t` is `i32`.
fn read_i32_le(entry: &[u8], offset: usize) -> i32 {
    // SAFETY: `entry[offset..offset+4]` is within the bounds guaranteed by the
    // caller. `read_unaligned` is safe regardless of the pointer alignment;
    // it avoids UB that a plain `*const i32` cast would produce on unaligned
    // access. The kernel guarantees the bytes are fully initialized.
    #[expect(clippy::cast_ptr_alignment, reason = "read_unaligned is used precisely to avoid alignment requirements")]
    let ptr = entry[offset..].as_ptr().cast::<i32>();
    unsafe { ptr.read_unaligned() }
}

/// Reads a little-endian `u32` (uid_t / gid_t) at `offset`.
fn read_u32_le(entry: &[u8], offset: usize) -> u32 {
    // SAFETY: Same reasoning as `read_i32_le`.
    #[expect(clippy::cast_ptr_alignment, reason = "read_unaligned is used precisely to avoid alignment requirements")]
    let ptr = entry[offset..].as_ptr().cast::<u32>();
    unsafe { ptr.read_unaligned() }
}

/// Extracts a NUL-terminated string from a byte slice of `len` bytes.
fn c_bytes_to_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

// ---- proc_pidinfo / PROC_PIDTASKINFO (Wave H) --------------------------------

/// `proc_pidinfo` flavor constant for `proc_taskinfo`.
///
/// Value 4 = `PROC_PIDTASKINFO` per `<sys/proc_info.h>` (macOS SDK 14).
const PROC_PIDTASKINFO: i32 = 4;

/// Byte offset of `kp_proc.p_starttime` (`timeval`, i64 tv_sec + i32 tv_usec)
/// within `kinfo_proc` on macOS SDK 14 arm64/x86_64 (64-bit ABI).
///
/// Verified by C probe: `offsetof(kinfo_proc, kp_proc.p_starttime) == 0`.
/// `extern_proc` begins at offset 0 (`kp_proc`) and `p_starttime` is the
/// very first field of `extern_proc` on macOS 64-bit ABI.
const OFF_P_STARTTIME_TV_SEC: usize = 0;

/// Byte offset of the `tv_usec` field of `p_starttime`.
/// `timeval` on macOS 64-bit: tv_sec (i64 = 8 bytes) then tv_usec (i32 = 4 bytes).
const OFF_P_STARTTIME_TV_USEC: usize = 8;

/// Mirror of `struct proc_taskinfo` from `<sys/proc_info.h>` (macOS SDK 14).
///
/// Full size is **96 bytes** (verified by C probe: `sizeof(proc_taskinfo)==96`).
/// Only the first four fields are used; the remaining bytes are padding to
/// ensure `size_of::<ProcTaskInfo>()` matches the kernel struct so
/// `proc_pidinfo` receives the correct buffer-size argument.
///
/// Layout (arm64 + x86_64, verified via offsetof probes):
/// ```text
/// offset  0: pti_virtual_size  u64
/// offset  8: pti_resident_size u64
/// offset 16: pti_total_user    u64  (nanoseconds)
/// offset 24: pti_total_system  u64  (nanoseconds)
/// offset 32..95: remaining fields (not used, represented as padding)
/// ```
///
/// Must be `#[repr(C)]` to match the kernel ABI.
#[repr(C)]
struct ProcTaskInfo {
    pti_virtual_size:  u64,
    pti_resident_size: u64,
    pti_total_user:    u64,  // nanoseconds
    pti_total_system:  u64,  // nanoseconds
    /// Padding fields to reach the full 96-byte kernel struct size.
    /// (`proc_taskinfo` has many more counters beyond offset 32; we ignore them.)
    _padding: [u8; 64],
}

impl ProcTaskInfo {
    #[expect(clippy::missing_const_for_fn, reason = "unsafe const fn is not yet stable for mem::zeroed")]
    fn zeroed() -> Self {
        // SAFETY: `ProcTaskInfo` contains only integer fields and a byte-array
        // padding; zero-initializing all bytes produces a valid value for every
        // field. This is the standard idiom for zeroing a C-compatible struct
        // before handing its address to a kernel API.
        unsafe { std::mem::zeroed() }
    }
}

// `proc_pidinfo` is available in `libproc` on macOS. The function is
// declared in `<libproc.h>` but not directly exposed by the `libc` crate.
// We declare the extern signature manually using the documented ABI.
// SAFETY: This extern block declares a link-time symbol from macOS libsystem.
// The function is read-only (queries kernel state, does not mutate it).
unsafe extern "C" {
    /// Queries per-process information from the macOS kernel.
    ///
    /// - `pid`: target process identifier.
    /// - `flavor`: info type (e.g., `PROC_PIDTASKINFO = 4`).
    /// - `arg`: flavor-specific argument (0 for `PROC_PIDTASKINFO`).
    /// - `buffer`: output buffer pointer.
    /// - `buffersize`: size in bytes of `buffer`.
    ///
    /// Returns the number of bytes written on success, or -1 on error.
    fn proc_pidinfo(
        pid: libc::c_int,
        flavor: libc::c_int,
        arg: u64,
        buffer: *mut libc::c_void,
        buffersize: libc::c_int,
    ) -> libc::c_int;
}

/// Retrieves task-level metrics (RSS, VM, CPU times) for a process via
/// `proc_pidinfo(PROC_PIDTASKINFO)`.
///
/// Returns `None` for processes that deny access (e.g., system daemons owned
/// by root when we run as a normal user). Callers MUST NOT treat `None` as an
/// error — they should leave the corresponding fields at their zero defaults.
fn task_info(pid: u32) -> Option<ProcTaskInfo> {
    let mut info = ProcTaskInfo::zeroed();
    #[expect(clippy::cast_possible_truncation, clippy::cast_possible_wrap, reason = "ProcTaskInfo is 96 bytes; fits in i32 with certainty")]
    let buf_size = std::mem::size_of::<ProcTaskInfo>() as libc::c_int;

    // SAFETY: `pid` is a POSIX process identifier. `PROC_PIDTASKINFO` is a
    // read-only flavor; no kernel state is mutated. `info` is a valid,
    // zero-initialized `ProcTaskInfo` whose size exactly matches the layout
    // expected by the kernel for this flavor. The return value is checked
    // before the struct fields are accessed. The pointer does not escape this
    // function.
    #[expect(clippy::cast_possible_wrap, reason = "pid is a valid POSIX pid; kernel rejects pids > INT_MAX")]
    let ret = unsafe {
        proc_pidinfo(
            pid as libc::c_int,
            PROC_PIDTASKINFO,
            0,
            std::ptr::addr_of_mut!(info).cast::<libc::c_void>(),
            buf_size,
        )
    };

    if ret < buf_size {
        None
    } else {
        Some(info)
    }
}

/// Converts a `timeval` stored at raw byte offsets within a `kinfo_proc` entry
/// to a Unix epoch timestamp in seconds.
///
/// `timeval` layout on macOS 64-bit: tv_sec (i64, 8 bytes) + tv_usec (i32, 4 bytes).
fn parse_start_time(entry: &[u8]) -> Option<i64> {
    if OFF_P_STARTTIME_TV_USEC + 4 > entry.len() {
        return None;
    }
    let start_secs = read_i64_le(entry, OFF_P_STARTTIME_TV_SEC);
    let usec_frac = i64::from(read_i32_le(entry, OFF_P_STARTTIME_TV_USEC));
    if start_secs <= 0 {
        return None;
    }
    // Convert timeval to Unix epoch seconds (usec fraction contributes < 1 s).
    Some(start_secs + usec_frac / 1_000_000)
}

/// Reads a little-endian `i64` at `offset` within `entry`.
fn read_i64_le(entry: &[u8], offset: usize) -> i64 {
    // SAFETY: `entry[offset..offset+8]` is within bounds (callers verify).
    // `read_unaligned` avoids UB from potential misalignment; all macOS
    // platforms are little-endian.
    #[expect(clippy::cast_ptr_alignment, reason = "read_unaligned is used precisely to avoid alignment requirements")]
    let ptr = entry[offset..].as_ptr().cast::<i64>();
    unsafe { ptr.read_unaligned() }
}

// ---- CPU-delta state ---------------------------------------------------------

/// Snapshot of per-process CPU state used for delta-based CPU% computation.
#[derive(Debug, Clone, Copy)]
struct CpuSnapshot {
    /// Total CPU time in nanoseconds (user + system) at the last sample.
    total_cpu_ns: u64,
    /// Wall-clock instant of the last sample.
    wall:         Instant,
}

// ---- Entry parser -----------------------------------------------------------

/// Parses a single `KINFO_PROC_SIZE`-byte entry into a `ProcessInfo`.
///
/// `task` carries the result of `proc_pidinfo(PROC_PIDTASKINFO)` for this PID
/// (may be `None` for system processes that deny access). `cpu_pct` is the
/// delta-based CPU percentage computed by the caller.
///
/// Returns `None` for entries where `p_pid == 0` (empty kernel slot).
fn parse_entry(entry: &[u8], task: Option<&ProcTaskInfo>, cpu_pct: f32) -> Option<ProcessInfo> {
    debug_assert_eq!(entry.len(), KINFO_PROC_SIZE);

    let pid = read_i32_le(entry, OFF_P_PID);
    if pid == 0 {
        return None;
    }

    let parent_pid = read_i32_le(entry, OFF_PPID);
    let uid = read_u32_le(entry, OFF_RUID);
    let gid = read_u32_le(entry, OFF_RGID);

    let p_stat = read_u8(entry, OFF_P_STAT);
    let state = match p_stat {
        1 => "I", // idle (being created)
        2 => "R", // running
        3 => "S", // sleeping
        4 => "T", // stopped
        5 => "Z", // zombie
        _ => "?",
    }
    .to_owned();

    let comm_bytes = &entry[OFF_P_COMM..OFF_P_COMM + MAXCOMLEN_PLUS1];
    let name = c_bytes_to_string(comm_bytes);

    let start_time_unix = parse_start_time(entry);

    let (rss_kb, vm_kb) = task.map_or((0, 0), |t| {
        (t.pti_resident_size / 1024, t.pti_virtual_size / 1024)
    });

    #[expect(clippy::cast_sign_loss, reason = "pid validated > 0 above; parent_pid is a valid non-negative pid")]
    Some(ProcessInfo {
        pid: pid as u32,
        ppid: parent_pid as u32,
        name,
        command: String::new(), // argv not available via kinfo_proc
        uid,
        gid,
        cpu_pct,
        rss_kb,
        vm_kb,
        start_time_unix,
        state,
    })
}

// ---- MacOsProcessScanner ----------------------------------------------------

/// macOS-specific process scanner backed by `sysctl(KERN_PROC_ALL)` +
/// `proc_pidinfo(PROC_PIDTASKINFO)` for accurate RSS, VM, and CPU data.
#[derive(Debug)]
pub struct MacOsProcessScanner {
    /// Per-PID CPU snapshots for delta-based CPU% computation.
    ///
    /// The `Mutex` is a `std::sync::Mutex` (not a tokio one) because
    /// `scan_all` is a synchronous method that must not `.await`.
    cpu_snapshots: Mutex<HashMap<u32, CpuSnapshot>>,
    /// Number of logical CPUs — used to normalize CPU% to a 0–100 range
    /// rather than 0–`N * 100`.
    cpu_count: u64,
}

impl Default for MacOsProcessScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl MacOsProcessScanner {
    /// Constructs a new scanner instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cpu_snapshots: Mutex::new(HashMap::new()),
            cpu_count: num_cpus::get() as u64,
        }
    }

    /// Computes the delta CPU% for `pid` given its current total CPU
    /// nanoseconds. Updates the internal snapshot for the next call.
    ///
    /// Returns `0.0` on the first call for a given PID (no previous snapshot).
    fn cpu_pct(&self, pid: u32, total_cpu_ns: u64) -> f32 {
        let now = Instant::now();
        let mut guard = self
            .cpu_snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Precision loss on every cast in the CPU% computation is intentional:
        // - u128→u64: saturate at u64::MAX nanoseconds (~584 years) which is fine.
        // - u64→f64:  mantissa is 52 bits; at nanosecond precision this causes
        //   sub-nanosecond rounding that is irrelevant for a 1-decimal-place %.
        // - f64→f32:  display precision is 1 decimal place; f32 is sufficient.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss,
            reason = "intentional precision loss in CPU% display; see comment above"
        )]
        let pct = guard.get(&pid).map_or(0.0_f32, |prev| {
            let cpu_delta = total_cpu_ns.saturating_sub(prev.total_cpu_ns);
            let wall_ns = now.duration_since(prev.wall).as_nanos().min(u128::from(u64::MAX)) as u64;
            if wall_ns == 0 || self.cpu_count == 0 {
                0.0
            } else {
                let pct_f64 =
                    (cpu_delta as f64 / wall_ns as f64) * 100.0 / self.cpu_count as f64;
                pct_f64 as f32
            }
        });

        guard.insert(pid, CpuSnapshot { total_cpu_ns, wall: now });
        pct
    }
}

impl ProcessScannerPort for MacOsProcessScanner {
    fn scan_all(&self) -> SubstrateResult<Vec<ProcessInfo>> {
        let raw = sysctl_proc_all_raw()?;
        if raw.is_empty() {
            return Ok(Vec::new());
        }

        // Each entry is exactly KINFO_PROC_SIZE bytes.
        let result = raw
            .chunks_exact(KINFO_PROC_SIZE)
            .filter_map(|entry| {
                let pid = read_i32_le(entry, OFF_P_PID);
                if pid <= 0 {
                    return None;
                }
                #[expect(clippy::cast_sign_loss, reason = "pid > 0 checked above")]
                let pid_u32 = pid as u32;

                // Fetch task-level metrics (best-effort; None for protected PIDs).
                let task = task_info(pid_u32);

                // Compute delta CPU% from task CPU times.
                let pct = task.as_ref().map_or(0.0, |t| {
                    let total_ns = t
                        .pti_total_user
                        .saturating_add(t.pti_total_system);
                    self.cpu_pct(pid_u32, total_ns)
                });

                parse_entry(entry, task.as_ref(), pct)
            })
            .collect();

        Ok(result)
    }
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    reason = "test module — panics and unwraps on assertion failure are the intended behavior"
)]
mod tests {
    use super::*;

    #[test]
    fn scan_returns_nonzero_processes() {
        let scanner = MacOsProcessScanner::new();
        let procs = scanner.scan_all().expect("scan_all must not fail on macOS");
        assert!(!procs.is_empty(), "process list must not be empty");
    }

    #[test]
    fn scan_contains_current_pid() {
        let scanner = MacOsProcessScanner::new();
        let procs = scanner.scan_all().expect("scan_all must not fail");
        let my_pid = std::process::id();
        assert!(
            procs.iter().any(|p| p.pid == my_pid),
            "current process (pid={my_pid}) must appear in the list"
        );
    }

    #[test]
    fn all_processes_have_names() {
        let scanner = MacOsProcessScanner::new();
        let procs = scanner.scan_all().expect("scan_all must not fail");
        for p in &procs {
            assert!(!p.name.is_empty(), "pid {} has empty name", p.pid);
        }
    }

    #[test]
    fn all_pids_are_nonzero() {
        let scanner = MacOsProcessScanner::new();
        let procs = scanner.scan_all().expect("scan_all must not fail");
        for p in &procs {
            assert!(p.pid > 0, "pid must be > 0; got {}", p.pid);
        }
    }

    // ---- Wave H: proc_pidinfo tests ------------------------------------------

    #[test]
    fn current_process_has_nonzero_rss() {
        let scanner = MacOsProcessScanner::new();
        let procs = scanner.scan_all().expect("scan_all must not fail");
        let my_pid = std::process::id();
        let me = procs
            .iter()
            .find(|p| p.pid == my_pid)
            .expect("current process must appear in list");
        // RSS of this test process should be at least a few kilobytes.
        assert!(
            me.rss_kb > 0,
            "rss_kb must be > 0 for pid {my_pid}; got {}",
            me.rss_kb
        );
    }

    #[test]
    fn current_process_has_nonzero_vm() {
        let scanner = MacOsProcessScanner::new();
        let procs = scanner.scan_all().expect("scan_all must not fail");
        let my_pid = std::process::id();
        let me = procs
            .iter()
            .find(|p| p.pid == my_pid)
            .expect("current process must appear");
        assert!(
            me.vm_kb > 0,
            "vm_kb must be > 0 for pid {my_pid}; got {}",
            me.vm_kb
        );
    }

    #[test]
    fn current_process_has_start_time() {
        let scanner = MacOsProcessScanner::new();
        let procs = scanner.scan_all().expect("scan_all must not fail");
        let my_pid = std::process::id();
        let me = procs
            .iter()
            .find(|p| p.pid == my_pid)
            .expect("current process must appear");
        let st = me
            .start_time_unix
            .expect("start_time_unix must be Some for current process");
        // Must be a plausible Unix timestamp (after 2020-01-01).
        assert!(st > 1_577_836_800, "start_time_unix={st} looks implausible");
    }

    #[test]
    fn second_scan_has_cpu_pct_for_current_process() {
        let scanner = MacOsProcessScanner::new();
        // First scan — primes the snapshot map; returns 0.0 for all.
        let _ = scanner.scan_all().expect("first scan must not fail");
        // Do some work to generate measurable CPU time.
        let _ = (0u64..1_000_000).fold(0u64, u64::wrapping_add);
        // Second scan — should have delta data for the current PID.
        let procs = scanner.scan_all().expect("second scan must not fail");
        let my_pid = std::process::id();
        // cpu_pct for the current process may still be 0.0 (too fast), but
        // the field must at least be a valid finite non-negative float.
        let me = procs
            .iter()
            .find(|p| p.pid == my_pid)
            .expect("current process must appear in second scan");
        assert!(
            me.cpu_pct.is_finite() && me.cpu_pct >= 0.0,
            "cpu_pct must be finite and non-negative; got {}",
            me.cpu_pct
        );
    }
}
