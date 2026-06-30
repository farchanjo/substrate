# ADR-0064 cross-ref: trust store and user config must be 0600 + owner with no world-writable parent
Feature: an insecure trust store is rejected at startup
  As an operator
  I want substrate to refuse a world-readable or non-owner trust store
  So that an attacker cannot forge or read trusted tuples

  Scenario: a mode-0644 trust store is rejected before any bless lookup
    Given the user-scope trust store exists at mode 0644
    When the launch trust store is loaded at startup
    Then startup fails with SUBSTRATE_LAUNCH_TRUST_STORE_INSECURE
    And no bless lookup or Profile load proceeds
