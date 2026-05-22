//! JSON-RPC `id: null` compatibility shim (ADR-0013 §null-id).
//!
//! # Problem
//!
//! JSON-RPC 2.0 §4 allows `"id"` to be a number, string, **or null**.  The MCP
//! protocol inherits this: when a client sends a request with `"id": null`, the
//! server MUST respond with `"id": null` in the response envelope.
//!
//! rmcp 1.7 models `RequestId` as `NumberOrString` (never null).  Its
//! `#[serde(untagged)]` deserialization of `JsonRpcMessage` attempts each
//! variant in order:
//!
//!  1. `JsonRpcRequest` — fails because `id: null` cannot deserialise as
//!     `NumberOrString`.
//!  2. `JsonRpcResponse` — fails (no `result` field).
//!  3. `JsonRpcNotification` — **succeeds** (extra fields are silently ignored,
//!     so `id: null` is dropped and the message is treated as a notification).
//!  4. `JsonRpcError` — never reached.
//!
//! The result: a `tools/call` with `"id": null` is silently discarded as a
//! notification.  No response is sent.  The client times out (20 s deadline).
//!
//! # Fix — line-level rewrite shim
//!
//! `NullIdStdin` wraps `tokio::io::Stdin`.  For each incoming line it:
//!
//!  1. Parses the raw JSON.
//!  2. If the top-level `"id"` KEY is **present** and its value is `null`,
//!     rewrites the value to `NULL_ID_SENTINEL` and sets the shared
//!     `NullIdActive` flag.
//!  3. Re-serialises to bytes and passes them to rmcp.
//!
//! `NullIdStdout` wraps `tokio::io::Stdout`.  For each outgoing line it:
//!
//!  1. Parses the raw JSON.
//!  2. If the `"id"` field equals `NULL_ID_SENTINEL` AND the active flag is
//!     set, rewrites it back to `null` and clears the flag.
//!  3. Re-serialises and flushes.
//!
//! The sentinel is `i64::MIN` (–9223372036854775808).  No well-behaved MCP
//! client uses this as a request id; rmcp's `NumberOrString::Number` stores
//! `i64`, so the sentinel round-trips through rmcp without truncation.
//!
//! Per ADR-0013: the shim MUST NOT alter messages whose `"id"` field is absent
//! (notifications) or already a valid `NumberOrString` value.

// `redundant_pub_crate` fires for `pub(crate)` items in a `pub(crate)` module —
// contradicts `unreachable_pub`.  Keep `pub(crate)` for explicit intent.
#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional in pub(crate) modules"
)]

use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Sentinel integer substituted for `null` request ids on the inbound path.
const NULL_ID_SENTINEL: i64 = i64::MIN;

/// Shared flag: `true` while a null-id request is in flight.
///
/// A single boolean suffices because the STDIO transport is strictly sequential
/// (one client, one connection, one request at a time on the STDIO pipe).
type NullIdActive = Arc<Mutex<bool>>;

/// Creates a matched (stdin shim, stdout shim) pair that transparently
/// translates `"id": null` in JSON-RPC framing.
///
/// Both ends share the same `NullIdActive` flag so the outbound shim only
/// rewrites responses when there is an active null-id request.
pub(crate) fn null_id_pair() -> (NullIdStdin, NullIdStdout) {
    let active: NullIdActive = Arc::new(Mutex::new(false));
    (
        NullIdStdin::new(tokio::io::stdin(), Arc::clone(&active)),
        NullIdStdout::new(tokio::io::stdout(), Arc::clone(&active)),
    )
}

// ---- Inbound shim -----------------------------------------------------------

/// `AsyncRead` wrapper around `tokio::io::Stdin` that rewrites `"id": null`
/// lines to `"id": <NULL_ID_SENTINEL>` before passing bytes to rmcp.
pub(crate) struct NullIdStdin {
    inner: tokio::io::Stdin,
    /// Buffered bytes from a rewritten line that haven't been read yet.
    pending: Vec<u8>,
    /// Raw byte accumulator — collects bytes until `\n`.
    line_buf: Vec<u8>,
    active: NullIdActive,
}

impl NullIdStdin {
    const fn new(inner: tokio::io::Stdin, active: NullIdActive) -> Self {
        Self {
            inner,
            pending: Vec::new(),
            line_buf: Vec::new(),
            active,
        }
    }
}

impl AsyncRead for NullIdStdin {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        // Drain buffered rewritten bytes first.
        if !this.pending.is_empty() {
            let n = buf.remaining().min(this.pending.len());
            buf.put_slice(&this.pending[..n]);
            this.pending.drain(..n);
            return Poll::Ready(Ok(()));
        }

        // Accumulate one byte at a time until `\n` so we can rewrite whole lines.
        loop {
            let mut byte = [0u8; 1];
            let mut rb = ReadBuf::new(&mut byte);
            match Pin::new(&mut this.inner).poll_read(cx, &mut rb) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {
                    if rb.filled().is_empty() {
                        // EOF
                        return Poll::Ready(Ok(()));
                    }
                    this.line_buf.push(byte[0]);
                    if byte[0] == b'\n' {
                        let transformed = rewrite_null_id_inbound(&this.line_buf, &this.active);
                        this.line_buf.clear();
                        let n = buf.remaining().min(transformed.len());
                        buf.put_slice(&transformed[..n]);
                        if transformed.len() > n {
                            this.pending.extend_from_slice(&transformed[n..]);
                        }
                        return Poll::Ready(Ok(()));
                    }
                }
            }
        }
    }
}

/// Rewrites a single JSON-RPC line if `"id": null` is present.
///
/// Returns the (possibly rewritten) bytes including the trailing `\n`.
fn rewrite_null_id_inbound(line: &[u8], active: &NullIdActive) -> Vec<u8> {
    // Fast path: not JSON.
    if !line.contains(&b'"') {
        return line.to_vec();
    }
    let stripped = line.strip_suffix(b"\n").unwrap_or(line);
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(stripped) else {
        return line.to_vec();
    };
    let Some(obj) = value.as_object_mut() else {
        return line.to_vec();
    };
    // Act only when "id" KEY is present AND its value is JSON null.
    if obj.get("id").is_some_and(serde_json::Value::is_null) {
        obj.insert(
            "id".to_owned(),
            serde_json::Value::Number(serde_json::Number::from(NULL_ID_SENTINEL)),
        );
        if let Ok(mut guard) = active.lock() {
            *guard = true;
        }
        let mut out = serde_json::to_vec(&value).unwrap_or_else(|_| stripped.to_vec());
        out.push(b'\n');
        return out;
    }
    line.to_vec()
}

// ---- Outbound shim ----------------------------------------------------------

/// `AsyncWrite` wrapper around `tokio::io::Stdout` that rewrites
/// `"id": <NULL_ID_SENTINEL>` back to `"id": null` in outgoing responses.
pub(crate) struct NullIdStdout {
    inner: tokio::io::Stdout,
    /// Bytes buffered when a `poll_write` could not be fully flushed.
    pending: Vec<u8>,
    pending_offset: usize,
    /// Raw byte accumulator — collects bytes until `\n`.
    line_buf: Vec<u8>,
    active: NullIdActive,
}

impl NullIdStdout {
    const fn new(inner: tokio::io::Stdout, active: NullIdActive) -> Self {
        Self {
            inner,
            pending: Vec::new(),
            pending_offset: 0,
            line_buf: Vec::new(),
            active,
        }
    }
}

impl AsyncWrite for NullIdStdout {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        src: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        // Flush any buffered remainder from a previous rewrite.
        while this.pending_offset < this.pending.len() {
            let rest = &this.pending[this.pending_offset..];
            match Pin::new(&mut this.inner).poll_write(cx, rest) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(n)) => this.pending_offset += n,
            }
        }
        if this.pending_offset >= this.pending.len() {
            this.pending.clear();
            this.pending_offset = 0;
        }

        // Accumulate src; flush each complete line.
        let mut consumed = 0usize;
        for &byte in src {
            this.line_buf.push(byte);
            consumed += 1;
            if byte == b'\n' {
                let transformed = rewrite_null_id_outbound(&this.line_buf, &this.active);
                this.line_buf.clear();
                // Best-effort synchronous flush of the transformed line.
                let mut off = 0usize;
                while off < transformed.len() {
                    match Pin::new(&mut this.inner).poll_write(cx, &transformed[off..]) {
                        Poll::Pending => {
                            // Buffer the remainder; we've consumed `consumed` bytes.
                            this.pending.extend_from_slice(&transformed[off..]);
                            return Poll::Ready(Ok(consumed));
                        }
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Ready(Ok(n)) => off += n,
                    }
                }
            }
        }
        Poll::Ready(Ok(consumed))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// Rewrites a JSON-RPC response line, restoring `"id": null` when the
/// sentinel id is present and the active flag is set.
fn rewrite_null_id_outbound(line: &[u8], active: &NullIdActive) -> Vec<u8> {
    let is_active = active.lock().is_ok_and(|g| *g);
    if !is_active {
        return line.to_vec();
    }
    let stripped = line.strip_suffix(b"\n").unwrap_or(line);
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(stripped) else {
        return line.to_vec();
    };
    let Some(obj) = value.as_object_mut() else {
        return line.to_vec();
    };
    if obj.get("id").and_then(serde_json::Value::as_i64) == Some(NULL_ID_SENTINEL) {
        obj.insert("id".to_owned(), serde_json::Value::Null);
        if let Ok(mut guard) = active.lock() {
            *guard = false;
        }
        let mut out = serde_json::to_vec(&value).unwrap_or_else(|_| stripped.to_vec());
        out.push(b'\n');
        return out;
    }
    line.to_vec()
}

// ---- Unit tests -------------------------------------------------------------

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test code: panicking assertions are idiomatic in unit tests"
)]
mod tests {
    use super::*;

    fn make_active(val: bool) -> NullIdActive {
        Arc::new(Mutex::new(val))
    }

    #[test]
    fn inbound_null_id_rewrites_to_sentinel() {
        let active = make_active(false);
        let line =
            b"{\"jsonrpc\":\"2.0\",\"method\":\"tools/call\",\"id\":null,\"params\":{}}\n";
        let result = rewrite_null_id_inbound(line, &active);
        let v: serde_json::Value = serde_json::from_slice(
            result.strip_suffix(b"\n").expect("trailing newline"),
        )
        .expect("valid JSON");
        assert_eq!(
            v["id"].as_i64(),
            Some(NULL_ID_SENTINEL),
            "id must be rewritten to sentinel"
        );
        assert!(*active.lock().expect("lock"), "active flag must be set");
    }

    #[test]
    fn inbound_normal_id_not_rewritten() {
        let active = make_active(false);
        let line =
            b"{\"jsonrpc\":\"2.0\",\"method\":\"tools/call\",\"id\":42,\"params\":{}}\n";
        let result = rewrite_null_id_inbound(line, &active);
        assert_eq!(result, line, "normal id must not be altered");
        assert!(
            !*active.lock().expect("lock"),
            "active flag must remain false"
        );
    }

    #[test]
    fn inbound_notification_not_rewritten() {
        let active = make_active(false);
        let line = b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";
        let result = rewrite_null_id_inbound(line, &active);
        assert_eq!(result, line, "notification (no id key) must not be altered");
    }

    #[test]
    fn outbound_sentinel_restored_to_null() {
        let active = make_active(true);
        let line = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{NULL_ID_SENTINEL},\"result\":{{\"tools\":[]}}}}\n"
        );
        let result = rewrite_null_id_outbound(line.as_bytes(), &active);
        let v: serde_json::Value = serde_json::from_slice(
            result.strip_suffix(b"\n").expect("trailing newline"),
        )
        .expect("valid JSON");
        assert!(v["id"].is_null(), "id must be restored to null");
        assert!(
            !*active.lock().expect("lock"),
            "active flag must be cleared after restoration"
        );
    }

    #[test]
    fn outbound_normal_id_not_rewritten_when_inactive() {
        let active = make_active(false);
        let line = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n";
        let result = rewrite_null_id_outbound(line, &active);
        assert_eq!(result, line, "normal response must not be altered");
    }
}
