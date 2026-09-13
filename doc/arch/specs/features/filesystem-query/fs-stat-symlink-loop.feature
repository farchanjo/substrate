Feature: fs.find detects symlink loops and returns SUBSTRATE_SYMLINK_LOOP without hanging
  As an LLM agent driving substrate
  I want recursive directory traversal to detect and report symlink cycles
  So that the server never hangs or exhausts resources when a cyclic symlink is encountered

  Background:
    Given an allowlist with root "/work/repo"
    And the symlink "/work/repo/loop_a" exists and points to "/work/repo/loop_b"
    And the symlink "/work/repo/loop_b" exists and points to "/work/repo/loop_a"

  Scenario: fs.find under a directory containing a symlink loop returns SUBSTRATE_SYMLINK_LOOP
    When the client calls fs.find with root="/work/repo" and pattern="**/*"
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_SYMLINK_LOOP"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

  Scenario: fs.find returns the error within 5 seconds and does not hang
    When the client calls fs.find with root="/work/repo" and pattern="**/*"
    Then the server returns a response within 5 seconds
    And the error object has field "code" equal to "SUBSTRATE_SYMLINK_LOOP"

  Scenario: SUBSTRATE_SYMLINK_LOOP is not a crash — server continues accepting requests
    When the client calls fs.find with root="/work/repo" and pattern="**/*"
    Then the error object has field "code" equal to "SUBSTRATE_SYMLINK_LOOP"
    When the client subsequently calls fs.stat with path="/work/repo" (the root directory)
    Then the server returns a success response for the second call

  Scenario: The SUBSTRATE_SYMLINK_LOOP error details identify the cycle members
    When the client calls fs.find with root="/work/repo" and pattern="**/*"
    Then the error object has field "code" equal to "SUBSTRATE_SYMLINK_LOOP"
    And the error object details include at least one of "loop_a" or "loop_b" in the path information

  Scenario: Three-node symlink cycle is also detected
    Given the symlink "/work/repo/cycle_x" points to "/work/repo/cycle_y"
    And the symlink "/work/repo/cycle_y" points to "/work/repo/cycle_z"
    And the symlink "/work/repo/cycle_z" points to "/work/repo/cycle_x"
    When the client calls fs.find with root="/work/repo" and pattern="**/*"
    Then the error object has field "code" equal to "SUBSTRATE_SYMLINK_LOOP"
    And the server returns the error within 5 seconds
