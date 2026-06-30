# ADR-0069 cross-ref: ToolSearch discovery — descriptions are the retrieval surface
# ADR-0007 cross-ref: thin description budget (<=100 chars, closing See substrate skill.)
Feature: launch tool descriptions are ToolSearch-discoverable and within budget
  As a small LLM resolving deferred tools through ToolSearch
  I want each launch description to be short and uniquely keyworded
  So that a query resolves to exactly one launch tool

  Scenario: each launch description is budget-compliant and uniquely discriminable
    Given the nine launch.* tool descriptions
    When the descriptions are validated
    Then each description is at most 100 characters and ends with See substrate skill.
    And each description contains a launch-domain noun among stack, service, or profile
    And no two launch descriptions share a leading verb
