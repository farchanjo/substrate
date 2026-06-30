# ADR-0068 cross-ref: zero-orphan layer 4 — reaper-on-boot reaps an orphan under shutdown policy
# ADR-0055 cross-ref: orphan reaper extended from temp files to processes
Feature: an orphaned child under a shutdown policy is reaped on boot
  As an operator running substrate
  I want a leftover orphaned process to be reaped at startup
  So that a prior crash does not leave the host dirty

  Scenario: reaper-on-boot reaps an orphaned child recorded under a shutdown policy
    Given a durable registry entry whose child is orphaned and whose policy is shutdown
    When a new MCP server runs its reaper-on-boot reconcile pass
    Then the orphaned child's process group is killed with killpg SIGTERM then SIGKILL
    And the registry entry is cleared and SUBSTRATE_LAUNCH_ORPHAN_REAPED is recorded
