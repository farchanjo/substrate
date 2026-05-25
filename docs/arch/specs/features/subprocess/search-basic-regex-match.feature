# ADR-0057 cross-ref: subprocess output pagination and search
Feature: subprocess.search matches lines against a regex pattern
  As an LLM agent using substrate
  I want to search captured subprocess output with a regex pattern
  So that I can locate specific log lines without retrieving the full output

  Scenario: case-sensitive regex matches exact line prefix
    Given a subprocess wrote stdout lines "INFO foo", "ERROR bar", and "WARN baz" and has exited with Succeeded
    When subprocess.search is called with pattern "^ERROR" and case_insensitive false
    Then the response total_matches equals 1
    And matches[0].stream equals "stdout"
    And matches[0].line_number equals 2
    And matches[0].line_text equals "ERROR bar"

  Scenario: case-insensitive flag widens the match set
    Given a subprocess wrote stdout lines "INFO foo", "ERROR bar", and "WARN baz" and has exited with Succeeded
    When subprocess.search is called with pattern "error" and case_insensitive true
    Then the response total_matches equals 1
    And matches[0].stream equals "stdout"
    And matches[0].line_number equals 2
    And matches[0].line_text equals "ERROR bar"
