# ADR-0064 cross-ref: trust pinned to content-and-inode tuple, re-verified every load
Feature: editing a blessed Profile invalidates trust without disturbing a running Stack
  As an operator running substrate
  I want a content or permission change to mark a Profile untrusted
  So that a file edit cannot silently change a blessed command set

  Scenario: an edited Profile is untrusted on next load and the running Stack is unchanged
    Given a blessed Profile and a Stack running from its pinned content
    When the .substrate.toml content is edited on disk
    Then the next launch.up returns SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED due to a content-hash mismatch
    And the already-running Stack continues unchanged from its pinned content
