Feature: fs.symlink creates symbolic links within the allowlist
  As an LLM agent driving substrate
  I want to create symbolic links between paths inside the allowlist
  So that I can establish canonical aliases for files without duplicating data

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/src/main.rs" exists on disk
    And the path "/work/repo/links/main.rs" does not exist

  Scenario: Dry run returns a preview without creating the symlink
    When the client calls fs.symlink with src="/work/repo/src/main.rs" and dst="/work/repo/links/main.rs" and dry_run=true
    Then the tool returns a dry-run plan describing the link source and destination
    And the path "/work/repo/links/main.rs" does not exist on disk
    And no error is returned

  Scenario: Confirmed creation after dry run creates the symlink
    Given a dry run for src="/work/repo/src/main.rs" and dst="/work/repo/links/main.rs" has been reviewed
    When the client calls fs.symlink with src="/work/repo/src/main.rs" and dst="/work/repo/links/main.rs" and dry_run=false and elicitation_confirmed=true
    Then the symlink "/work/repo/links/main.rs" exists on disk pointing to "/work/repo/src/main.rs"
    And the tool returns a success result with the created symlink path

  Scenario: Creation without elicitation confirmation returns SUBSTRATE_DRY_RUN_REQUIRED
    When the client calls fs.symlink with src="/work/repo/src/main.rs" and dst="/work/repo/links/main.rs" and dry_run=false and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_DRY_RUN_REQUIRED
    And the path "/work/repo/links/main.rs" does not exist on disk

  Scenario: Destination already exists returns SUBSTRATE_ALREADY_EXISTS
    Given a symlink or file already exists at "/work/repo/links/main.rs"
    When the client calls fs.symlink with src="/work/repo/src/main.rs" and dst="/work/repo/links/main.rs" and dry_run=false and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_ALREADY_EXISTS
