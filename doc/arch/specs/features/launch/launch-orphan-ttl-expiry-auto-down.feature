# ADR-0068 cross-ref: zero-orphan layer 3 — orphan TTL auto-downs a clientless detached Stack
Feature: a detached Stack with no client is auto-brought-down after the orphan TTL
  As an operator running substrate
  I want a forgotten detached Stack to stop itself after a bound
  So that nothing runs indefinitely with no client watching it

  Scenario: a clientless detached Stack expires its orphan TTL and is brought down
    Given a detached Stack with orphan_ttl_secs set to a short bound and no client attached
    When the orphan TTL elapses with no client re-attachment
    Then the supervisor brings the Stack down and clears its registry entry
    And SUBSTRATE_LAUNCH_STACK_TTL_EXPIRED is recorded
