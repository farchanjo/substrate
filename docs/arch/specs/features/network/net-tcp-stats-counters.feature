# ADR-0058 cross-ref: network socket introspection bounded context
Feature: net.tcp_stats returns a valid kernel counter snapshot
  As an LLM agent using substrate
  I want to retrieve aggregate TCP statistics from the host kernel
  So that I can monitor connection health and detect anomalies such as
  elevated retransmissions or keepalive drops

  Scenario: net.tcp_stats returns non-negative counters with a valid timestamp
    Given the net.tcp_stats tool is available
    When net.tcp_stats is invoked with no parameters
    Then the result contains a TcpStats object
    And segs_in is greater than or equal to 0
    And segs_out is greater than or equal to 0
    And segs_retransmitted is greater than or equal to 0
    And segs_retransmitted is less than or equal to segs_in
    And rcv_packets is greater than or equal to 0
    And snd_packets is greater than or equal to 0
    And connections_initiated is greater than or equal to 0
    And connections_accepted is greater than or equal to 0
    And connections_established is greater than or equal to 0
    And connections_closed is greater than or equal to 0
    And persist_timer_drops is greater than or equal to 0
    And keepalive_drops is greater than or equal to 0
    And bad_checksums is greater than or equal to 0
    And captured_at parses as a valid RFC 3339 timestamp
