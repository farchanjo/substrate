Feature: fs.mkdir supports dry-run preview and confirmed creation
  As an LLM agent driving substrate
  I want to preview directory creation before committing
  So that I can validate the plan without side effects

  Background:
    Given an allowlist with root "/work/repo"
    And the directory "/work/repo/src/new_module" does not exist

  Scenario: Dry run returns a preview without creating the directory
    When the client calls fs.mkdir with path="/work/repo/src/new_module" and dry_run=true
    Then the tool returns a dry-run plan describing the directory to be created
    And the directory "/work/repo/src/new_module" does not exist on disk
    And no error is returned

  Scenario: Confirmed creation after elicitation creates the directory
    Given a dry run for "/work/repo/src/new_module" has been reviewed
    When the client calls fs.mkdir with path="/work/repo/src/new_module" and dry_run=false and elicitation_confirmed=true
    Then the directory "/work/repo/src/new_module" exists on disk
    And the tool returns a success result with the created path

  Scenario: Creation without elicitation confirmation returns SUBSTRATE_DRY_RUN_REQUIRED
    When the client calls fs.mkdir with path="/work/repo/src/new_module" and dry_run=false and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_DRY_RUN_REQUIRED
    And the directory "/work/repo/src/new_module" does not exist on disk

  Scenario: Mkdir with parents=true creates intermediate directories
    When the client calls fs.mkdir with path="/work/repo/a/b/c" and parents=true and dry_run=false and elicitation_confirmed=true
    Then the directories "/work/repo/a", "/work/repo/a/b", and "/work/repo/a/b/c" exist on disk
