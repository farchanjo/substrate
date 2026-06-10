Feature: text.count_lines returns line count and byte count for a file
  As an LLM agent driving substrate
  I want to count the lines and bytes in a file without reading the entire content
  So that I can decide whether to invoke text.search or text.head before loading large files

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/src/main.rs" contains exactly 120 lines and 3840 bytes

  Scenario: text.count_lines returns line_count and byte_count for a regular file
    When the client calls text.count_lines with path="/work/repo/src/main.rs"
    Then the structured content contains a line_count field equal to 120
    And the structured content contains a byte_count field equal to 3840
    And no error is returned

  Scenario: text.count_lines result is consistent with byte length of content
    Given the file "/work/repo/src/lib.rs" contains "hello\nworld\n"
    When the client calls text.count_lines with path="/work/repo/src/lib.rs"
    Then the structured content contains a line_count field equal to 2
    And the structured content contains a byte_count field equal to 12

  Scenario: text.count_lines returns SUBSTRATE_NOT_FOUND for a missing file
    Given no file exists at "/work/repo/missing.txt"
    When the client calls text.count_lines with path="/work/repo/missing.txt"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters

  Scenario: text.count_lines returns SUBSTRATE_PATH_OUTSIDE_JAIL for a path outside the allowlist
    When the client calls text.count_lines with path="/etc/passwd"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_PATH_OUTSIDE_JAIL"

  Scenario: text.count_lines on an empty file returns zero for both counters
    Given the file "/work/repo/empty.txt" contains no bytes
    When the client calls text.count_lines with path="/work/repo/empty.txt"
    Then the structured content contains a line_count field equal to 0
    And the structured content contains a byte_count field equal to 0
    And no error is returned
