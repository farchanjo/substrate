Feature: Capability probe selects tier per port and emits a startup audit event with the full tier map
  As an operator auditing a substrate deployment
  I want a single structured event at startup that records every chosen adapter tier
  So that I can reconstruct the effective security and performance posture from logs alone

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests

  Scenario: On Linux 5.15 with statx, inotify, and openat2 the startup audit event reports the expected tier map
    Given the host is running Linux kernel 5.15 or later
    And has_statx is true
    And has_inotify is true
    And has_openat2 is true
    When substrate completes the capability probe and factory build phase during startup
    Then exactly one audit event with code "SUBSTRATE_CAPABILITY_TIERS_SELECTED" is written to stderr
    And that audit event includes field "walker_tier" equal to "linux-statx"
    And that audit event includes field "watcher_tier" equal to "linux-inotify"
    And that audit event includes field "jail_tier" equal to "linux-openat2"
    And the audit event has field "seq" equal to 0

  Scenario: On macOS 14 with getattrlistbulk, FSEvents, and O_NOFOLLOW_ANY the startup audit event reports the expected tier map
    Given the host is running macOS 14 Sonoma or later
    And has_getattrlistbulk is true
    And has_fsevents is true
    And has_o_nofollow_any is true
    When substrate completes the capability probe and factory build phase during startup
    Then exactly one audit event with code "SUBSTRATE_CAPABILITY_TIERS_SELECTED" is written to stderr
    And that audit event includes field "walker_tier" equal to "macos-bulk"
    And that audit event includes field "watcher_tier" equal to "macos-fsevents"
    And that audit event includes field "jail_tier" equal to "macos-o-nofollow-any"

  Scenario: When inotify is unavailable the watcher_tier falls back to polling and the audit event reflects that
    Given the host is a Linux environment where inotify is unavailable in the kernel
    And has_inotify is false
    When substrate completes the capability probe and factory build phase during startup
    Then the audit event with code "SUBSTRATE_CAPABILITY_TIERS_SELECTED" includes field "watcher_tier" equal to "polling"
    And a tracing warn line mentioning degraded change detection latency is present in stderr
