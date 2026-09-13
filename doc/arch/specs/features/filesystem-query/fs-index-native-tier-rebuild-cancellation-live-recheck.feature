Feature: Native-tier index rebuilds observe a live cancellation flag, not one frozen at spawn time
  As an operator relying on cancellation to bound rebuild resource usage
  I want a Linux or macOS native-tier rebuild to notice cancellation fired after the walk started
  So that cancelling mid-walk actually stops the walk instead of running to completion regardless

  Background:
    Given a running substrate server with the fs-index feature enabled
    And an allowlist with root "/work/repo" containing more than 512 subdirectories
    And a rebuild of the index for "/work/repo" has started using the native directory-walk tier

  Scenario: Cancellation requested after the walk begins is observed before the walk completes
    Given the rebuild has completed its first directory-iteration boundary
    When the CancellationToken for the rebuild is cancelled after the walk has already started
    Then the walk observes the cancellation at the next 256-entry directory boundary
    And the walk returns SubstrateError::Cancelled before visiting every subdirectory
    And the partial snapshot under construction is discarded

  Scenario: The cancellation check reads a live flag shared with the async caller, not a value captured at spawn time
    Given the rebuild's spawn_blocking closure was entered before cancellation was requested
    When cancellation fires while the blocking closure is still running
    Then the shared cancellation flag observed inside the closure reflects the post-spawn cancellation
    And the walk does not continue to completion on the assumption that cancellation was not yet requested

  Scenario: The fix applies identically on both native tiers
    Given the rebuild is running on the Linux getdents64 plus statx tier
    When cancellation fires mid-walk
    Then the Linux tier discards its partial snapshot and returns SubstrateError::Cancelled
    Given the rebuild is running on the macOS getattrlistbulk tier instead
    When cancellation fires mid-walk
    Then the macOS tier discards its partial snapshot and returns SubstrateError::Cancelled

  Scenario: The prior valid snapshot continues to serve reads after a native-tier cancellation
    When the native-tier rebuild for "/work/repo" is cancelled mid-walk
    And a subsequent client calls fs.find with root="/work/repo" and pattern="*.rs"
    Then the result set is drawn from the prior valid snapshot
    And every result passes through the Layer-0 lazy lstat validation layer
