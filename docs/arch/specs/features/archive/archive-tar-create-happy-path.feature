Feature: archive.tar_create supports dry-run plan and confirmed archive creation
  As an LLM agent driving substrate
  I want to preview tar archive contents before committing to disk
  So that I can validate the file set without producing side effects

  Background:
    Given an allowlist with root "/work/repo"
    And the directory "/work/repo/src" contains 10 Rust source files
    And the destination path "/work/repo/dist/src.tar.gz" does not exist

  Scenario: Dry run returns plan without writing to disk
    When the client calls archive.tar_create with src="/work/repo/src" and dst="/work/repo/dist/src.tar.gz" and dry_run=true
    Then the tool returns a dry-run plan listing the 10 files to be archived
    And the file "/work/repo/dist/src.tar.gz" does not exist on disk
    And no error is returned

  Scenario: Confirmed creation after elicitation writes the archive
    Given a dry run for "/work/repo/dist/src.tar.gz" has been reviewed
    When the client calls archive.tar_create with src="/work/repo/src" and dst="/work/repo/dist/src.tar.gz" and dry_run=false and elicitation_confirmed=true
    Then the file "/work/repo/dist/src.tar.gz" exists on disk
    And the tool returns a success result with archive size in bytes

  Scenario: Creation without elicitation confirmation returns SUBSTRATE_DRY_RUN_REQUIRED
    When the client calls archive.tar_create with src="/work/repo/src" and dst="/work/repo/dist/src.tar.gz" and dry_run=false and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_DRY_RUN_REQUIRED
    And the file "/work/repo/dist/src.tar.gz" does not exist on disk

  Scenario: Progress notification is emitted for large archives
    Given the directory "/work/repo/src" contains enough data that archiving takes >= 1 second
    When the client calls archive.tar_create with src="/work/repo/src" and dst="/work/repo/dist/src.tar.gz" and dry_run=false and elicitation_confirmed=true
    Then at least one ProgressNotification is emitted with a progressToken
