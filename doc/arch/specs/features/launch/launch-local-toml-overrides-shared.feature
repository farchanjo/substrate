# ADR-0064 cross-ref: shared vs local Profiles — both pass one TOFU gate; local overrides shared keys
Feature: a trusted .substrate.local.toml overrides matching service keys
  As an operator with a local override file
  I want the local Profile to win on conflicting keys once both are trusted
  So that per-developer settings take effect

  Scenario: local override wins when both files are blessed
    Given a blessed .substrate.toml and a blessed .substrate.local.toml that redefines the api service command
    When launch.up is invoked for the Stack
    Then the merged Profile uses the local api command
    And services declared only in the shared file are unchanged
