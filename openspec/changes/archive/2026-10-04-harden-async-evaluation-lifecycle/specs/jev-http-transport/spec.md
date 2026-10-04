# Spec Delta

## MODIFIED Requirements

### Requirement: Per-evaluation total deadline

Timeout SHALL bound one dispatched logical evaluation, including HTTP attempts,
retry waits, body decoding, and response-contract validation. It SHALL NOT
impose one deadline on a whole table invocation. Cancellation SHALL interrupt
awaiting requests and waits without waiting for timeout expiry.

#### Scenario: Multiple attempts share one deadline

- **WHEN** retry waits and HTTP attempts together exhaust the configured timeout
- **THEN** the evaluation terminates even if no individual attempt consumed the full timeout

#### Scenario: Long table invocation

- **WHEN** each row finishes within its evaluation deadline but the whole table takes longer than that duration
- **THEN** the table does not fail solely because of total invocation elapsed time

#### Scenario: Validation exceeds the remaining deadline

- **WHEN** a complete JSON response arrives before the deadline but contract validation finishes after it
- **THEN** the evaluation reports a timeout without returning that response or success metrics
