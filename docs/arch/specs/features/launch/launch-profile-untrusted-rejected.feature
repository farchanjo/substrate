# ADR-0064 cross-ref: profile trust model (TOFU) — untrusted Profile cannot execute
Feature: launch.up on an unblessed Profile is rejected before any spawn
  As an operator running substrate against a cloned repository
  I want an unblessed .substrate.toml to never spawn a process
  So that cloning a hostile repository is not arbitrary code execution

  Scenario: launch.up on an unblessed Profile returns PROFILE_NOT_TRUSTED
    Given a Profile referencing an allowlisted binary with no bless record in the user-scope trust store
    When launch.up is invoked for the Stack
    Then the call returns SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED
    And no child process is spawned
    And the recovery hint directs the operator to run launch.trust
