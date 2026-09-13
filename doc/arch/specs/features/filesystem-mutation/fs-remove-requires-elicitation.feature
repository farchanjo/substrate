Feature: fs.remove requires elicitation before deletion
  As a safety control in substrate
  I want removal operations to require explicit user confirmation
  So that accidental data loss is prevented

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/src/old_file.rs" exists on disk

  Scenario: Remove without confirmation returns SUBSTRATE_CONFIRMATION_REQUIRED
    When the client calls fs.remove with path="/work/repo/src/old_file.rs" and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_CONFIRMATION_REQUIRED
    And the file "/work/repo/src/old_file.rs" still exists on disk

  Scenario: Remove with confirmed elicitation deletes the file
    When the client calls fs.remove with path="/work/repo/src/old_file.rs" and elicitation_confirmed=true
    Then the file "/work/repo/src/old_file.rs" does not exist on disk
    And the tool returns a success result confirming deletion

  Scenario: Remove a non-existent file with confirmation returns SUBSTRATE_NOT_FOUND
    Given the file "/work/repo/src/ghost.rs" does not exist
    When the client calls fs.remove with path="/work/repo/src/ghost.rs" and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_NOT_FOUND

  Scenario: Recursive remove of a directory requires confirmation
    Given the directory "/work/repo/src/obsolete/" contains 3 files
    When the client calls fs.remove with path="/work/repo/src/obsolete/" and recursive=true and elicitation_confirmed=false
    Then the tool returns error code SUBSTRATE_CONFIRMATION_REQUIRED
    And the directory "/work/repo/src/obsolete/" still exists on disk
