# ADR-0068 cross-ref: the reconcile sweep waitpid-reaps zombie supervised children
Feature: the reconcile sweep reaps a zombie supervised child
  As an operator
  I want a supervised child that exited but was not yet reaped to be cleaned up
  So that zombies do not accumulate under the supervisor

  Scenario: a zombie child in state Z is waitpid-reaped by the sweep
    Given a supervised child has exited and is in state Z (zombie)
    When the supervisor reconcile sweep runs
    Then the child is waitpid-reaped and removed from the registry
    And a hygiene event is emitted
