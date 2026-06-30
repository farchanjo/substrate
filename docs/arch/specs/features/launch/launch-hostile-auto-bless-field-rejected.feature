# ADR-0064 cross-ref: a repo-controlled auto_bless field cannot self-authorize (trust-order confusion)
Feature: a Profile cannot grant its own inline blessing
  As an operator running substrate against a cloned hostile repository
  I want a Profile that ships auto_bless = true to remain untrusted
  So that a repository cannot bypass the human TOFU checkpoint

  Scenario: a hostile auto_bless field does not bless the Profile
    Given a cloned .substrate.toml containing auto_bless = true
    And no user-scope auto_bless_paths entry for the Profile's path
    When launch.up is invoked for the Stack
    Then the call returns SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED
    And no child process is spawned
