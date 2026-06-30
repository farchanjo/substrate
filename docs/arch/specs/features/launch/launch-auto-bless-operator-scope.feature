# ADR-0064 cross-ref: auto_bless lives in user-scope launch.toml (auto_bless_paths), never in the repo
Feature: inline auto-blessing is opt-in only through user-scope operator config
  As an operator who trusts a project directory
  I want launch.up to bless a new tuple inline for paths I listed in user scope
  So that I get convenience without the repository granting its own trust

  Scenario: a path in user-scope auto_bless_paths is blessed inline by launch.up
    Given the user-scope launch.toml lists the Profile's canonical path in auto_bless_paths
    And the Profile has no existing bless record
    When launch.up is invoked for the Stack
    Then launch.up blesses the new content and identity tuple inline and proceeds
    And the bless record is written to the user-scope trust store
