Feature: archive.zip_extract blocks zip-slip path traversal before any write
  As a security boundary in substrate
  I want archive extraction to validate all member paths before writing any file
  So that a malicious archive cannot escape the extraction directory

  Background:
    Given an allowlist with root "/work/repo"
    And the extraction target directory is "/work/repo/extracted/"

  Scenario: Archive with traversal member "../evil.txt" is rejected before any write
    Given a zip archive containing a member with path "../evil.txt"
    When the client calls archive.zip_extract with archive="/work/repo/evil.zip" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no files are written to disk in "/work/repo/extracted/"
    And the file "/work/evil.txt" does not exist on disk

  Scenario: Archive with absolute path member "/etc/passwd" is rejected
    Given a zip archive containing a member with path "/etc/passwd"
    When the client calls archive.zip_extract with archive="/work/repo/evil.zip" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no files are written to disk in "/work/repo/extracted/"

  Scenario: Archive with nested traversal "a/../../outside.txt" is rejected
    Given a zip archive containing a member with path "a/../../outside.txt"
    When the client calls archive.zip_extract with archive="/work/repo/nested_slip.zip" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no files are written to disk in "/work/repo/extracted/"

  Scenario: Legitimate archive with all members inside target extracts successfully
    Given a zip archive where all member paths resolve inside "/work/repo/extracted/"
    When the client calls archive.zip_extract with archive="/work/repo/safe.zip" and dst="/work/repo/extracted/"
    Then all archive members are extracted into "/work/repo/extracted/"
    And no error is returned
