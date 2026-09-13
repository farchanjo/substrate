Feature: Mutation tools update the filesystem index at commit time via write-through
  As a consistency guarantee of the fs-index feature
  I want every in-process mutation to update the index at the atomic-rename commit point
  So that subsequent fs.find calls reflect the mutation immediately without waiting for a TTL rebuild

  Background:
    Given a running substrate server with the fs-index feature enabled
    And an allowlist with root "/work/repo"
    And the filesystem index has been built for "/work/repo"

  Scenario: fs.mkdir entry is visible to fs.find immediately after commit
    Given the directory "/work/repo/new-dir" does not exist in the index
    When the client calls fs.mkdir with path="/work/repo/new-dir"
    Then the mutation commits successfully
    And the client calls fs.find with root="/work/repo" and pattern="new-dir"
    And the result set contains "/work/repo/new-dir"
    And the index entry was added via write-through at commit time without a TTL wait

  Scenario: fs.remove evicts the entry from the index at the same atomic point
    Given the file "/work/repo/old-file.rs" exists in the index
    When the client calls fs.remove with path="/work/repo/old-file.rs"
    Then the mutation commits successfully
    And the client calls fs.find with root="/work/repo" and pattern="old-file.rs"
    And the result set does not contain "/work/repo/old-file.rs"
    And the index entry for "/work/repo/old-file.rs" was evicted at commit time

  Scenario: fs.rename updates the index to reflect the new path and removes the old
    Given the file "/work/repo/source.rs" exists in the index
    And the path "/work/repo/dest.rs" is not present in the index
    When the client calls fs.rename with source="/work/repo/source.rs" and destination="/work/repo/dest.rs"
    Then the mutation commits successfully
    And the client calls fs.find with root="/work/repo" and pattern="dest.rs"
    And the result set contains "/work/repo/dest.rs"
    And the client calls fs.find with root="/work/repo" and pattern="source.rs"
    And the result set does not contain "/work/repo/source.rs"
