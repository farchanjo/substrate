Feature: archive.hash computes an integrity digest of an archive without extraction
  As an LLM agent driving substrate
  I want to hash an archive file with a specified algorithm
  So that I can verify archive integrity before and after transfer

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/dist/release.tar.gz" exists on disk with known content

  Scenario: Default algorithm (blake3) returns a hex digest
    When the client calls archive.hash with path="/work/repo/dist/release.tar.gz"
    Then the tool returns a hex string digest in the structured content
    And the structured content includes field "algorithm" with value "blake3"
    And no error is returned

  Scenario: sha256 algorithm returns a 64-character hex digest
    When the client calls archive.hash with path="/work/repo/dist/release.tar.gz" and algorithm="sha256"
    Then the tool returns a hex string digest of exactly 64 characters
    And the structured content includes field "algorithm" with value "sha256"

  Scenario: Repeated calls with the same algorithm return the same digest
    When the client calls archive.hash with path="/work/repo/dist/release.tar.gz" and algorithm="blake3"
    And the client calls archive.hash with path="/work/repo/dist/release.tar.gz" and algorithm="blake3" again
    Then both calls return identical digests

  Scenario: Path outside allowlist returns SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    When the client calls archive.hash with path="/tmp/outside.tar.gz"
    Then the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    And no file content is read

  Scenario: Non-existent archive path returns SUBSTRATE_NOT_FOUND
    Given the path "/work/repo/dist/missing.tar.gz" does not exist
    When the client calls archive.hash with path="/work/repo/dist/missing.tar.gz"
    Then the tool returns error code SUBSTRATE_NOT_FOUND

  Scenario: Path pointing to a directory returns SUBSTRATE_NOT_A_FILE
    When the client calls archive.hash with path="/work/repo/dist"
    Then the tool returns error code SUBSTRATE_NOT_A_FILE
