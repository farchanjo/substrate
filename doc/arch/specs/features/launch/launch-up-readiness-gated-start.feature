# ADR-0063 cross-ref: launch BC — launch.up brings a Stack up over the subprocess BC
# ADR-0065 cross-ref: dependency graph — start in topological order gated on readiness
Feature: launch.up starts Services in dependency order gated on readiness
  As an LLM agent using substrate
  I want a Stack to start its Services in dependency order
  So that each Service only starts once its dependencies report Ready

  Scenario: a three-Service Stack starts in topological order gated on Ready
    Given a trusted Profile with services db, api depends_on db, and web depends_on api
    When launch.up is invoked for the Stack
    Then db is started first and api waits until db reaches the Ready state
    And api is started next and web waits until api reaches the Ready state
    And the launch.up Task reports the Stack Running once every Service is Ready
