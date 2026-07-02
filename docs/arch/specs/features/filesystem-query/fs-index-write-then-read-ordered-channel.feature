Feature: Write-through consistency is guaranteed by an ordered channel with an oneshot ack
  As an LLM agent chaining a mutation followed by a query
  I want the index to already reflect a mutation before its tool response returns
  So that an immediately-following fs.find or text.search never race a not-yet-applied write

  Background:
    Given a running substrate server with the fs-index feature enabled
    And an allowlist with root "/work/repo"
    And the filesystem index has been built for "/work/repo"

  Scenario: fs.write's tool response is not returned until the index ack is received
    When the client calls fs.write with path="/work/repo/new.rs" and content="fn main() {}"
    Then the write-through IndexCommand::Upsert is sent on the ordered mpsc queue before the tool response
    And the tool response is only returned after the IndexerActor's oneshot ack is received
    And the acked generation is greater than or equal to the generation observed before the call

  Scenario: A query issued immediately after the acked response observes the mutation
    Given the client received a fs.write tool response for "/work/repo/new.rs" with its index ack confirmed
    When the client immediately calls fs.find with root="/work/repo" and pattern="new.rs"
    Then the result set contains "/work/repo/new.rs"

  Scenario: Two concurrent mutations to related paths apply in commit order, not arrival order
    Given a client calls fs.rename with source="/work/repo/a.rs" and destination="/work/repo/b.rs"
    And a second client concurrently calls fs.write with path="/work/repo/b.rs" and new content
    When both write-through commands are drained by the single-writer IndexerActor
    Then the commands are applied in the order they were sent on the queue
    And the final indexed state for "/work/repo/b.rs" reflects whichever commit actually happened last on disk

  Scenario: An ack timeout does not fail the mutation and is closed by the Layer-0 backstop
    Given the IndexerActor is saturated and the write-through ack for "/work/repo/slow.rs" times out
    When the client calls fs.write with path="/work/repo/slow.rs" and content="data"
    Then the tool response still reports the write as successful
    And a subsequent fs.find call for "/work/repo/slow.rs" triggers a live lazy-lstat revalidation
    And the result set contains "/work/repo/slow.rs" with current on-disk metadata
