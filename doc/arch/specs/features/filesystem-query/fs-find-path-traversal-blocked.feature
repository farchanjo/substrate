Feature: fs.find blocks path traversal attempts
  As a security boundary in substrate
  I want path traversal patterns to be rejected before filesystem access
  So that the allowlist cannot be bypassed via relative segments

  Background:
    Given an allowlist with root "/work/repo"

  Scenario: Traversal via leading "../" segments is blocked
    When the client calls fs.find with root="../etc" and pattern="*"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no filesystem read is performed

  Scenario: Traversal embedded inside path is blocked
    When the client calls fs.find with root="/work/repo/../../etc" and pattern="*"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no filesystem read is performed

  Scenario: Encoded traversal sequence "%2e%2e" is blocked
    When the client calls fs.find with root="/work/repo/%2e%2e/etc" and pattern="*"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And no filesystem read is performed

  Scenario: Absolute path outside allowlist is blocked with correct error
    When the client calls fs.find with root="/tmp/outside" and pattern="*"
    Then the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    And no filesystem read is performed
