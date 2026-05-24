# ADR-0033 cross-ref: transactional write pattern — amendment 2026-05-24 (subprocess tmp files)
# ADR-0054 cross-ref: subprocess stdout/stderr stream multiplex — TmpFile branch
Feature: TmpFile capture mode persists subprocess output to disk via atomic rename
  As an LLM agent using substrate
  I want subprocess stdout/stderr persisted as final files in tmp_root when the child exits successfully
  So that I can read large output without holding it in memory and confirm atomicity via the absence of transit files

  Scenario: TmpFile capture mode persists bytes to disk and finalises via atomic rename
    Given subprocess.tmp_root is configured to a writable directory inside policy.roots
    And subprocess.spawn is invoked with capture_kind "tmp_file" emitting 4096 stdout bytes
    When the child exits with code 0 and the job transitions to Succeeded
    Then subprocess.result returns stdout_tmp_path pointing to a file
    And the file at stdout_tmp_path exists on disk
    And the file size equals 4096 bytes
    And the stdout_tmp_path matches the pattern ".*/.substrate-subprocess-stream-[0-9a-f-]+\\.stdout$"
    And no transit file matching ".*\\.tmp\\.[0-9a-f-]+$" remains under tmp_root
