# ADR-0068 cross-ref (2026-07-01 amendment): launch.down against a DETACHED
# stack routes through the control FIFO, with real confirmation, instead of
# a silent no-op over an empty in-session job map
Feature: launch.down against a detached stack routes through the control FIFO
  As an operator running a detached long-lived stack
  I want launch.down to reach the detached supervisor and be confirmed
  So that the stack is genuinely torn down instead of the request silently doing nothing

  Scenario: launch.down on a detached stack tears the stack down and is confirmed before returning
    Given a running Stack started with on_client_disconnect set to detach
    When launch.down is invoked for that stack_id
    Then a down command is sent to the detached supervisor over the control FIFO
    And launch.down does not report success until the durable registry confirms the stack is gone
    And launch.status reports the stack Down
