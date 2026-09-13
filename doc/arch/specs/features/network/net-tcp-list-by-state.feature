# ADR-0058 cross-ref: network socket introspection bounded context
Feature: net.tcp_list filtered by TCP state
  As an LLM agent using substrate
  I want to retrieve TCP socket entries filtered by connection state
  So that I can inspect only the sockets relevant to a given operational concern
  without processing the full socket table

  Scenario: state_filter Listen returns only listening sockets
    Given a host with at least one process bound to a listening TCP socket
    When net.tcp_list is invoked with state_filter ["Listen"]
    Then the result entries list is non-empty
    And every entry in entries has state equal to "Listen"
    And every entry in entries has local_port greater than 0
    And total equals the length of entries when pagination is absent

  Scenario: state_filter Established returns only established sockets with remote endpoints
    Given a host with at least one TCP connection in the Established state
    When net.tcp_list is invoked with state_filter ["Established"]
    Then every entry in entries has state equal to "Established"
    And every entry in entries has a non-empty remote_addr field
    And every entry in entries has remote_port greater than 0
    And every entry in entries has local_port greater than 0
