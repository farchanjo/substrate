# ADR-0068 cross-ref: reaper-on-boot adopts an orphan under detach policy instead of reaping
Feature: an orphaned child under a detach policy is adopted on boot
  As an operator running a long-lived detached Stack
  I want a surviving orphaned child to be re-adopted rather than killed
  So that a supervisor restart restores supervision without losing the process

  Scenario: reaper-on-boot adopts an orphaned child recorded under a detach policy
    Given a durable registry entry whose child is orphaned and whose policy is detach
    When a new MCP server runs its reaper-on-boot reconcile pass
    Then a supervisor re-establishes ownership of the child tracked by its process group
    And SUBSTRATE_LAUNCH_ORPHAN_ADOPTED is recorded and the child appears in launch.status
