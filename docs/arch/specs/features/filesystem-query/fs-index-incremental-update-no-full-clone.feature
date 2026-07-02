Feature: A single-path mutation publishes without cloning the entire index snapshot
  As an operator of a large indexed tree
  I want a single fs.write or fs.mkdir to cost work proportional to the hot shard, not the whole index
  So that write-through latency stays flat as the indexed corpus grows

  Background:
    Given a running substrate server with the fs-index feature enabled
    And an allowlist with root "/work/repo"
    And the filesystem index for "/work/repo" contains 500000 entries across multiple immutable shards

  Scenario: A single-entry mutation rebuilds only the hot shard, not the immutable segments
    When the client calls fs.touch with path="/work/repo/one-more-file.rs"
    Then the write-through publish rebuilds only the hot delta shard
    And the pre-existing immutable shards are carried forward by reference, not cloned

  Scenario: Publish latency does not scale with total indexed entry count
    Given index.hot_shard_max_entries is configured to 1000
    When 1000 sequential single-entry mutations are committed against an index of 500000 entries
    And the same 1000 sequential single-entry mutations are committed against an index of 5000 entries
    Then the observed per-mutation publish latency is within the same order of magnitude for both corpus sizes

  Scenario: The hot shard is compacted into an immutable segment once it exceeds its bound
    Given the hot shard has reached index.hot_shard_max_entries
    When the next write-through mutation is committed
    Then a background compaction merges the hot shard into a new immutable segment
    And a fresh, empty hot shard is created to receive subsequent mutations

  Scenario: Compaction runs off the hot path and never blocks a publish
    Given a background compaction is in progress
    When a concurrent write-through mutation is committed
    Then the mutation's publish is not blocked waiting for compaction to complete
