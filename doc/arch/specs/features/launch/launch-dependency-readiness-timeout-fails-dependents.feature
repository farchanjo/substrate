# ADR-0065 cross-ref: a dependency that misses its readiness probe budget fails its dependents
Feature: a readiness timeout on a required dependency fails its dependents
  As an operator
  I want a required dependency that never becomes ready to fail its dependents explicitly
  So that the failure is observable rather than a silent hang

  Scenario: a required dependency readiness timeout fails the dependent
    Given service api depends_on db with required=true and db never reaches Ready within its probe budget
    When launch.up is invoked for the Stack
    Then api is not started and the call returns SUBSTRATE_LAUNCH_DEPENDENCY_FAILED
    And the error payload names db as the failed dependency
