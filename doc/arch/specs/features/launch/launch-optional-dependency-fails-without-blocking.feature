# ADR-0065 cross-ref: required=false demotes a failed dependency from blocker to warning
Feature: an optional dependency failure does not block its dependents
  As a developer not running every sidecar
  I want a service marked required=false to be optional
  So that a missing optional dependency is a warning, not a stack failure

  Scenario: a dependent starts when its required=false dependency fails
    Given service web depends_on cache with required=false and cache fails readiness
    When launch.up is invoked for the Stack
    Then web still starts and reaches readiness
    And the failed cache is reported as a warning, not SUBSTRATE_LAUNCH_DEPENDENCY_FAILED
