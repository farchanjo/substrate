# ADR-0063 amendment cross-ref: launch.forget removes a Down stack's registry
# entry without an MCP server restart (the registry is process-lifetime-only
# for shutdown-policy Stacks; only detach Stacks get a durable supervisor.json)
Feature: launch.forget removes a terminal stack's registry entry
  As an operator running many short-lived stacks
  I want to clear a stack that has already been brought down
  So that launch.status stops listing it without restarting the MCP server

  Scenario: forgetting a Down stack removes it from status
    Given a running Stack started with the default on_client_disconnect policy shutdown
    And the Stack has been brought down via launch.down
    When launch.forget is invoked for that stack_id
    Then the forget call succeeds
    And launch.status no longer lists that stack_id

  Scenario: forgetting a non-terminal stack is rejected
    Given a running Stack started with the default on_client_disconnect policy shutdown
    When launch.forget is invoked for that stack_id
    Then the forget call fails with SUBSTRATE_LAUNCH_STACK_NOT_TERMINAL
    And launch.status still lists that stack_id
