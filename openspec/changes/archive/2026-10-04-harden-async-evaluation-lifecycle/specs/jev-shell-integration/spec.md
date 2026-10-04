# Spec Delta

## MODIFIED Requirements

### Requirement: Validate configured setting types

Timeout SHALL be a positive Nu duration in flags/Nu config or positive integer
milliseconds in environment/TOML. Jobs SHALL be a positive integer that the
bounded scheduler can represent; retries SHALL be nonnegative. Proxy policy
SHALL be `auto`, `direct`, or an explicit `http://` or `socks5h://` URL.

#### Scenario: Invalid timeout setting

- **WHEN** a selected timeout from environment or TOML is zero or not an integer millisecond count
- **THEN** the invocation rejects it before HTTP dispatch rather than using another source

#### Scenario: Jobs exceed scheduler capacity

- **WHEN** a positive selected `jobs` value would exceed the scheduler's admission capacity
- **THEN** the invocation rejects it before reading input or starting HTTP rather than panicking
