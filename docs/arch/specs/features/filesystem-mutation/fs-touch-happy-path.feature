Feature: fs.touch creates or updates file timestamps within the allowlist
  As an LLM agent driving substrate
  I want to create an empty file or update timestamps
  So that I can signal file activity without altering content

  Background:
    Given an allowlist with root "/work/repo"

  Scenario: Touch a non-existent path creates an empty file
    Given the path "/work/repo/src/marker.txt" does not exist
    When the client calls fs.touch with path="/work/repo/src/marker.txt"
    Then the file "/work/repo/src/marker.txt" exists on disk
    And the file "/work/repo/src/marker.txt" has size 0 bytes
    And the tool returns a success result with the created path

  Scenario: Touch an existing file updates its modification timestamp
    Given the file "/work/repo/src/existing.txt" exists with an mtime in the past
    When the client calls fs.touch with path="/work/repo/src/existing.txt"
    Then the mtime of "/work/repo/src/existing.txt" is updated to approximately now
    And the content of "/work/repo/src/existing.txt" is unchanged
    And no error is returned

  Scenario: Touch preserves existing file content
    Given the file "/work/repo/src/data.txt" exists with content "hello"
    When the client calls fs.touch with path="/work/repo/src/data.txt"
    Then the content of "/work/repo/src/data.txt" is still "hello"

  Scenario: Touch with a path outside the allowlist returns SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    When the client calls fs.touch with path="/tmp/outside.txt"
    Then the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    And no file is created at "/tmp/outside.txt"

  Scenario: Touch when parent directory does not exist returns SUBSTRATE_NOT_FOUND
    Given the directory "/work/repo/nonexistent/" does not exist
    When the client calls fs.touch with path="/work/repo/nonexistent/file.txt"
    Then the tool returns error code SUBSTRATE_NOT_FOUND
