# ADR-0064 cross-ref: world-writable Profile parent directory is rejected before hashing or spawn
Feature: a Profile in a world-writable directory is rejected
  As an operator
  I want substrate to refuse a .substrate.toml whose parent directory is world-writable
  So that a co-resident user cannot swap the Profile under me

  Scenario: a world-writable parent directory blocks launch.up before hashing
    Given a .substrate.toml whose containing directory has the world-write bit set
    When launch.up is invoked for the Stack
    Then the call returns SUBSTRATE_LAUNCH_CONFIG_UNTRUSTED_DIR
    And no content hash is computed and no process is spawned
