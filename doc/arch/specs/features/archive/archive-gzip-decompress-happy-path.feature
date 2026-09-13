Feature: archive.gzip.decompress supports dry-run preview and confirmed decompression
  As an LLM agent driving substrate
  I want to preview gzip decompression before committing to disk
  So that I can validate the output path without producing side effects

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/dist/output.bin.gz" exists on disk as a valid gzip archive
    And the path "/work/repo/dist/output.bin" does not exist

  Scenario: Dry run returns a plan without writing to disk
    When the client calls archive.gzip.decompress with src="/work/repo/dist/output.bin.gz" and dst="/work/repo/dist/output.bin" and dry_run=true
    Then the tool returns a dry-run plan describing the decompressed output path and estimated size
    And the file "/work/repo/dist/output.bin" does not exist on disk
    And no error is returned

  Scenario: Confirmed decompression after dry run writes the output file
    Given a dry run for "/work/repo/dist/output.bin" has been reviewed
    When the client calls archive.gzip.decompress with src="/work/repo/dist/output.bin.gz" and dst="/work/repo/dist/output.bin" and dry_run=false and elicitation_confirmed=true
    Then the file "/work/repo/dist/output.bin" exists on disk
    And the tool returns a success result with the decompressed file size in bytes

  Scenario: Decompression without elicitation confirmation returns SUBSTRATE_DRY_RUN_REQUIRED
    When the client calls archive.gzip.decompress with src="/work/repo/dist/output.bin.gz" and dst="/work/repo/dist/output.bin" and dry_run=false and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_DRY_RUN_REQUIRED
    And the file "/work/repo/dist/output.bin" does not exist on disk

  Scenario: Destination path outside allowlist returns SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    When the client calls archive.gzip.decompress with src="/work/repo/dist/output.bin.gz" and dst="/tmp/output.bin" and dry_run=false and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    And no file is created at "/tmp/output.bin"

  Scenario: Source is not a valid gzip archive returns SUBSTRATE_ENCODING_ERROR
    Given the file "/work/repo/dist/corrupt.gz" is not a valid gzip stream
    When the client calls archive.gzip.decompress with src="/work/repo/dist/corrupt.gz" and dst="/work/repo/dist/output.bin" and dry_run=false and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_ENCODING_ERROR
    And no file is created at "/work/repo/dist/output.bin"

  Scenario: Progress notification is emitted for large archives
    Given the file "/work/repo/dist/output.bin.gz" is large enough that decompression takes >= 1 second
    When the client calls archive.gzip.decompress with src="/work/repo/dist/output.bin.gz" and dst="/work/repo/dist/output.bin" and dry_run=false and elicitation_confirmed=true
    Then at least one ProgressNotification is emitted with a progressToken
