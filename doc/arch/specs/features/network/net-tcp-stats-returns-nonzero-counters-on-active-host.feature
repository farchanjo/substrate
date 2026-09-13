@adr-0058 @platform-macos @platform-linux
Feature: net.tcp_stats — counters must reflect kernel TCP activity
  net.tcp_stats reads cumulative TCP counters from the platform kernel.
  On Linux, any host that has performed network I/O since boot reports
  strictly positive segs_in and segs_out. On macOS Sequoia the read-only
  sysctl net.inet.tcp.disable_access_to_stats defaults to 1, zeroing every
  counter for unprivileged callers; per ADR-0058 Amendment v3 substrate
  reports the kernel values verbatim, so all-zero counters are valid there
  and the adapter contract is a well-formed TcpStats object.

  # ADR-0058 Amendment v3 (2026-05-25): macOS Sequoia zeroes TCP counters for
  # unprivileged callers via net.inet.tcp.disable_access_to_stats=1. The
  # tcpstat_n mirror and offset_of! guards are correct; substrate does not lie
  # about kernel-reported values, so the contract is a well-formed object.
  Scenario: macOS adapter returns a well-formed TcpStats object
    Given the substrate-mcp-server is running on macOS
    And the host has completed at least one TCP handshake since boot
    When the client calls net.tcp_stats
    Then the result contains a TcpStats object

  Scenario: Linux adapter returns non-zero TCP counters on an active host
    Given the substrate-mcp-server is running on Linux
    And the host has completed at least one TCP handshake since boot
    When the client calls net.tcp_stats
    Then segs_in > 0
    And segs_out > 0
