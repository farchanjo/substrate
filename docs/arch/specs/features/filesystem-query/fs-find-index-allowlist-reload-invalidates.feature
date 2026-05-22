Feature: Runtime allowlist mutation invalidates filesystem index snapshots for removed roots
  As an operator of substrate
  I want allowlist changes at runtime to be reflected in the index immediately
  So that entries under removed roots are never served after the reload completes

  Background:
    Given a running substrate server with the fs-index feature enabled
    And an allowlist with roots "/work/repo" and "/data/share"
    And the filesystem index has snapshots for both "/work/repo" and "/data/share"

  Scenario: Removing an allowlist root drops its index snapshots immediately
    When the operator reloads the substrate configuration removing "/data/share" from the allowlist
    Then the index snapshot for "/data/share" is dropped immediately via evict_root
    And no entries under "/data/share" remain in the active index

  Scenario: A subsequent fs.find against a removed root returns SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    Given the operator has reloaded configuration removing "/data/share" from the allowlist
    When the client calls fs.find with root="/data/share" and pattern="*"
    Then the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    And no filesystem read is performed for "/data/share"

  Scenario: Adding a new allowlist root triggers a lazy rebuild on first fs.find against that root
    When the operator reloads the substrate configuration adding "/work/scratch" to the allowlist
    And the client calls fs.find with root="/work/scratch" and pattern="*.rs" for the first time
    Then a Zone B index rebuild for "/work/scratch" is triggered lazily on that first call
    And the result set is drawn from the freshly built snapshot for "/work/scratch"
    And subsequent calls to fs.find against "/work/scratch" use the cached snapshot
