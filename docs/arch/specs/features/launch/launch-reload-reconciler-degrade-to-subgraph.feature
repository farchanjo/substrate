# ADR-0065 cross-ref: reconciler degrades to a subgraph down/up when no safe sequence exists
Feature: reload degrades to the affected subgraph when it cannot sequence safely
  As an operator editing a live stack topology
  I want an unsequenceable topology change to bounce only the affected subgraph
  So that unaffected services stay up and the whole stack is never aborted

  Scenario: an unsequenceable topology change cycles only the affected subgraph
    Given a running Stack and an edited Profile whose topology change cannot be safely sequenced
    When launch.reload is invoked
    Then only the affected subgraph is brought down and back up
    And unaffected services remain running
    And the per-service reload outcome reports the degradation
