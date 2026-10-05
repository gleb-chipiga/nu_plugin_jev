# Proposal

## Why

The plugin already shares a Tokio runtime and reusable HTTP clients across Nu
commands, but live command overlap is not an explicit, end-to-end tested
contract. Per-invocation `--jobs` also does not bound the combined HTTP activity
of concurrent calls.

## What Changes

- Guarantee concurrent command execution through native Nu facilities such as
  `par-each`, without a plugin-specific command scheduler or `--parallel` flag.
- Add one process-wide limit for actual HTTP attempts from `jev ask`,
  `jev annotate`, and `jev models`, including retries and all client policies.
- Resolve the attempt limit once at plugin startup: process environment
  `NU_PLUGIN_JEV_MAX_IN_FLIGHT`, local NUON `max_in_flight`, user NUON
  `max_in_flight`, then **128**. Reject an invalid selected value at startup;
  changes require a plugin restart, not another invocation or file edit.
- Acquire one slot per attempt and release it after body acquisition or attempt
  termination, before decoding, validation, retry pauses, or output delivery.
  Waiting remains asynchronous, locally cancellable, and covered by the
  existing logical-operation deadline.
- Preserve HTTP measurement boundaries and response schemas while documenting
  where slot waiting belongs in existing durations.
- Keep `--jobs` invocation-local with its unchanged default of **16**. Preserve
  bounded row admission, ordering, and invocation-local deduplication and LRU.
- Keep dispatched HTTP operations and their deadlines progressing independently
  of a full, open output channel; retain both task and row-admission bounds.
- Close the interrupt-registration race and retain each handler for its full
  operation or returned stream lifetime.
- Stop admission and clean up remaining evaluations before awaiting delivery
  of an observed terminal error, preserving its original cause. Cleanup must
  not wait for a blocked synchronous input iterator.
- Prove real Nu command overlap, aggregate limits, independent settings and
  failures, and slot reclamation using the existing local HTTP/2 fixture.

## Capabilities

### New Capabilities

None; this extends existing integration, transport, and streaming contracts.

### Modified Capabilities

- `jev-shell-integration`: Concurrent command execution and isolation, with a
  startup-only process setting distinct from invocation-scoped precedence.
- `jev-http-transport`: Aggregate HTTP attempt admission, permit lifetime,
  cancellation/deadline behavior, and unchanged measurement semantics.
- `jev-table-streaming`: Table-local work bounds continue to apply while
  evaluations wait for the shared network budget; `--jobs` stays unchanged.

## Impact

- Shared state and startup configuration in `src/plugin.rs`, `src/app.rs`,
  and `src/config.rs`; HTTP attempt handling in `src/api/client.rs`.
- Tests in `src/api/client.rs`, `src/nu/stream.rs`, and `tests/real_nu.rs`,
  reusing `tests/support/h2_fixture.rs` rather than a custom HTTP server.
- Brief actionable updates to README, command guidance where needed, the
  repository usage skill, and the unreleased changelog during implementation.
- No new dependencies or Nushell version update; retain `nu-plugin 0.116.0`,
  the existing runtime, client pooling, protocol modes, and response formats.
- Aggregate contention may delay or time out a logical operation before its
  first HTTP attempt. The limit is a local process budget, not a provider quota,
  requests-per-second limiter, memory ceiling, or connection-count limit.
- No cross-invocation cache or deduplication, dynamic budget resizing,
  per-key/per-endpoint budgets, or new commands or flags.
