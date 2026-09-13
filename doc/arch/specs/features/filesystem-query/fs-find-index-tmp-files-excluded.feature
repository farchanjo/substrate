Feature: Transactional tmp files are never indexed by the fs-index feature
  As a correctness invariant of the transactional write pattern per ADR-0033
  I want files matching the .tmp.<uuid7> suffix to be excluded at walk time
  So that in-flight writes are never surfaced to fs.find clients

  Background:
    Given a running substrate server with the fs-index feature enabled
    And an allowlist with root "/work/repo"
    And the filesystem index has been built for "/work/repo"

  Scenario: An in-flight tmp file exists on disk but is filtered out at walk time
    Given an fs.write operation is in progress for target "/work/repo/output.rs"
    And the transactional tmp file "/work/repo/output.rs.tmp.<uuid7>" is present on disk
    When the client calls fs.find with root="/work/repo" and pattern="*.tmp.*"
    Then the result set does not contain any path matching the suffix ".tmp.<uuid7>"
    And the in-flight tmp file was excluded at index walk time and never inserted

  Scenario: After atomic rename the target path enters the index and the tmp file does not
    Given an fs.write operation completes and atomically renames the tmp file to "/work/repo/output.rs"
    When the client calls fs.find with root="/work/repo" and pattern="output.rs"
    Then the result set contains "/work/repo/output.rs"
    And the result set does not contain any path matching the suffix ".tmp.<uuid7>"
    And the entry for "/work/repo/output.rs" was added via write-through at commit time

  Scenario: If fs.write fails and the tmp file is cleaned up no orphan entry remains
    Given an fs.write operation fails after creating "/work/repo/output.rs.tmp.<uuid7>"
    And the cleanup handler removes the tmp file on failure
    When the client calls fs.find with root="/work/repo" and pattern="*.tmp.*"
    Then the result set does not contain any path matching the suffix ".tmp.<uuid7>"
    And no orphan index entry for the tmp file exists
