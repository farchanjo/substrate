Feature: archive.zip_extract blocks symlink members that point outside the extraction root
  As a security boundary in substrate
  I want symlink entries in a zip archive to be validated before any file is written
  So that a crafted archive cannot plant a symlink that escapes the extraction directory

  Background:
    Given an allowlist with root "/work/repo"
    And the extraction target directory is "/work/repo/extracted/"

  Scenario: Archive whose first member is a symlink pointing outside extraction root is rejected before any write
    Given a zip archive at "/work/repo/symlink_escape.zip" whose first member is a symlink entry named "link" pointing to "../../outside"
    When the client calls archive.zip_extract with archive="/work/repo/symlink_escape.zip" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And no symlink named "link" exists under "/work/repo/extracted/"
    And no other files are written to "/work/repo/extracted/"

  Scenario: Symlink entry pointing to an absolute path outside extraction root is rejected
    Given a zip archive at "/work/repo/abs_symlink.zip" whose member is a symlink entry pointing to "/etc/passwd"
    When the client calls archive.zip_extract with archive="/work/repo/abs_symlink.zip" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no files are written to disk in "/work/repo/extracted/"
    And the path "/etc/passwd" is not created or modified

  Scenario: Symlink entry pointing inside the extraction root is allowed
    Given a zip archive at "/work/repo/safe_symlink.zip" whose member is a symlink entry named "a/link" pointing to "a/target.txt"
    And that archive also contains the regular file "a/target.txt"
    When the client calls archive.zip_extract with archive="/work/repo/safe_symlink.zip" and dst="/work/repo/extracted/"
    Then the tool returns a success result
    And the symlink "/work/repo/extracted/a/link" exists on disk pointing to "a/target.txt"
    And the file "/work/repo/extracted/a/target.txt" exists on disk

  Scenario: Validation happens before any file write — no partial extraction on rejection
    Given a zip archive at "/work/repo/mixed.zip" containing a benign regular file "good.txt" as the first member and a symlink escape entry as the second member
    When the client calls archive.zip_extract with archive="/work/repo/mixed.zip" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no files are written to disk in "/work/repo/extracted/"
    And the file "/work/repo/extracted/good.txt" does not exist on disk

  Scenario: Symlink loop created via archive member is blocked
    Given a zip archive at "/work/repo/loop.zip" whose members are two symlink entries "a" pointing to "b" and "b" pointing to "a"
    When the client calls archive.zip_extract with archive="/work/repo/loop.zip" and dst="/work/repo/extracted/"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no symlinks are created on disk in "/work/repo/extracted/"
