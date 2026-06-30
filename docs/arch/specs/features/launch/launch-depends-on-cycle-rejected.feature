# ADR-0065 cross-ref: depends_on must form a DAG; a cycle is rejected at validation time
Feature: a depends_on cycle is rejected before any Service is started
  As an operator authoring a Profile
  I want a dependency cycle to fail validation
  So that an unsatisfiable ordering never starts a partial Stack

  Scenario: a Profile with a dependency cycle returns CYCLE_DETECTED
    Given a Profile where service a depends_on b and service b depends_on a
    When launch.up validates the dependency graph
    Then the call returns SUBSTRATE_LAUNCH_CYCLE_DETECTED
    And no Service is spawned
