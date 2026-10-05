# Spec Delta

## MODIFIED Requirements

### Requirement: No runtime transport override

The selected build's HTTP version policy SHALL NOT be changed by command
flags, environment values, or NUON settings.

#### Scenario: Runtime settings do not change protocol policy

- **WHEN** a default build receives command flags, environment values, or NUON settings
- **THEN** none of them disables HTTP/2 prior knowledge for a live request
