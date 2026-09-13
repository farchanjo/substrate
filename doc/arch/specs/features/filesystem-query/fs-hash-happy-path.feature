Feature: fs.hash computes file integrity digests with algorithm selection
  As an LLM agent driving substrate
  I want to compute cryptographic hashes of files with a specified algorithm
  So that I can verify integrity before and after mutation operations

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/dist/binary.bin" exists on disk with known content

  Scenario: Default algorithm (blake3) returns a hex digest
    When the client calls fs.hash with path="/work/repo/dist/binary.bin"
    Then the tool returns a hex string digest in the structured content
    And the structured content includes field "algorithm" with value "blake3"
    And no error is returned

  Scenario: Explicit blake3 selection returns same digest as default
    When the client calls fs.hash with path="/work/repo/dist/binary.bin" and algorithm="blake3"
    Then the tool returns a hex string digest in the structured content
    And the digest matches the digest returned by the default call

  Scenario: sha256 algorithm returns a 64-character hex digest
    When the client calls fs.hash with path="/work/repo/dist/binary.bin" and algorithm="sha256"
    Then the tool returns a hex string digest of exactly 64 characters
    And the structured content includes field "algorithm" with value "sha256"

  Scenario: sha512 algorithm returns a 128-character hex digest
    When the client calls fs.hash with path="/work/repo/dist/binary.bin" and algorithm="sha512"
    Then the tool returns a hex string digest of exactly 128 characters
    And the structured content includes field "algorithm" with value "sha512"

  Scenario: md5 algorithm returns a 32-character hex digest
    When the client calls fs.hash with path="/work/repo/dist/binary.bin" and algorithm="md5"
    Then the tool returns a hex string digest of exactly 32 characters
    And the structured content includes field "algorithm" with value "md5"

  Scenario: Path outside allowlist returns SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    When the client calls fs.hash with path="/etc/passwd" and algorithm="blake3"
    Then the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    And no file content is read

  Scenario: Non-existent path returns SUBSTRATE_NOT_FOUND
    Given the path "/work/repo/dist/missing.bin" does not exist
    When the client calls fs.hash with path="/work/repo/dist/missing.bin"
    Then the tool returns error code SUBSTRATE_NOT_FOUND

  Scenario: Hash of a directory path returns SUBSTRATE_NOT_A_FILE
    When the client calls fs.hash with path="/work/repo/src"
    Then the tool returns error code SUBSTRATE_NOT_A_FILE
