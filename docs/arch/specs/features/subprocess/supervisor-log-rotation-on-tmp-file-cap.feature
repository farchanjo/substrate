# ADR-0056 cross-ref: subprocess supervisor semantics — BySize log rotation
# ADR-0033 cross-ref: atomic-rename invariant preserved during rotation
# ADR-0054 cross-ref: capture_kind tmp_file branch — rotation applies only here
Feature: BySize log rotation renames log files atomically and respects keep_files cap
  As an LLM agent using substrate
  I want stdout log files to be rotated when they reach max_bytes_per_file
  So that disk usage is bounded and older log segments are discarded predictably

  Background:
    Given subprocess.spawn is invoked with capture_kind tmp_file
    And log_rotation BySize max_bytes_per_file 1048576 keep_files 3

  Scenario: child writes 2.5 MiB resulting in two log files
    Given the child process has written 2621440 bytes to stdout
    When the reader task detects that stdout.log has reached max_bytes_per_file
    Then stdout.log is atomically renamed to stdout.log.1
    And a new stdout.log is opened for writing
    And the final on-disk state contains stdout.log and stdout.log.1
    And no stdout.log.2 exists because only two files were produced
    And each rename in the rotation sequence satisfies the ADR-0033 atomic-rename invariant

  Scenario: child writes 4 MiB and oldest file is unlinked to respect keep_files
    Given the child process has written 4194304 bytes to stdout
    When rotation would produce a fourth file stdout.log.3
    Then stdout.log.3 is unlinked before the rotation sequence completes
    And the final on-disk state contains stdout.log and stdout.log.1 and stdout.log.2
    And no stdout.log.3 exists on disk

  Scenario: log_rotation None causes the tmp file to grow without rotation
    Given subprocess.spawn is invoked with capture_kind tmp_file and log_rotation None
    When the child process writes more than 1048576 bytes to stdout
    Then no rotation occurs
    And stdout.log grows to the full written size
    And no stdout.log.1 exists on disk
