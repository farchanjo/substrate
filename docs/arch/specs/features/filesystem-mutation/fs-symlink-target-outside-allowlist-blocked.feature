Feature: fs.symlink blocks link targets that resolve outside the allowlist
  As a security boundary in substrate
  I want symlink creation to validate the target path against the path jail
  So that an attacker cannot plant a symlink that escapes the allowlist root

  Background:
    Given an allowlist with root "/work/repo"

  Scenario: Symlink with absolute target outside allowlist is blocked before any write
    When the client calls fs.symlink with src="/etc/passwd" and dst="/work/repo/links/passwd" and dry_run=false and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_SYMLINK_ESCAPE
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters
    And the error object has field "correlation_id" matching the UUIDv7 pattern "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    And the path "/work/repo/links/passwd" does not exist on disk

  Scenario: Symlink with relative traversal target that escapes allowlist is blocked
    When the client calls fs.symlink with src="../../etc/shadow" and dst="/work/repo/links/shadow" and dry_run=false and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_SYMLINK_ESCAPE
    And the path "/work/repo/links/shadow" does not exist on disk

  Scenario: Dry run with an escaping target also returns SUBSTRATE_SYMLINK_ESCAPE without writes
    When the client calls fs.symlink with src="/etc/passwd" and dst="/work/repo/links/passwd" and dry_run=true
    Then the tool returns error code SUBSTRATE_SYMLINK_ESCAPE
    And the path "/work/repo/links/passwd" does not exist on disk

  Scenario: Symlink with a target inside the allowlist is allowed
    Given the file "/work/repo/src/lib.rs" exists on disk
    When the client calls fs.symlink with src="/work/repo/src/lib.rs" and dst="/work/repo/links/lib.rs" and dry_run=false and elicitation_confirmed=true
    Then the tool returns a success result with the created symlink path
    And the symlink "/work/repo/links/lib.rs" exists on disk pointing to "/work/repo/src/lib.rs"

  Scenario: Chained symlink whose resolved target escapes allowlist is blocked
    Given the file "/work/repo/hop1" is a symlink pointing to "/work/repo/hop2"
    And "/work/repo/hop2" is a symlink pointing to "/etc/passwd"
    When the client calls fs.symlink with src="/work/repo/hop1" and dst="/work/repo/links/chain" and dry_run=false and elicitation_confirmed=true
    Then the tool returns error code SUBSTRATE_SYMLINK_ESCAPE
    And the path "/work/repo/links/chain" does not exist on disk
