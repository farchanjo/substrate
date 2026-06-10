Feature: archive.tar.extract blocks tar path-traversal members before any write
  As a security boundary in substrate
  I want tar archive extraction to validate all member paths before writing any file
  So that a malicious archive cannot escape the extraction directory

  Background:
    Given an allowlist with root "/work/repo"
    And the extraction target directory is "/work/repo/extracted/"

  Scenario: Tar with traversal member "../evil.txt" is rejected before any write
    Given a tar archive containing a member with path "../evil.txt"
    When the client calls archive.tar.extract with archive="/work/repo/evil.tar" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no files are written to disk in "/work/repo/extracted/"
    And the file "/work/evil.txt" does not exist on disk

  Scenario: Tar with absolute path member "/etc/passwd" is rejected
    Given a tar archive containing a member with path "/etc/passwd"
    When the client calls archive.tar.extract with archive="/work/repo/evil.tar" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no files are written to disk in "/work/repo/extracted/"

  Scenario: Tar with nested traversal "a/../../outside.txt" is rejected
    Given a tar archive containing a member with path "a/../../outside.txt"
    When the client calls archive.tar.extract with archive="/work/repo/nested_slip.tar" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no files are written to disk in "/work/repo/extracted/"

  Scenario: Validation happens before any file write — no partial extraction on rejection
    Given a tar archive at "/work/repo/mixed.tar" containing a benign regular file "good.txt" as the first member and a traversal member "../evil.txt" as the second member
    When the client calls archive.tar.extract with archive="/work/repo/mixed.tar" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no files are written to disk in "/work/repo/extracted/"
    And the file "/work/repo/extracted/good.txt" does not exist on disk

  Scenario: Tar member with symlink pointing outside extraction root is blocked
    Given a tar archive at "/work/repo/symlink_escape.tar" whose member is a symlink entry named "link" pointing to "../../outside"
    When the client calls archive.tar.extract with archive="/work/repo/symlink_escape.tar" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no symlink named "link" exists under "/work/repo/extracted/"

  Scenario: Legitimate tar archive with all members inside target extracts successfully
    Given a tar archive where all member paths resolve inside "/work/repo/extracted/"
    When the client calls archive.tar.extract with archive="/work/repo/safe.tar" and dst="/work/repo/extracted/"
    Then all archive members are extracted into "/work/repo/extracted/"
    And no error is returned
