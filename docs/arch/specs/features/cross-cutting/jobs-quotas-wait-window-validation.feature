# ADR-0059
Feature: Boot-time guard rejects invalid jobs.quotas wait-window configuration
  As an operator deploying substrate
  I want the server to abort startup with a precise error when the wait-window quotas are logically inconsistent
  So that runtime polling guarantees are structurally enforced at boot rather than discovered at call time

  Background:
    Given substrate is configured with a config file at "/etc/substrate/config.toml"

  Scenario: Valid wait-window configuration allows successful startup
    Given the config file sets jobs.quotas.result_default_wait_ms to 5000
    And the config file sets jobs.quotas.result_max_wait_ms to 30000
    When substrate starts
    Then the process does not exit immediately with a non-zero code
    And no SUBSTRATE_CONFIG_INVALID error is emitted

  Scenario: result_default_wait_ms set to zero causes boot abort with SUBSTRATE_CONFIG_INVALID
    Given the config file sets jobs.quotas.result_default_wait_ms to 0
    And the config file sets jobs.quotas.result_max_wait_ms to 30000
    When substrate starts
    Then the process exits with code 78
    And exactly one JSON line is written to stderr
    And that JSON line has field "code" equal to "SUBSTRATE_CONFIG_INVALID"
    And that JSON line details include field "offending_field" mentioning "jobs.quotas"
    And that JSON line has field "recovery_hint" whose length is between 1 and 150 characters
    And no bytes are written to stdout

  Scenario: result_default_wait_ms exceeding result_max_wait_ms causes boot abort with SUBSTRATE_CONFIG_INVALID
    Given the config file sets jobs.quotas.result_default_wait_ms to 5000
    And the config file sets jobs.quotas.result_max_wait_ms to 1000
    When substrate starts
    Then the process exits with code 78
    And exactly one JSON line is written to stderr
    And that JSON line has field "code" equal to "SUBSTRATE_CONFIG_INVALID"
    And that JSON line details include field "offending_field" mentioning "jobs.quotas"
    And that JSON line has field "recovery_hint" whose length is between 1 and 150 characters
    And no bytes are written to stdout
