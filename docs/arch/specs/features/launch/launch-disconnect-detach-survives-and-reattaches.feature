# ADR-0068 cross-ref: detach disconnect policy — Stack survives and a later session re-attaches
Feature: a detached Stack survives client disconnect and is re-attachable
  As an operator running a long-lived dev stack
  I want a detached Stack to survive closing the agent
  So that reopening the agent restores the running processes

  Scenario: a detached Stack survives the MCP server and re-attaches on reconnect
    Given a running Stack started with on_client_disconnect set to detach
    When the MCP client disconnects and the MCP server exits
    Then the detached supervisor keeps owning and supervising the children
    And a new MCP server reads the durable registry and re-attaches via launch.status
    And the restored Stack reports its running Services with an event replay
