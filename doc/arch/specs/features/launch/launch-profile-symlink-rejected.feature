# ADR-0064 cross-ref: trust pipeline — config opened O_NOFOLLOW, symlink rejected first
# ADR-0035 cross-ref: path-safety hardening — symlink-safe open
Feature: a symlinked .substrate.toml is rejected before any hash is computed
  As an operator running substrate
  I want the Profile open to refuse a symlink
  So that a symlink cannot redirect trust to attacker-controlled content

  Scenario: launch.up on a symlinked config returns CONFIG_SYMLINK_REJECTED
    Given .substrate.toml is a symlink to another file
    When launch.up opens the config with O_NOFOLLOW
    Then the open fails with ELOOP
    And the call returns SUBSTRATE_LAUNCH_CONFIG_SYMLINK_REJECTED before any content hash is computed
