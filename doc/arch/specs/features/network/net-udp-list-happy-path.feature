# ADR-0058 cross-ref: network socket introspection bounded context — net.udp_list tool
Feature: net.udp_list returns UDP socket entries from the kernel
  As an LLM agent using substrate
  I want to retrieve the list of open UDP sockets from the host kernel
  So that I can inspect which processes are using UDP and on which ports

  Scenario: net.udp_list returns at least one entry on an active host
    Given the net.udp_list tool is available
    And the host has at least one open UDP socket
    When net.udp_list is invoked with no parameters
    Then the result entries list is non-empty
    And every entry in entries has local_port greater than 0
    And every entry in entries has local_addr formatted as an IPv4 or IPv6 textual address
    And total equals the length of entries when pagination is absent

  Scenario: net.udp_list entry fields conform to the SocketEntry schema
    Given the net.udp_list tool is available
    And the host has at least one open UDP socket
    When net.udp_list is invoked with no parameters
    Then every entry in entries has a non-empty local_addr field
    And every entry in entries has local_port greater than 0
    And every entry in entries has inode greater than or equal to 0

  Scenario: net.udp_list with resolve_pid=false omits the pid field
    Given the net.udp_list tool is available
    When net.udp_list is invoked with resolve_pid=false
    Then the result entries list is returned
    And no entry in entries has a pid field populated

  Scenario: net.udp_list on Linux returns SUBSTRATE_NOT_IMPLEMENTED
    Given the substrate-mcp-server is running on Linux
    When net.udp_list is invoked with no parameters
    Then the tool returns error code SUBSTRATE_NOT_IMPLEMENTED
