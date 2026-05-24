# ADR-0053 cross-ref: process lifecycle cascade contract — macOS watchdog pipe mechanism
# ADR-0052 cross-ref: subprocess bounded context — macOS-specific orphan prevention
Feature: macOS substrate-aware child detects watchdog pipe EOF and exits
  As an operator
  I want substrate-aware child processes on macOS to exit when substrate is killed
  So that no orphaned processes persist despite macOS lacking PR_SET_PDEATHSIG

  Scenario: macOS substrate-aware child detects watchdog pipe EOF and exits
    Given target OS is macOS
    And a substrate-aware test binary reads SUBSTRATE_WATCHDOG_FD env var on startup
    And a subprocess job is Running with the watchdog pipe installed
    When substrate process is SIGKILL'd
    Then the write-end of the watchdog pipe is closed by the kernel
    And the child watchdog thread observes EOF on read
    And the child calls _exit(0) within 100ms
