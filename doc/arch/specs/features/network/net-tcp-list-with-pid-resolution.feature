# ADR-0058 cross-ref: network socket introspection bounded context
Feature: net.tcp_list PID resolution toggle
  As an LLM agent using substrate
  I want to control whether each socket entry includes the owning process ID
  So that I can trade latency for enriched data depending on my operational goal

  Scenario: resolve_pid false omits pid field and completes within latency budget
    Given the net.tcp_list tool is available
    When net.tcp_list is invoked with resolve_pid false
    Then every entry in the result entries list has a null or absent pid field
    And the tool response is received within 50 milliseconds

  Scenario: resolve_pid true populates pid for listening sockets
    Given a host with at least one process bound to a listening TCP socket
    When net.tcp_list is invoked with resolve_pid true and state_filter ["Listen"]
    Then at least one entry in the result entries list has a non-null pid field
    And for every entry with a populated pid field the pid value appears in the result of proc.list as a running process
