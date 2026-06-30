# ADR-0064 cross-ref: profile trust model — launch.trust blesses a Profile without running it
Feature: launch.trust blesses a Profile and pins its inode/content tuple
  As an operator who has reviewed a Profile
  I want to bless it once without starting the stack
  So that subsequent launch.up runs do not re-prompt

  Scenario: launch.trust records a bless tuple and suppresses the next prompt
    Given an unblessed .substrate.toml at a regular-file, owner-owned path
    When launch.trust is invoked for the Profile
    Then a bless record binding dev, ino, uid, mode, and content is written to the user-scope trust store
    And a subsequent launch.up passes the trust gate without elicitation
    And launch.trust itself spawns no process
