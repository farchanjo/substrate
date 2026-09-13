# ADR-0064 cross-ref: .substrate.local.toml is NOT trusted on presence (clone-and-run RCE class)
Feature: a committed .substrate.local.toml is not trusted on first load
  As an operator running substrate against a cloned repository
  I want a checked-in .substrate.local.toml to require the same TOFU bless
  So that a hostile author committing one is not arbitrary code execution

  Scenario: a freshly cloned .substrate.local.toml is rejected until blessed
    Given a freshly cloned repository containing a committed .substrate.local.toml with no bless record
    When launch.up is invoked for the Stack
    Then the call returns SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED
    And no child process is spawned
