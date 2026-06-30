# ADR-0063 cross-ref: zero-orphan guarantee layer 1 — default shutdown disconnect policy
# ADR-0068 cross-ref: disconnect policy — shutdown drains and cascade-kills on client disconnect
Feature: the default disconnect policy leaves zero surviving processes
  As an operator running substrate
  I want a Stack to be torn down when the MCP client disconnects
  So that closing the agent never leaves stray processes on the host

  Scenario: client disconnect under the default policy kills the whole Stack
    Given a running Stack started with the default on_client_disconnect policy shutdown
    When the MCP client disconnects and the MCP server exits
    Then the supervisor cascade-kills every Service via killpg
    And the durable registry entry for the Stack is cleared
    And no supervised process remains running
