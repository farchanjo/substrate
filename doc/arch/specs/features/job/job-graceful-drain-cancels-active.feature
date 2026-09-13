Feature: SIGTERM and SIGINT cancel all active jobs before STDIO closes
  As an operator of substrate
  I want the server to cleanly cancel in-flight jobs on shutdown signals
  So that transactional tmp files are removed and clients receive a final cancellation notification

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the server configuration has shutdown_drain_secs set to 5
    And two archive.tar.create jobs are currently running

  Scenario: On SIGTERM every active job receives a CancellationToken cancel within shutdown_drain_secs
    When the operating system delivers SIGTERM to the substrate process
    Then the root CancellationToken is cancelled within 100 ms
    And each active job's child CancellationToken is signalled as cancelled within 100 ms of root cancellation
    And all active jobs transition to state cancelled within shutdown_drain_secs

  Scenario: Final notifications/progress event with state=cancelled is emitted before STDIO closes
    When the operating system delivers SIGTERM to the substrate process
    Then for each active job a notifications/progress event is emitted on STDIO with job_state="cancelled"
    And all cancellation notifications are emitted before the STDIO channel closes

  Scenario: Transactional tmp files are cleaned for every active job during graceful drain
    Given each running archive.tar.create job has created one or more .tmp.<uuid7> files under the destination path
    When the operating system delivers SIGTERM to the substrate process
    Then all .tmp.<uuid7> files created by active jobs are removed before the process exits
    And no .tmp.<uuid7> files remain under any destination path after the process has exited
