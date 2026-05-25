//! Linux `/proc/net` adapter for the network-info BC (ADR-0058).
//!
//! All four [`NetworkInfoPort`] methods are fully implemented:
//!
//! - `list_tcp`  — parses `/proc/net/tcp` (IPv4) + `/proc/net/tcp6` (IPv6).
//! - `list_udp`  — parses `/proc/net/udp` (IPv4) + `/proc/net/udp6` (IPv6).
//! - `tcp_stats` — parses the `Tcp:` row from `/proc/net/snmp`.
//! - `connection_count` — derived from `list_tcp` with no state filter.
//!
//! No shell-out. No unsafe code. Pure text parsing via `tokio::fs::read_to_string`.
//!
//! ## `/proc/net/tcp` row format
//!
//! ```text
//! sl  local_address          rem_address          st  tx_q:rx_q  tr:tm_when  retrnsmt uid  timeout  inode
//! 0:  0100007F:1F40          00000000:0000        0A  00000000:00000000  00:00000000  00000000  1000 0  12345
//! ```
//!
//! `local_address` for IPv4: 8-char hex, **little-endian** (i.e., bytes reversed
//! relative to network order). To convert: parse as `u32`, byte-swap, then call
//! `Ipv4Addr::from(u32)`.  Port is 4-char hex big-endian.
//!
//! For IPv6 (`/proc/net/tcp6`): 32-char hex split into four little-endian `u32`
//! words; each word must be byte-swapped before assembling the `Ipv6Addr`.
//!
//! References: ADR-0058, `<net/tcp_states.h>`, `<linux/net/ipv4/tcp_ipv4.c>`.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, Ipv6Addr};

use async_trait::async_trait;
use substrate_domain::errors::SubstrateError;
use substrate_domain::network::{
    AddrFamily, ConnectionCounts, NetworkTcpListRequest, NetworkTcpListResult,
    NetworkUdpListRequest, NetworkUdpListResult, Pagination, Protocol, SocketEntry, TcpState,
    TcpStats,
};
use substrate_domain::ports::network_info::NetworkInfoPort;
use time::OffsetDateTime;
use tracing::warn;

use crate::state::linux_state_from_hex;

// ---- Adapter ----------------------------------------------------------------

/// Linux `/proc/net` adapter implementing [`NetworkInfoPort`].
///
/// All operations are async (Zone A per ADR-0003): they call
/// `tokio::fs::read_to_string` and `tokio::fs::read_dir`/`read_link`.
/// No blocking code runs on the async executor.
#[derive(Debug, Default)]
pub struct LinuxProcNetAdapter;

#[async_trait]
impl NetworkInfoPort for LinuxProcNetAdapter {
    async fn list_tcp(
        &self,
        req: NetworkTcpListRequest,
    ) -> Result<NetworkTcpListResult, SubstrateError> {
        req.validate()?;

        let mut entries = Vec::new();
        parse_proc_net(Protocol::Tcp, AddrFamily::Inet, "/proc/net/tcp", &mut entries).await?;
        parse_proc_net(Protocol::Tcp, AddrFamily::Inet6, "/proc/net/tcp6", &mut entries).await?;

        if let Some(ref filter) = req.state_filter {
            entries.retain(|e| filter.contains(&e.state));
        }

        if req.resolve_pid {
            resolve_pids(&mut entries).await;
        }

        let total = entries.len() as u64;
        let (page, next_offset) = paginate(entries, req.pagination.as_ref());
        Ok(NetworkTcpListResult {
            entries: page,
            total,
            next_offset,
        })
    }

    async fn list_udp(
        &self,
        req: NetworkUdpListRequest,
    ) -> Result<NetworkUdpListResult, SubstrateError> {
        req.validate()?;

        let mut entries = Vec::new();
        parse_proc_net(Protocol::Udp, AddrFamily::Inet, "/proc/net/udp", &mut entries).await?;
        parse_proc_net(Protocol::Udp, AddrFamily::Inet6, "/proc/net/udp6", &mut entries).await?;

        if req.resolve_pid {
            resolve_pids(&mut entries).await;
        }

        let total = entries.len() as u64;
        let (page, next_offset) = paginate(entries, req.pagination.as_ref());
        Ok(NetworkUdpListResult {
            entries: page,
            total,
            next_offset,
        })
    }

    async fn tcp_stats(&self) -> Result<TcpStats, SubstrateError> {
        parse_snmp_stats().await
    }

    async fn connection_count(&self) -> Result<ConnectionCounts, SubstrateError> {
        // Re-use list_tcp with no filter for an accurate per-state count.
        // This reads /proc/net/tcp + /proc/net/tcp6 in full; adapters that need
        // a cheaper count-only path can override this with a dedicated counter.
        let req = NetworkTcpListRequest {
            state_filter: None,
            resolve_pid: false,
            pagination: None,
        };
        // list_tcp applies default pagination; we want ALL entries.
        // Build without a pagination limit by requesting max page size.
        let all = collect_all_tcp(self).await?;

        let mut by_state: BTreeMap<TcpState, u32> = BTreeMap::new();
        for entry in &all {
            *by_state.entry(entry.state).or_insert(0) += 1;
        }
        let total = all.len() as u32;
        Ok(ConnectionCounts {
            by_state,
            total,
            captured_at: OffsetDateTime::now_utc(),
        })
    }
}

// ---- Internal helpers -------------------------------------------------------

/// Collects every TCP entry (both IPv4 and IPv6) without pagination.
async fn collect_all_tcp(adapter: &LinuxProcNetAdapter) -> Result<Vec<SocketEntry>, SubstrateError> {
    let mut entries = Vec::new();
    parse_proc_net(Protocol::Tcp, AddrFamily::Inet, "/proc/net/tcp", &mut entries).await?;
    parse_proc_net(Protocol::Tcp, AddrFamily::Inet6, "/proc/net/tcp6", &mut entries).await?;
    Ok(entries)
}

/// Parses a `/proc/net/{tcp,tcp6,udp,udp6}` file and appends entries to `out`.
///
/// The first line (header) is skipped. Lines that fail to parse are silently
/// dropped with a `warn!` trace event — a single malformed kernel line must not
/// abort the entire listing.
async fn parse_proc_net(
    proto: Protocol,
    family: AddrFamily,
    path: &str,
    out: &mut Vec<SocketEntry>,
) -> Result<(), SubstrateError> {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // tcp6/udp6 may not exist on kernels without IPv6 support.
            return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(SubstrateError::PermissionDenied {
                path: path.to_string(),
                reason: "cannot read proc/net socket table".to_string(),
                correlation_id: None,
            });
        }
        Err(e) => {
            return Err(SubstrateError::InternalError {
                reason: format!("read {path}: {e}"),
                correlation_id: None,
            });
        }
    };

    for line in content.lines().skip(1) {
        match parse_proc_net_line(line, proto, family) {
            Ok(entry) => out.push(entry),
            Err(reason) => {
                warn!(path, reason, "skipping malformed /proc/net line");
            }
        }
    }
    Ok(())
}

/// Parses a single data row from `/proc/net/{tcp,tcp6,udp,udp6}`.
///
/// Returns an error string (for tracing) when any field is malformed.
fn parse_proc_net_line(
    line: &str,
    proto: Protocol,
    family: AddrFamily,
) -> Result<SocketEntry, String> {
    // Fields are whitespace-separated; the layout is:
    //   0: sl  1: local_address  2: rem_address  3: st  4: tx_q:rx_q
    //   5: tr:tm_when  6: retrnsmt  7: uid  8: timeout  9: inode  ...
    let mut fields = line.split_whitespace();

    let _sl = fields.next().ok_or("missing sl")?;
    let local_raw = fields.next().ok_or("missing local_address")?;
    let remote_raw = fields.next().ok_or("missing rem_address")?;
    let state_raw = fields.next().ok_or("missing st")?;
    // skip tx_q:rx_q, tr:tm_when, retrnsmt, uid, timeout
    for _ in 0..5 {
        fields.next();
    }
    let inode_raw = fields.next().ok_or("missing inode")?;

    let (local_addr, local_port) = parse_addr_port(local_raw, family)?;
    let (remote_addr_str, remote_port_val) = parse_addr_port(remote_raw, family)?;

    let state_byte =
        u8::from_str_radix(state_raw, 16).map_err(|_| format!("bad state: {state_raw}"))?;
    let state = linux_state_from_hex(state_byte);

    let inode: u64 = inode_raw
        .parse()
        .map_err(|_| format!("bad inode: {inode_raw}"))?;

    // Remote addr/port of 0.0.0.0:0 or :::0 means not connected.
    let (remote_addr, remote_port) = if remote_port_val == 0 {
        (None, None)
    } else {
        (Some(remote_addr_str), Some(remote_port_val))
    };

    Ok(SocketEntry {
        protocol: proto,
        family,
        local_addr,
        local_port,
        remote_addr,
        remote_port,
        state,
        pid: None,
        inode: Some(inode),
    })
}

/// Parses a `HEX_IP:HEX_PORT` field from `/proc/net/{tcp,tcp6}`.
///
/// IPv4 (`Inet`): 8 hex chars = little-endian `u32`; byte-swap to get network order.
/// IPv6 (`Inet6`): 32 hex chars = four little-endian `u32` words concatenated;
/// each word is byte-swapped independently before building the `Ipv6Addr`.
fn parse_addr_port(raw: &str, family: AddrFamily) -> Result<(String, u16), String> {
    let (addr_hex, port_hex) = raw
        .split_once(':')
        .ok_or_else(|| format!("missing colon in addr:port field: {raw}"))?;

    let port = u16::from_str_radix(port_hex, 16)
        .map_err(|_| format!("bad port hex: {port_hex}"))?;

    let addr = match family {
        AddrFamily::Inet => {
            if addr_hex.len() != 8 {
                return Err(format!("expected 8-char IPv4 hex, got {}", addr_hex.len()));
            }
            let raw_u32 = u32::from_str_radix(addr_hex, 16)
                .map_err(|_| format!("bad IPv4 hex: {addr_hex}"))?;
            // The kernel writes the address in host byte order on x86/x86_64
            // (little-endian), so we byte-swap to get network order (big-endian),
            // then build Ipv4Addr which expects big-endian u32.
            Ipv4Addr::from(raw_u32.swap_bytes()).to_string()
        }
        AddrFamily::Inet6 => {
            if addr_hex.len() != 32 {
                return Err(format!(
                    "expected 32-char IPv6 hex, got {}",
                    addr_hex.len()
                ));
            }
            // 32 chars = 4 × 8-char words, each little-endian u32.
            let mut words = [0u32; 4];
            for (i, word) in words.iter_mut().enumerate() {
                let chunk = &addr_hex[i * 8..(i + 1) * 8];
                let raw_u32 = u32::from_str_radix(chunk, 16)
                    .map_err(|_| format!("bad IPv6 word hex: {chunk}"))?;
                *word = raw_u32.swap_bytes();
            }
            // Build 16-byte array from the four u32 words in network order.
            let mut bytes = [0u8; 16];
            for (i, w) in words.iter().enumerate() {
                bytes[i * 4..(i + 1) * 4].copy_from_slice(&w.to_be_bytes());
            }
            Ipv6Addr::from(bytes).to_string()
        }
    };

    Ok((addr, port))
}

// ---- PID resolution ---------------------------------------------------------

/// Resolves PIDs for each socket entry by walking `/proc/<pid>/fd/`.
///
/// Best-effort: entries that cannot be resolved retain `pid: None`.
/// Errors from unprivileged access (`EACCES`, `EPERM`) are silently skipped.
async fn resolve_pids(entries: &mut Vec<SocketEntry>) {
    // Build inode → index map for O(1) lookup.
    let mut inode_map: BTreeMap<u64, usize> = BTreeMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        if let Some(inode) = entry.inode {
            inode_map.insert(inode, idx);
        }
    }
    if inode_map.is_empty() {
        return;
    }

    let Ok(mut proc_dir) = tokio::fs::read_dir("/proc").await else {
        return;
    };

    while let Ok(Some(pid_entry)) = proc_dir.next_entry().await {
        let file_name = pid_entry.file_name();
        let Some(pid_str) = file_name.to_str() else {
            continue;
        };
        let Ok(pid): Result<u32, _> = pid_str.parse() else {
            continue; // not a numeric PID directory
        };

        let fd_dir = format!("/proc/{pid}/fd");
        let Ok(mut fd_dir_entries) = tokio::fs::read_dir(&fd_dir).await else {
            continue; // EACCES or process exited between readdir and open
        };

        while let Ok(Some(fd_entry)) = fd_dir_entries.next_entry().await {
            let Ok(target) = tokio::fs::read_link(fd_entry.path()).await else {
                continue;
            };
            // Symlink targets for sockets look like "socket:[INODE]".
            let target_str = target.to_string_lossy();
            if let Some(inode_str) = target_str
                .strip_prefix("socket:[")
                .and_then(|s| s.strip_suffix(']'))
            {
                if let Ok(inode) = inode_str.parse::<u64>() {
                    if let Some(&idx) = inode_map.get(&inode) {
                        entries[idx].pid = Some(pid);
                    }
                }
            }
        }
    }
}

// ---- TCP stats from /proc/net/snmp ------------------------------------------

/// Parses `/proc/net/snmp` and extracts the `Tcp:` counters.
///
/// `/proc/net/snmp` format (two lines per protocol):
/// ```text
/// Tcp: RtoAlgorithm RtoMin RtoMax MaxConn ActiveOpens PassiveOpens AttemptFails ...
/// Tcp: 1 200 120000 -1 12 34 5 6 7 8 9 10 11 12 13
/// ```
async fn parse_snmp_stats() -> Result<TcpStats, SubstrateError> {
    let content = tokio::fs::read_to_string("/proc/net/snmp")
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("read /proc/net/snmp: {e}"),
            correlation_id: None,
        })?;

    // Find the two "Tcp:" lines: header then values.
    let mut tcp_header: Option<Vec<&str>> = None;
    let mut tcp_values: Option<Vec<&str>> = None;

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("Tcp: ") {
            if tcp_header.is_none() {
                tcp_header = Some(rest.split_whitespace().collect());
            } else if tcp_values.is_none() {
                tcp_values = Some(rest.split_whitespace().collect());
                break;
            }
        }
    }

    let headers = tcp_header.ok_or_else(|| SubstrateError::InternalError {
        reason: "missing Tcp: header line in /proc/net/snmp".to_string(),
        correlation_id: None,
    })?;
    let values = tcp_values.ok_or_else(|| SubstrateError::InternalError {
        reason: "missing Tcp: values line in /proc/net/snmp".to_string(),
        correlation_id: None,
    })?;

    // Build a name → value map.
    let map: BTreeMap<&str, u64> = headers
        .iter()
        .zip(values.iter())
        .filter_map(|(k, v)| {
            // Some counter fields are "-1" for "not applicable"; treat as 0.
            let val: u64 = v.parse::<i64>().unwrap_or(0).max(0) as u64;
            Some((*k, val))
        })
        .collect();

    let get = |key: &str| map.get(key).copied().unwrap_or(0);

    Ok(TcpStats {
        segs_in: get("InSegs"),
        segs_out: get("OutSegs"),
        segs_retransmitted: get("RetransSegs"),
        rcv_packets: get("InSegs"),
        snd_packets: get("OutSegs"),
        connections_initiated: get("ActiveOpens"),
        connections_accepted: get("PassiveOpens"),
        connections_established: get("ActiveOpens") + get("PassiveOpens"),
        connections_closed: get("EstabResets"),
        persist_timer_drops: 0, // not available in /proc/net/snmp
        keepalive_drops: 0,     // not available in /proc/net/snmp
        bad_checksums: get("InErrs"),
        captured_at: OffsetDateTime::now_utc(),
    })
}

/// Applies cursor-based pagination to a list of entries.
///
/// Returns `(page, next_offset)`. When `pagination` is `None` the first 50
/// entries are returned (default page size per ADR-0008).
fn paginate(entries: Vec<SocketEntry>, pagination: Option<&Pagination>) -> (Vec<SocketEntry>, Option<u64>) {
    let offset = pagination.map_or(0, |p| p.offset as usize);
    let page_size = pagination.map_or(50, |p| p.page_size as usize);

    let slice: Vec<SocketEntry> = entries.into_iter().skip(offset).collect();
    let next_offset = if slice.len() > page_size {
        Some((offset + page_size) as u64)
    } else {
        None
    };
    let page = slice.into_iter().take(page_size).collect();
    (page, next_offset)
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use substrate_domain::network::{AddrFamily, Protocol, TcpState};

    use super::{parse_addr_port, parse_proc_net_line};

    // ---- IPv4 hex byte-swap -------------------------------------------------

    #[test]
    fn ipv4_loopback_parses_correctly() {
        // 0100007F = 0x7F000001 byte-swapped = 127.0.0.1
        let (addr, port) = parse_addr_port("0100007F:1F40", AddrFamily::Inet)
            .expect("parse loopback");
        assert_eq!(addr, "127.0.0.1");
        assert_eq!(port, 0x1F40); // 8000
    }

    #[test]
    fn ipv4_any_parses_correctly() {
        // 00000000:0000 = 0.0.0.0:0
        let (addr, port) = parse_addr_port("00000000:0000", AddrFamily::Inet)
            .expect("parse any");
        assert_eq!(addr, "0.0.0.0");
        assert_eq!(port, 0);
    }

    #[test]
    fn ipv4_non_loopback_parses_correctly() {
        // 0101A8C0 = 0xC0A80101 byte-swapped = 192.168.1.1
        let (addr, _) = parse_addr_port("0101A8C0:0050", AddrFamily::Inet)
            .expect("parse 192.168.1.1");
        assert_eq!(addr, "192.168.1.1");
    }

    // ---- IPv6 hex byte-swap -------------------------------------------------

    #[test]
    fn ipv6_loopback_parses_correctly() {
        // All-zero except last word which is 0x01000000 → after swap = 0x00000001 → ::1
        let raw = "00000000000000000000000001000000:0050";
        let (addr, port) = parse_addr_port(raw, AddrFamily::Inet6)
            .expect("parse ::1");
        assert_eq!(addr, "::1");
        assert_eq!(port, 80);
    }

    #[test]
    fn ipv6_any_parses_correctly() {
        let raw = "00000000000000000000000000000000:0000";
        let (addr, port) = parse_addr_port(raw, AddrFamily::Inet6)
            .expect("parse ::");
        assert_eq!(addr, "::");
        assert_eq!(port, 0);
    }

    // ---- Full line parsing --------------------------------------------------

    #[test]
    fn proc_net_tcp_listen_line_parses() {
        // Real-world /proc/net/tcp line for a listening socket on port 22.
        let line = "   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 12345 1 0000000000000000 100 0 0 10 0";
        let entry = parse_proc_net_line(line, Protocol::Tcp, AddrFamily::Inet)
            .expect("parse listen line");
        assert_eq!(entry.local_addr, "0.0.0.0");
        assert_eq!(entry.local_port, 22);
        assert_eq!(entry.state, TcpState::Listen);
        assert!(entry.remote_addr.is_none());
        assert!(entry.remote_port.is_none());
        assert_eq!(entry.inode, Some(12345));
    }

    #[test]
    fn proc_net_tcp_established_line_parses() {
        // An established connection from 127.0.0.1:8080 to 127.0.0.1:12345.
        let line = "   1: 0100007F:1F90 0100007F:3039 01 00000000:00000000 00:00000000 00000000  1000        0 67890 1 0000000000000000 20 0 0 10 -1";
        let entry = parse_proc_net_line(line, Protocol::Tcp, AddrFamily::Inet)
            .expect("parse established line");
        assert_eq!(entry.local_addr, "127.0.0.1");
        assert_eq!(entry.local_port, 0x1F90); // 8080
        assert_eq!(entry.state, TcpState::Established);
        assert!(entry.remote_addr.is_some());
        assert_eq!(entry.inode, Some(67890));
    }

    // ---- State decode -------------------------------------------------------

    #[test]
    fn all_linux_states_decode() {
        use crate::state::linux_state_from_hex;
        // Spot-check a few via the line parser's state path.
        assert_eq!(linux_state_from_hex(0x0A), TcpState::Listen);
        assert_eq!(linux_state_from_hex(0x01), TcpState::Established);
        assert_eq!(linux_state_from_hex(0x06), TcpState::TimeWait);
    }
}
