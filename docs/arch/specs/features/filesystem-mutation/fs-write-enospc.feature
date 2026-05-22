Feature: fs.write returns SUBSTRATE_STORAGE_FULL when the target filesystem has insufficient space
  As an LLM agent driving substrate
  I want a clear structured error when a write cannot complete due to disk space exhaustion
  So that I can surface actionable storage information rather than leaving partial files behind

  Background:
    Given an allowlist with root "/work/repo"
    And the target filesystem for "/work/repo" has less than 1 MiB of free space (near-full fixture)
    And the file "/work/repo/output.bin" does not exist on disk

  Scenario: fs.write fails with SUBSTRATE_STORAGE_FULL when data exceeds free space
    When the client calls fs.write with path="/work/repo/output.bin" and content of size 2 MiB
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_STORAGE_FULL"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"

  Scenario: No partial temporary file is left on disk after SUBSTRATE_STORAGE_FULL
    When the client calls fs.write with path="/work/repo/output.bin" and content of size 2 MiB
    Then the error object has field "code" equal to "SUBSTRATE_STORAGE_FULL"
    And no file named "output.bin" exists under "/work/repo/"
    And no ".tmp" file created during the write attempt remains under "/work/repo/"

  Scenario: SUBSTRATE_STORAGE_FULL error details include observed and limit byte counts
    When the client calls fs.write with path="/work/repo/output.bin" and content of size 2 MiB
    Then the error object has field "code" equal to "SUBSTRATE_STORAGE_FULL"
    And the error object details include field "observed_bytes" with a positive integer value
    And the error object details include field "limit_bytes" with a positive integer value
    And the value of "observed_bytes" is greater than the value of "limit_bytes"

  Scenario: fs.write succeeds when content fits within available free space
    Given the target filesystem has at least 10 MiB of free space
    When the client calls fs.write with path="/work/repo/small.txt" and content of size 1 KiB
    Then the response does not contain an error object
    And the file "/work/repo/small.txt" exists on disk with the expected content

  Scenario: SUBSTRATE_STORAGE_FULL is not confused with SUBSTRATE_READ_ONLY_FS
    When the client calls fs.write with path="/work/repo/output.bin" and content of size 2 MiB
    Then the error object has field "code" equal to "SUBSTRATE_STORAGE_FULL"
    And the error object does not have field "code" equal to "SUBSTRATE_READ_ONLY_FS"
