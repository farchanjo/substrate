@adr-0058 @platform-macos @platform-linux
Feature: net.tcp_list — Listen entries must expose a non-zero local port
  Every entry returned by net.tcp_list with state=LISTEN represents a server
  socket bound to a concrete address+port. A LISTEN entry whose local_port
  reads as zero indicates a parser layout bug in the platform adapter.

  Scenario: macOS sysctl adapter returns Listen entry with non-zero local port
    Given the substrate-mcp-server is running on macOS
    And at least one TCP server is listening on the host (e.g., SSH on port 22)
    When the client calls net.tcp_list with state_filter=["Listen"]
    Then every returned entry has state="Listen"
    And every returned entry has local_port > 0
    And every returned entry has local_addr formatted as an IPv4 or IPv6 textual address

  Scenario: Linux /proc/net adapter returns Listen entry with non-zero local port
    Given the substrate-mcp-server is running on Linux
    And at least one TCP server is listening on the host
    When the client calls net.tcp_list with state_filter=["Listen"]
    Then every returned entry has state="Listen"
    And every returned entry has local_port > 0
