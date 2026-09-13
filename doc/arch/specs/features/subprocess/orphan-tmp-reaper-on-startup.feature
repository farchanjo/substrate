# ADR-0055 cross-ref: orphan reaper on startup
# ADR-0033 cross-ref: transactional write pattern (tmp file naming convention)
# ADR-0052 cross-ref: subprocess bounded context — startup cleanup
Feature: Stale tmp files older than orphan_reap_age_secs are removed at startup
  As an operator restarting substrate after a crash
  I want orphaned temporary files from a previous run to be cleaned up automatically
  So that disk space is not leaked and stale tmp files do not interfere with new operations

  Scenario: stale tmp files older than orphan_reap_age_secs are removed at startup
    Given a stale file /tmp/sandbox/foo.tmp.0192f000-7c0e-7000-8000-000000000001 exists with mtime 20 minutes ago
    And startup.orphan_reap_age_secs is 600
    When substrate starts up
    Then the orphan reaper removes the stale file
    And emits audit event SUBSTRATE_ORPHAN_TMP_REAPED
