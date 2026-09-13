Feature: fs.rename blocks overwrite when target exists without explicit flag
  As a safety control in substrate
  I want rename to fail when the destination already exists unless overwrite is explicitly allowed
  So that existing files are not silently replaced

  Background:
    Given an allowlist with root "/work/repo"
    And the file "/work/repo/src/module_a.rs" exists on disk
    And the file "/work/repo/src/module_b.rs" exists on disk

  Scenario: Rename onto existing target without overwrite flag returns SUBSTRATE_INVALID_ARGUMENT
    When the client calls fs.rename with src="/work/repo/src/module_a.rs" and dst="/work/repo/src/module_b.rs" and overwrite=false
    Then the tool returns error code SUBSTRATE_INVALID_ARGUMENT
    And both "/work/repo/src/module_a.rs" and "/work/repo/src/module_b.rs" still exist on disk

  Scenario: Rename onto existing target with overwrite=true succeeds
    When the client calls fs.rename with src="/work/repo/src/module_a.rs" and dst="/work/repo/src/module_b.rs" and overwrite=true
    Then the file "/work/repo/src/module_b.rs" exists on disk with the contents of the former module_a.rs
    And the file "/work/repo/src/module_a.rs" does not exist on disk

  Scenario: Rename to a new path with no conflict succeeds without the overwrite flag
    Given the file "/work/repo/src/module_c.rs" does not exist
    When the client calls fs.rename with src="/work/repo/src/module_a.rs" and dst="/work/repo/src/module_c.rs" and overwrite=false
    Then the file "/work/repo/src/module_c.rs" exists on disk
    And the file "/work/repo/src/module_a.rs" does not exist on disk
