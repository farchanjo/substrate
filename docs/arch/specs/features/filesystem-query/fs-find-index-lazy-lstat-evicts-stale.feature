Feature: fs.find lazy lstat evicts stale index entries before emission
  As a correctness invariant of the fs-index feature
  I want every index hit to pass through an lstat syscall
  So that stale entries are evicted silently and never surfaced to the client

  Background:
    Given a running substrate server with the fs-index feature enabled
    And an allowlist with root "/work/repo"
    And the filesystem index has been built for "/work/repo"

  Scenario: A deleted file is evicted silently and excluded from results
    Given the file "/work/repo/deleted.rs" existed at index build time
    And "/work/repo/deleted.rs" has been removed out-of-band since the last rebuild
    When the client calls fs.find with root="/work/repo" and pattern="*.rs"
    Then the result set does not contain "/work/repo/deleted.rs"
    And no error code SUBSTRATE_NOT_FOUND is surfaced in the response
    And the index entry for "/work/repo/deleted.rs" is evicted

  Scenario: A path replaced by a different inode is re-validated and reflects current state
    Given the file "/work/repo/replaced.rs" was indexed with inode A
    And "/work/repo/replaced.rs" has been atomically replaced by a new file with inode B out-of-band
    When the client calls fs.find with root="/work/repo" and pattern="replaced.rs"
    Then the result set contains "/work/repo/replaced.rs"
    And the returned metadata reflects the current on-disk state of inode B

  Scenario: A symlink whose target was deleted is excluded when is_file filter is active
    Given the symlink "/work/repo/broken-link.rs" is indexed
    And the target of "/work/repo/broken-link.rs" has been removed out-of-band
    When the client calls fs.find with root="/work/repo" and pattern="*.rs" and is_file=true
    Then the result set does not contain "/work/repo/broken-link.rs"
    And no error code is returned for the broken symlink
