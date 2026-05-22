//! FFI shim for macOS-specific syscalls used by `substrate-fs-index`.
//!
//! This crate is NOT a public API. It exists solely to isolate all `unsafe`
//! FFI declarations from the main `substrate-fs-index` crate, which enforces
//! `#![cfg_attr(not(test), forbid(unsafe_code))]`.
//!
//! # Safety policy
//!
//! Every `unsafe` block carries a SAFETY comment justifying each invariant.
//! This crate opts out of the workspace-wide `unsafe_code = "deny"` lint via
//! `[lints] workspace = false` in its `Cargo.toml`, as permitted by ADR-0042
//! (macOS Native Primitive Exception) and ADR-0044 (SIMD / Low-Level Syscall
//! Exception Policy).

// This entire crate is macOS-only. On any other target it compiles to an
// empty library with no symbols.
#![cfg(target_os = "macos")]
#![allow(unsafe_code)]

/// Attribute-list descriptor passed to `getattrlistbulk(2)`.
///
/// Maps directly to the kernel `attrlist` struct defined in
/// `<sys/attr.h>`. All fields use the same ABI layout as the C struct
/// (`#[repr(C)]` is mandatory).
///
/// # Layout
///
/// | Field         | Size | Purpose                                     |
/// |---------------|------|---------------------------------------------|
/// | `bitmapcount` | u16  | Must be `ATTR_BIT_MAP_COUNT` (5).            |
/// | `reserved`    | u16  | Kernel-reserved; callers must set to 0.     |
/// | `commonattr`  | u32  | Common attribute bitmask (name, stat, etc.) |
/// | `volattr`     | u32  | Volume attributes.                          |
/// | `dirattr`     | u32  | Directory attributes.                       |
/// | `fileattr`    | u32  | File attributes.                            |
/// | `forkattr`    | u32  | Fork attributes.                            |
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct attrlist {
    /// Must be `ATTR_BIT_MAP_COUNT` (5) per `<sys/attr.h>`.
    pub bitmapcount: u16,
    /// Reserved; set to 0.
    pub reserved: u16,
    /// Common attribute bitmap (e.g., `ATTR_CMN_NAME | ATTR_CMN_MODTIME`).
    pub commonattr: u32,
    /// Volume attribute bitmap (unused for file walks; set to 0).
    pub volattr: u32,
    /// Directory attribute bitmap (unused for file walks; set to 0).
    pub dirattr: u32,
    /// File-specific attribute bitmap (unused in baseline; set to 0).
    pub fileattr: u32,
    /// Fork attribute bitmap (unused; set to 0).
    pub forkattr: u32,
}

/// Common-attribute bit: return the entry name string.
///
/// Source: `<sys/attr.h>` `ATTR_CMN_NAME`.
pub const ATTR_CMN_NAME: u32 = 0x0000_0001;

/// Common-attribute bit: return modification time.
///
/// Source: `<sys/attr.h>` `ATTR_CMN_MODTIME`.
pub const ATTR_CMN_MODTIME: u32 = 0x0000_0400;

/// Required value for `attrlist::bitmapcount`.
///
/// Source: `<sys/attr.h>` `ATTR_BIT_MAP_COUNT`.
pub const ATTR_BIT_MAP_COUNT: u16 = 5;

/// Maximum entries returned per `getattrlistbulk(2)` call.
///
/// The kernel accepts values up to 4096; 1024 is the practical sweet spot
/// for balancing syscall count and buffer size per ADR-0042.
pub const GETATTRLISTBULK_MAX_COUNT: u32 = 1024;

/// Calls the macOS `getattrlistbulk(2)` syscall via `libc`.
///
/// Returns the number of entries written to `attrBuf`, `0` when no entries
/// remain, or `-1` on error (errno is set by the kernel).
///
/// # Parameters
///
/// - `dirfd`: open file descriptor for the directory to enumerate.
/// - `attr_list`: pointer to a caller-initialised `attrlist` describing the
///   requested attributes.
/// - `attr_buf`: caller-allocated buffer to receive packed attribute records.
/// - `attr_buf_size`: byte length of `attr_buf`.
/// - `options`: option flags; pass `0` for default behaviour.
///
/// # Safety
///
/// - `dirfd` must be a valid, open directory file descriptor for the lifetime
///   of this call.
/// - `attr_list` must point to a correctly initialised `attrlist` with
///   `bitmapcount == ATTR_BIT_MAP_COUNT`.
/// - `attr_buf` must be valid for writes of at least `attr_buf_size` bytes and
///   must not alias `attr_list`.
/// - `attr_buf_size` must accurately reflect the allocated size of `attr_buf`.
#[inline]
pub unsafe fn getattrlistbulk(
    dirfd: libc::c_int,
    attr_list: *const attrlist,
    attr_buf: *mut libc::c_void,
    attr_buf_size: libc::size_t,
    options: u64,
) -> libc::c_int {
    // SAFETY: All preconditions are documented on this function's signature and
    // must be upheld by the caller. `libc::getattrlistbulk` is the direct
    // libc binding for the macOS `getattrlistbulk(2)` syscall. The call writes
    // up to `attr_buf_size` bytes into `attr_buf` and reads the `attrlist`
    // via `attr_list`; both pointers must remain valid for the call's duration.
    // `attr_list` is cast to `*mut c_void` because the libc binding takes a
    // mutable void pointer (the kernel does not mutate it, but the C signature
    // does not carry `const`).
    unsafe {
        libc::getattrlistbulk(
            dirfd,
            attr_list.cast::<libc::c_void>().cast_mut(),
            attr_buf,
            attr_buf_size,
            options,
        )
    }
}
