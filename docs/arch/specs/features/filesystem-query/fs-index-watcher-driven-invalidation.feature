Feature: The filesystem watcher is wired into the composition root and drives index invalidation
  As an operator running substrate with fs-index-watch enabled
  I want out-of-band filesystem changes to reach the index without a client-initiated call
  So that fs.find and text.search reflect external mutations without waiting for a TTL rebuild

  Background:
    Given a running substrate server with the fs-index and fs-index-watch features enabled
    And an allowlist with root "/work/repo"
    And the filesystem index has been built for "/work/repo"

  Scenario: A file created out-of-band is observed by the watcher and enqueued without a client call
    Given no client has called fs.find or text.search since server startup
    When the file "/work/repo/new-file.rs" is created out-of-band by a process other than substrate
    Then the FsIndexWatcher observes a native filesystem event for "/work/repo/new-file.rs"
    And an IndexCommand::Upsert for "/work/repo/new-file.rs" is enqueued onto the single-writer queue
    And an IndexEvent::Upserted is published on the broadcast channel

  Scenario: A subsequent fs.find reflects the watcher-driven update
    Given the file "/work/repo/new-file.rs" was created out-of-band and observed by the watcher
    When the client calls fs.find with root="/work/repo" and pattern="new-file.rs"
    Then the result set contains "/work/repo/new-file.rs"

  Scenario: A watch queue overflow degrades to a full-root Rescan rather than losing events
    Given a burst of filesystem events exceeds the watcher's internal queue capacity
    When the overflow condition fires
    Then an IndexCommand::Rescan with reason="watch_overflow" is enqueued for the affected root
    And the stale snapshot continues to serve reads, filtered by Layer-0 lazy lstat, until the rescan completes

  Scenario: The watcher is absent when fs-index-watch is not compiled in
    Given a running substrate server with the fs-index feature enabled and fs-index-watch compiled out
    When a file is created out-of-band under "/work/repo"
    Then no FsIndexWatcher event is observed
    And the entry becomes visible only after the next TTL-triggered rebuild or a client-triggered lookup revalidation
