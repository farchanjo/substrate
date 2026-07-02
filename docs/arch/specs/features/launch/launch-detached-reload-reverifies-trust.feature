# ADR-0065/ADR-0068 cross-ref (2026-07-01 amendment): a detached stack's
# supervisor re-verifies trust on reload via load_trusted, matching the
# in-session launch.reload trust gate, instead of trusting the swapped path
Feature: launch.reload against a detached stack re-verifies trust
  As an operator relying on TOFU trust for a detached stack
  I want a reload that swaps in a different Profile to be re-blessed
  So that an attacker-swapped Profile is never silently applied to a running stack

  Scenario: reloading a detached stack from an unblessed Profile is rejected
    Given a running detached Stack
    And a replacement Profile at a path with no bless record in the trust store
    When launch.reload is invoked with that replacement Profile path
    Then the detached supervisor rejects it with SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED
    And the stack's running Services are left untouched

  Scenario: reloading a detached stack from an operator-blessed Profile is applied
    Given a running detached Stack
    And a replacement Profile at a path blessed in the trust store
    When launch.reload is invoked with that replacement Profile path
    Then the detached supervisor applies the reload
    And launch.status reflects the new Profile's config hash
