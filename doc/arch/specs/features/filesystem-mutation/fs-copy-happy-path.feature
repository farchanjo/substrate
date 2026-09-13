Feature: fs.copy supports dry-run preview and confirmed file copy
  As an LLM agent driving substrate
  I want to preview a file copy before committing
  So that I can validate the plan without producing side effects

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/src/main.rs" exists on disk
    And the path "/work/repo/backup/main.rs" does not exist

  Scenario: Dry run returns a preview without copying the file
    When the client calls fs.copy with src="/work/repo/src/main.rs" and dst="/work/repo/backup/main.rs" and dry_run=true
    Then the tool returns a dry-run plan describing the source and destination paths
    And the file "/work/repo/backup/main.rs" does not exist on disk
    And no error is returned

  Scenario: Confirmed copy after dry run creates the destination file
    Given a dry run for src="/work/repo/src/main.rs" and dst="/work/repo/backup/main.rs" has been reviewed
    When the client calls fs.copy with src="/work/repo/src/main.rs" and dst="/work/repo/backup/main.rs" and dry_run=false and elicitation_confirmed=true
    Then the file "/work/repo/backup/main.rs" exists on disk
    And the content of "/work/repo/backup/main.rs" is identical to "/work/repo/src/main.rs"
    And the tool returns a success result with the destination path

  Scenario: Copy without elicitation confirmation returns SUBSTRATE_DRY_RUN_REQUIRED
    When the client calls fs.copy with src="/work/repo/src/main.rs" and dst="/work/repo/backup/main.rs" and dry_run=false and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_DRY_RUN_REQUIRED
    And the file "/work/repo/backup/main.rs" does not exist on disk

  Scenario: Copy to existing destination without overwrite flag returns SUBSTRATE_ALREADY_EXISTS
    Given the file "/work/repo/backup/main.rs" already exists on disk
    When the client calls fs.copy with src="/work/repo/src/main.rs" and dst="/work/repo/backup/main.rs" and dry_run=false and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_ALREADY_EXISTS
    And the existing "/work/repo/backup/main.rs" is not modified

  Scenario: Copy with overwrite=true replaces the destination file
    Given the file "/work/repo/backup/main.rs" already exists on disk with different content
    When the client calls fs.copy with src="/work/repo/src/main.rs" and dst="/work/repo/backup/main.rs" and overwrite=true and dry_run=false and elicitation_confirmed=true
    Then the file "/work/repo/backup/main.rs" exists on disk
    And the content of "/work/repo/backup/main.rs" is identical to "/work/repo/src/main.rs"

  Scenario: Source path outside allowlist returns SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    When the client calls fs.copy with src="/etc/passwd" and dst="/work/repo/backup/passwd" and dry_run=false and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    And no file is created at "/work/repo/backup/passwd"

  Scenario: Destination path outside allowlist returns SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    When the client calls fs.copy with src="/work/repo/src/main.rs" and dst="/tmp/evil.rs" and dry_run=false and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    And no file is created at "/tmp/evil.rs"
