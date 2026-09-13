# ADR-0058 cross-ref: network socket introspection bounded context
Feature: net.connection_count returns a consistent state histogram
  As an LLM agent using substrate
  I want to retrieve a histogram of TCP connection counts grouped by state
  So that I can quickly assess the overall connection health of the host
  without iterating over individual socket entries

  Scenario: net.connection_count total equals sum of by_state values
    Given the net.connection_count tool is available
    When net.connection_count is invoked with no parameters
    Then the result contains a ConnectionCounts object
    And total equals the arithmetic sum of all values in by_state
    And every key in by_state is one of the 12 TcpState variants
    And every value in by_state is greater than or equal to 0
    And total matches the count of currently open TCP sockets as reported by a simultaneous net.tcp_list call with no state_filter
    And captured_at parses as a valid RFC 3339 timestamp
