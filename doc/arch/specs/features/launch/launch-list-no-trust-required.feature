# ADR-0064 cross-ref: profile trust model — launch.list reads, never executes, so it bypasses the trust gate
Feature: launch.list enumerates services without requiring a blessed Profile
  As a small LLM inspecting a cloned repository
  I want to list the declared services without blessing the Profile
  So that I can see what a stack would run before deciding to trust it

  Scenario: launch.list on an unblessed Profile succeeds read-only
    Given a .substrate.toml with services db, api, and web and no bless record
    When launch.list is invoked
    Then the declared services db, api, and web are returned
    And no trust gate is applied and no process is spawned
    And the response hint suggests launch_up as the next tool
