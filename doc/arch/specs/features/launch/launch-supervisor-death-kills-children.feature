# ADR-0068 cross-ref: zero-orphan layer 2 — parent-death binding kills children on supervisor death
# ADR-0053 cross-ref: PR_SET_PDEATHSIG (Linux) / WatchdogPipe (macOS) orphan prevention
Feature: a detached supervisor's death kills its children
  As an operator running substrate
  I want the kernel to kill a detached supervisor's children if the supervisor dies
  So that supervisor death cannot leave orphaned processes

  Scenario: SIGKILL of the detached supervisor takes the children down with it
    Given a detached Stack supervised by a detached supervisor with parent-death binding
    When the supervisor is killed with SIGKILL
    Then the kernel kills the children via PR_SET_PDEATHSIG on Linux or WatchdogPipe EOF on macOS
    And the next MCP server boot finds no surviving children in the registry
