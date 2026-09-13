Feature: Every index entry passes through path-jail re-validation before emission
  As a security invariant of the fs-index feature per ADR-0035
  I want each candidate retrieved from the snapshot to be re-validated by the active PathJail tier
  So that index entries that escape the allowlist are rejected before reaching the client

  Background:
    Given a running substrate server with the fs-index feature enabled
    And an allowlist with root "/work/repo"
    And the filesystem index has been built for "/work/repo"

  Scenario: An entry whose canonical path escaped the allowlist since index time is rejected and evicted
    Given the file "/work/repo/file.rs" was indexed while its canonical path was inside the allowlist
    And the allowlist has been mutated since index time so that "/work/repo/file.rs" now resolves outside it
    When the client calls fs.find with root="/work/repo" and pattern="file.rs"
    Then the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST for that entry
    And the index entry for "/work/repo/file.rs" is evicted from the snapshot
    And the entry is not present in the result set

  Scenario: An entry whose symlink target changed to escape the allowlist is rejected by path-jail re-validation
    Given the symlink "/work/repo/link.rs" was indexed with a target inside the allowlist
    And the symlink target has been changed out-of-band to a path outside the allowlist
    When the client calls fs.find with root="/work/repo" and pattern="link.rs"
    Then path-jail re-validation via openat2 or O_NOFOLLOW_ANY detects the escape
    And the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST for that entry
    And the index entry for "/work/repo/link.rs" is evicted

  Scenario: Path-jail re-validation still runs and emits a degraded audit event under userspace-degraded tier
    Given the kernel-level PathJail tier is unavailable at startup
    And the server operates in userspace-degraded tier
    When the server starts up
    Then an audit event with code SUBSTRATE_JAIL_DEGRADED is emitted at startup
    And subsequent fs.find calls still perform path-jail re-validation on every index hit
    And the re-validation uses the userspace canonicalize-and-check fallback path
