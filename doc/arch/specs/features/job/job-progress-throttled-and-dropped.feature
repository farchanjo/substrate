Feature: notifications/progress is throttled and drops oldest events under backpressure
  As an LLM agent driving substrate
  I want progress events to be emitted at a controlled cadence with defined backpressure behavior
  So that slow clients do not stall worker tasks and the server remains stable under load

  Background:
    Given a running substrate server accepting JSON-RPC 2.0 requests
    And the server configuration has jobs.progress_interval_ms set to 250
    And the server configuration has jobs.progress_channel_size set to 64

  Scenario: Progress events are emitted at 250 ms or 1 percentage point delta cadence
    Given the client has submitted an archive.tar.create job with a progressToken
    When the job runs and produces continuous progress updates internally
    Then each notifications/progress event is emitted only when at least 250 ms have elapsed since the previous emission or the progress value has increased by at least 1 percentage point since the previous emission
    And each notification/progress event contains field "sequence_number" that is strictly greater than the previous

  Scenario: When the mpsc channel with capacity 64 is full, the oldest event is dropped silently
    Given the client has submitted a job and is not consuming notifications/progress events
    When the worker submits more than 64 progress events via try_send without the client draining the channel
    Then the excess events are dropped without a panic or an error response to the client
    And the process-global progress_events_dropped counter increments for each dropped event

  Scenario: Terminal-state audit event reports the progress_events_dropped count
    Given the client has submitted a job during which 3 progress events were dropped due to backpressure
    When the job reaches a terminal state
    Then an audit event is emitted for the terminal state transition
    And the audit event has field "progress_events_dropped" equal to 3
