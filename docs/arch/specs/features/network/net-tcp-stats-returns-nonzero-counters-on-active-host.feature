@adr-0058 @platform-macos @platform-linux
Feature: net.tcp_stats — counters must reflect kernel TCP activity
  net.tcp_stats reads cumulative TCP counters from the platform kernel.
  On any host that has performed network I/O since boot, segs_in and
  segs_out are strictly positive. All-zero counters indicate the
  platform adapter is reading the wrong struct offset.

  Scenario: macOS adapter returns non-zero TCP counters on an active host
    Given the substrate-mcp-server is running on macOS
    And the host has completed at least one TCP handshake since boot
    When the client calls net.tcp_stats
    Then segs_in > 0
    And segs_out > 0

  Scenario: Linux adapter returns non-zero TCP counters on an active host
    Given the substrate-mcp-server is running on Linux
    And the host has completed at least one TCP handshake since boot
    When the client calls net.tcp_stats
    Then segs_in > 0
    And segs_out > 0
