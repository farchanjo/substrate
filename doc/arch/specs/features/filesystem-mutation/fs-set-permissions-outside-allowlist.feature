Feature: fs.set_permissions rejects paths outside the allowlist
  As a security boundary in substrate
  I want permission changes to be confined to the configured allowlist
  So that substrate cannot be used to modify host system file permissions

  Background:
    Given an allowlist with root "/work/repo"

  Scenario: Set permissions on a path outside the allowlist returns SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    When the client calls fs.set_permissions with path="/etc/cron.d/job" and mode="0777"
    Then the tool returns error code SUBSTRATE_PATH_OUTSIDE_ALLOWLIST
    And the permissions of "/etc/cron.d/job" are not changed

  Scenario: Set permissions on a path inside the allowlist succeeds
    Given the file "/work/repo/scripts/deploy.sh" exists with mode "0644"
    When the client calls fs.set_permissions with path="/work/repo/scripts/deploy.sh" and mode="0755"
    Then the file "/work/repo/scripts/deploy.sh" has mode "0755" on disk

  Scenario: Set permissions via traversal path is blocked
    When the client calls fs.set_permissions with path="/work/repo/../../etc/passwd" and mode="0777"
    Then the tool returns error code SUBSTRATE_PATH_TRAVERSAL_BLOCKED
    And the permissions of "/etc/passwd" are not changed

  Scenario: Set permissions on a symlink escaping allowlist is blocked
    Given "/work/repo/sys_link" is a symlink pointing to "/usr/bin/env"
    When the client calls fs.set_permissions with path="/work/repo/sys_link" and mode="0777"
    Then the tool returns error code SUBSTRATE_SYMLINK_ESCAPE
    And the permissions of "/usr/bin/env" are not changed
