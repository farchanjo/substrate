Feature: proc.tree returns a parent-child process hierarchy rooted at a given PID
  As an LLM agent driving substrate
  I want to inspect the process tree before issuing a proc.signal call
  So that I can confirm the target PID identity and its children before sending a signal

  Background:
    Given the host has more than 10 running processes
    And PID 1 is the init or launchd process

  Scenario: proc.tree rooted at PID 1 returns a node with process info and children
    When the client calls proc.tree with root_pid=1
    Then the structured content contains a pid field equal to 1
    And the structured content contains a children field of array type
    And the structured content contains a node_count field of positive integer type
    And the structured content contains a truncated field of boolean type
    And no error is returned

  Scenario: Each tree node carries the required ProcessInfo fields
    When the client calls proc.tree with root_pid=1
    Then the root node has a pid field of non-negative integer type
    And the root node has a ppid field equal to 0
    And the root node has a name field of non-empty string type
    And the root node has a command field of string type
    And the root node has a state field of non-empty string type
    And the root node has a cpu_pct field of float type
    And the root node has a rss_kb field of non-negative integer type

  Scenario: node_count matches the actual number of nodes in the returned tree
    When the client calls proc.tree with root_pid=1
    Then the node_count field equals the total number of nodes reachable from the root
    And the truncated field is false when node_count is less than the node cap

  Scenario: Children nodes are sorted by pid in ascending order
    When the client calls proc.tree with root_pid=1 and the tree has more than one child
    Then the children array is sorted by pid in ascending order

  Scenario: proc.tree truncates and sets truncated=true when node cap is reached
    Given the host process forest exceeds 500 nodes from PID 1
    When the client calls proc.tree with root_pid=1
    Then the truncated field is true
    And the node_count field is less than or equal to 500
    And the hints map includes a next_action_suggested entry recommending a narrower root_pid

  Scenario: proc.tree recommended workflow — inspect tree before signalling
    Given a process with PID 5678 is running and has child processes
    When the client calls proc.tree with root_pid=5678
    Then the root node pid field equals 5678
    And the children field lists the direct child processes
    And no error is returned

  Scenario: proc.tree returns SUBSTRATE_NOT_FOUND for a non-existent root PID
    Given PID 99999 does not refer to any running process
    When the client calls proc.tree with root_pid=99999
    Then the response contains an error object
    And the error object has field "code" equal to "SUBSTRATE_NOT_FOUND"
    And the error object has field "recovery_hint" whose length is between 1 and 150 characters

  Scenario: proc.tree platform parity — Linux uses procfs, macOS uses sysctl
    Given the server is running on any supported platform
    When the client calls proc.tree with root_pid=1
    Then the root node is returned with children regardless of platform
    And the structured content does not contain an error code field
