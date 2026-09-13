# ADR-0068 cross-ref: the stacks dir (0700) and control.fifo (0600, owner) permission boundary
Feature: an insecure supervisor registry or control FIFO is rejected
  As an operator
  I want a loose-permission stacks directory or control.fifo to be refused
  So that no same-UID or co-resident process can drive or tear down a Stack

  Scenario: a world-accessible control.fifo is rejected before the read end is opened
    Given a detached Stack whose control.fifo is mode 0666 or whose stacks directory is mode 0755
    When the supervisor starts and fstat-checks the registry
    Then startup fails with SUBSTRATE_LAUNCH_REGISTRY_INSECURE
    And the control FIFO read end is never opened
