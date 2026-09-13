Feature: Filesystem index rebuild honors CancellationToken at directory-iteration boundaries
  As an operator of substrate
  I want a cancelled rebuild to discard the partial snapshot without corrupting the active one
  So that clients continue to receive valid results from the prior snapshot after cancellation

  Background:
    Given a running substrate server with the fs-index feature enabled
    And an allowlist with root "/work/repo"
    And the filesystem index has a valid snapshot for "/work/repo"
    And a TTL-triggered rebuild of the index for "/work/repo" is in progress

  Scenario: Cancellation discards the partial snapshot and no atomic swap occurs
    When the underlying fs.find request receives a notifications/cancelled signal
    Then the CancellationToken for the rebuild task is fired
    And the partial snapshot under construction is discarded
    And the snapshot store is not updated with the partial result
    And the active snapshot for "/work/repo" remains the prior valid one

  Scenario: The prior valid snapshot continues to serve reads after cancellation
    When the rebuild is cancelled mid-walk
    And a subsequent client calls fs.find with root="/work/repo" and pattern="*.rs"
    Then the result set is drawn from the prior valid snapshot
    And every result passes through the lazy lstat validation layer
    And the response does not surface an error code for the cancelled rebuild

  Scenario: An audit event is emitted recording the rebuild cancellation
    When the rebuild for "/work/repo" is cancelled mid-walk
    Then an audit event with code SUBSTRATE_INDEX_REBUILD_CANCELLED is emitted
    And the audit event includes the root path "/work/repo"
    And the audit event includes the reason "cancellation_token_fired"
