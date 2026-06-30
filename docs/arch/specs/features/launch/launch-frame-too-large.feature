# ADR-0068 cross-ref: MAX_COMMAND_FRAME_SIZE = PIPE_BUF-1; oversize frames rejected on both ends
Feature: an oversize control-FIFO command frame is rejected
  As the supervisor IPC plane
  I want a command frame larger than the PIPE_BUF bound to be rejected
  So that a hostile writer cannot corrupt concurrent atomic frames

  Scenario: a frame exceeding the bound is rejected by writer and consumer
    Given a control-FIFO command frame larger than MAX_COMMAND_FRAME_SIZE
    When the frame is submitted to the control plane
    Then the writer rejects it before write with SUBSTRATE_LAUNCH_FRAME_TOO_LARGE
    And a consumer-side oversize frame is discarded with the same code and a correlation_id, never reassembled
