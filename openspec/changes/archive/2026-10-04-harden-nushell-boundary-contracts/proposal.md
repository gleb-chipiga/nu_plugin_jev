# Proposal

## Why

The plugin currently has no spec-level guarantee that malformed Nu records cannot silently lose fields before an API request. Its command signatures also leave otherwise known Nu input and output types undiscoverable. The boundary behavior already being implemented should be explicit and testable.

## What Changes

- **BREAKING** Reject unknown raw question fields and duplicate question names before constructing or sending a request, rather than ignoring or overwriting them.
- **BREAKING** Reject duplicate keys in any selected outbound Nu record, with a location-aware error, rather than collapsing them during JSON conversion.
- **BREAKING** Reject pipeline input to the offline root guidance and question constructors instead of silently ignoring it.
- Declare truthful Nu input/output types for all seven commands, including an intentionally broad table output type because error handling can pass through non-record values.
- Keep request bodies, API response data, and supported question shapes otherwise unchanged.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `jev-question-contracts`: make raw-question validation and constructor input behavior explicit.
- `jev-value-conversion`: reject duplicate keys in selected outbound records without lossy JSON conversion.
- `jev-shell-integration`: expose accurate command input/output types and reject ignored root pipeline input.

## Impact

Question parsing, Nu-to-JSON conversion, command signatures, boundary tests, README, and the repository-owned usage skill. No new dependencies or service endpoints are required. The corresponding implementation is currently present as uncommitted worktree changes; this change formalizes and verifies that behavior before synchronization.
