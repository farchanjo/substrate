Feature: text.search revalidates a content-index hit before serving it, never a stale snippet
  As a correctness invariant of the content index per ADR-0072
  I want every ranked hit to pass through Layer-0 mtime/size revalidation before emission
  So that a stale snippet or score is never served for a file that changed after indexing

  Background:
    Given a running substrate server with the fs-index and fs-index-content features enabled
    And an allowlist with root "/work/repo"
    And the content index has been built for "/work/repo"

  Scenario: An unchanged file serves its cached snippet without a live re-read
    Given the file "/work/repo/src/stable.rs" is indexed and unchanged since indexing
    When the client calls text.search with root="/work/repo" and pattern="stable"
    Then the match entry for "/work/repo/src/stable.rs" is served from the cached postings
    And no live re-read of "/work/repo/src/stable.rs" occurs

  Scenario: A file changed out-of-band since indexing is re-read live before its match is served
    Given the file "/work/repo/src/changed.rs" was indexed with a cached mtime and size
    And "/work/repo/src/changed.rs" has been modified out-of-band since indexing, changing its mtime and size
    When the client calls text.search with root="/work/repo" and pattern="changed"
    Then the match entry for "/work/repo/src/changed.rs" reflects the current on-disk content
    And an IndexCommand::Upsert is enqueued for "/work/repo/src/changed.rs"

  Scenario: A file deleted out-of-band since indexing is evicted silently and excluded
    Given the file "/work/repo/src/deleted.rs" is indexed
    And "/work/repo/src/deleted.rs" has been removed out-of-band since indexing
    When the client calls text.search with root="/work/repo" and pattern="deleted"
    Then the result set does not contain "/work/repo/src/deleted.rs"
    And no error code SUBSTRATE_NOT_FOUND is surfaced in the response
    And the postings entry for "/work/repo/src/deleted.rs" is evicted
