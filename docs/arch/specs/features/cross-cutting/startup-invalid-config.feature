Feature: substrate exits with code 78 when the configuration file contains invalid TOML
  As an operator deploying substrate
  I want a precise parse error reported on startup for malformed configuration
  So that the offending line can be identified and corrected without guesswork

  Background:
    Given substrate is configured with a config file at "/etc/substrate/config.toml"

  Scenario: Config with an unterminated string literal causes exit code 78
    Given the config file contains the TOML fragment 'name = "unterminated'
    When substrate starts
    Then the process exits with code 78
    And exactly one JSON line is written to stderr
    And that JSON line has field "code" equal to "SUBSTRATE_CONFIG_INVALID"
    And that JSON line has field "recovery_hint" whose length is between 1 and 150 characters
    And that JSON line has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And that JSON line details include field "parse_error_line" with a positive integer value

  Scenario: Config with a duplicate key causes exit code 78
    Given the config file contains duplicate key "allowlist_root" on two separate lines
    When substrate starts
    Then the process exits with code 78
    And the stderr JSON line has field "code" equal to "SUBSTRATE_CONFIG_INVALID"
    And the stderr JSON line details include field "parse_error_line" with a positive integer value

  Scenario: Config with unknown fields in strict mode causes exit code 78
    Given substrate is configured with strict_config=true
    And the config file contains an unrecognized key "bogus_option = true"
    When substrate starts
    Then the process exits with code 78
    And the stderr JSON line has field "code" equal to "SUBSTRATE_CONFIG_INVALID"
    And the stderr JSON line details include field "offending_field" equal to "bogus_option"

  Scenario: Valid config file results in successful startup
    Given the config file is a syntactically valid TOML with all required fields present
    When substrate starts
    Then the process does not exit immediately with a non-zero code
    And no SUBSTRATE_CONFIG_INVALID error is emitted

  Scenario: No stdout output is produced on config parse failure
    Given the config file contains the TOML fragment 'name = "unterminated'
    When substrate starts
    Then the process exits with code 78
    And no bytes are written to stdout
