# ADR-0065 cross-ref: reconciler reload — spawn-time change restarts the closure, not the Stack
Feature: reloading a spawn-time change restarts only the affected Service and its cascade
  As an operator editing a running Stack's Profile
  I want a command change to restart the minimal set of Services
  So that unaffected Services keep running

  Scenario: changing api args restarts api and its dependent web but not db
    Given a running Stack with services db, api depends_on db, and web depends_on api
    When the args of api are changed and the Profile is reloaded
    Then api is restarted as an orchestrated restart not counted against its crash budget
    And web is restarted because it depends on api with the default cascade
    And db is not restarted
