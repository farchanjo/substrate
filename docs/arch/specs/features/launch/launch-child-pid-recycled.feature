# ADR-0068 cross-ref: the reaper start-epoch pin prevents signalling a recycled pid
Feature: a recycled child pid is never signalled by the reaper
  As an operator after a supervisor crash
  I want the reaper to skip a recorded child whose pid was recycled
  So that an innocent unrelated process is never killed

  Scenario: a start-time mismatch clears the entry and sends no signal
    Given a recorded child whose pid was recycled to an unrelated process with a different start-time
    When reaper-on-boot evaluates the recorded child
    Then the live start-time does not match the recorded start_epoch
    And no signal is sent and the stale entry is cleared
    And SUBSTRATE_LAUNCH_CHILD_PID_RECYCLED is recorded
