Feature: fs.stat blocks symlink escape outside allowlist
  As a security boundary in substrate
  I want symlinks that resolve outside the allowlist root to be rejected
  So that an attacker cannot read arbitrary host files via symlink indirection

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/secret_link" is a symlink pointing to "/etc/passwd"

  Scenario: Stat on a symlink that escapes the allowlist is blocked
    When the client calls fs.stat with path="/work/repo/secret_link"
    Then the tool returns error code SUBSTRATE_SYMLINK_ESCAPE
    And the response body does not contain the content of "/etc/passwd"

  Scenario: Stat on a symlink within the allowlist succeeds
    Given the file "/work/repo/internal_link" is a symlink pointing to "/work/repo/src/main.rs"
    When the client calls fs.stat with path="/work/repo/internal_link"
    Then the tool returns file metadata for the resolved target
    And no error is returned

  Scenario: Nested symlink chain escaping allowlist is blocked
    Given "/work/repo/hop1" is a symlink to "/work/repo/hop2"
    And "/work/repo/hop2" is a symlink to "/etc/shadow"
    When the client calls fs.stat with path="/work/repo/hop1"
    Then the tool returns error code SUBSTRATE_SYMLINK_ESCAPE
    And no filesystem data outside the allowlist is returned
