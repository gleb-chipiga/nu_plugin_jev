# Proposal

## Why

Jev answers are normally small, and the global response-CPU semaphore adds
cross-invocation waiting without bounding the response bodies already read into
memory. Remove this disproportionate safeguard while keeping the limits that
directly govern HTTP work and response size.

## What Changes

- Remove the process-wide response-CPU semaphore and its configuration-free
  capacity calculation.
- Keep large JSON decoding and answer validation off Tokio workers, with the
  existing per-request deadline, cancellation, and abort-on-drop behavior for
  queued blocking tasks.
- Replace semaphore-specific tests with tests of the remaining cancellation
  and deadline guarantees.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `jev-http-transport`: Remove the process-wide bound on response CPU work;
  retain the independent response-size and per-evaluation deadline contracts.

## Impact

The HTTP client's response-processing helper, tests, and canonical transport
specification change. Nushell commands, flags, response records, and Cargo
dependencies do not change. The offline-estimation branch must adopt the same
client behavior when rebased onto `master`.
