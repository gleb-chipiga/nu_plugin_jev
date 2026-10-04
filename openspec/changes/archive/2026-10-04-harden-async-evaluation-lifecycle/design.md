# Design

## Context

See proposal.md for motivation. The current scheduler has one synchronous Nu
input thread and one Tokio supervisor. A bounded channel connects them, but
completion selection can bypass a duplicate already in that channel. HTTP
decoding runs under a timeout; System One answer validation runs afterward.
Tokio cannot abort a running `spawn_blocking` closure or an arbitrary upstream
`Iterator::next()`.

## Goals / Non-Goals

**Goals:** Preserve bounded row admission and current output order, make
single-flight deterministic at the scheduler boundary, enforce one evaluation
deadline, and bound response CPU work retained after cancellation.

**Non-Goals:** No new flags or response fields, no automatic packing, no promise
to cancel remote work or force an uncooperative external iterator to return.

## Decisions

### Linearize deduplication at prepared-row enqueue

The producer's successful channel send is the request-admission point. On a
completion event, the supervisor snapshots the bounded input queue length and
classifies those already queued rows before removing the matching in-flight
group. The existing row classification becomes a shared helper. The snapshot
caps this drain at the channel capacity; continuously arriving rows cannot
starve completions. Simply prioritizing input in the event selector was
rejected because a steady stream of cache hits could starve completions.

### Treat upstream cancellation cooperatively

Check cancellation after acquiring a permit, after `next()`, and before
preparing or sending a row. Keep the producer on its dedicated thread and
never join that thread synchronously from output drop. A Nu `ListStream` checks
its interrupt signal before calling its underlying iterator, not during the
call; the plugin therefore cannot guarantee that a stalled third-party
`next()` returns. Once it does, no row is dispatched.

### Share one deadline across transport and validation

Compute one absolute deadline when an HTTP operation is dispatched. A common
cancel/timeout wrapper encloses status retries, body read, JSON decoding,
answer validation, and success measurement. Retry guidance compares against
that same deadline. `jev models` uses the wrapper too, while retaining its
existing bodyless request and typed decoding.

### Bound non-abortable response CPU work

All HTTP clients in the process share one Tokio semaphore for large-response
decode and validation tasks. Its capacity is the available parallelism capped
at eight, with a fallback of four. Acquire a permit asynchronously before
spawning; move it into the blocking closure so cancellation does not release
the capacity until the work actually stops. An abort-on-drop join wrapper
cancels queued, not-yet-started work. Small responses keep the existing inline
path, avoiding task overhead. This bounds abandoned blocking closures without
claiming that an already running closure can be interrupted.

### Validate capacity and shorten pool locking

Require `2 * jobs <= Semaphore::MAX_PERMITS` during configuration resolution,
before stream creation. On an alternate-client cache miss, construct the
client outside the mutex, reacquire it, and reuse any client inserted by a
concurrent invocation before inserting the new one. Existing active clients
remain valid after LRU eviction.

## Risks / Trade-offs

- **Extra response CPU queueing** -> The shared limit may increase latency for
  many simultaneous large responses; eight-or-fewer CPU tasks cap retained
  work while ordinary HTTP concurrency remains controlled by `jobs`.
- **Upstream stalls after cancellation** -> HTTP and output stop promptly, but
  the input thread can remain until the external `next()` returns. Document
  this boundary and test that no row is dispatched afterward.
- **Completion-boundary regression** -> Use a gated test to enqueue a duplicate
  while the first operation is active and release both events together; assert
  one request for cache-ineligible success and error outcomes.

## Migration Plan

No user migration is required. Keep command signatures and output schemas
unchanged. Update the cancellation note in README and the usage skill with the
implementation. Validate and archive the OpenSpec change with spec sync after
the regression and project checks pass.
