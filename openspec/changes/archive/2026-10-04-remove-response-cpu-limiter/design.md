# Design

## Context

The client currently shares a response-CPU semaphore across all HTTP clients.
It is acquired after the body has been read, before large JSON decoding or
answer validation. See [proposal.md](proposal.md) for the reason to remove it.

## Goals / Non-Goals

**Goals:** Remove cross-invocation CPU admission while retaining the existing
response-size bound, per-request deadline, cancellation, and offload of large
CPU work away from Tokio worker threads.

**Non-Goals:** Change `--jobs`, HTTP retry behavior, response limits, or the
thresholds that select offloaded work. Do not add a replacement global limiter.

## Decisions

Remove the static semaphore, client field, and permit acquisition. Keep a small
`spawn_blocking` helper whose join handle is aborted on drop. This avoids
blocking a Tokio worker for large responses and prevents queued blocking work
from starting after its awaiting evaluation is cancelled. A closure already
running on a blocking thread can finish after its waiter is gone; it cannot
publish a late response because the deadline/cancellation wrapper has ended.

Keep deterministic deadline tests by pausing the offloaded closure with a
test-only gate, not by injecting a production CPU limiter. Test that a queued
blocking closure is aborted when its waiter is dropped. Remove tests of shared
permits and their release timing.

## Risks / Trade-offs

- **More simultaneous large decodes across invocations** → Accept this in
  exchange for removing global contention; the 16 MiB body cap and
  per-invocation `--jobs` still constrain individual operations, but neither
  is a process-wide CPU or memory cap.
- **A running blocking closure survives waiter cancellation briefly** → Keep
  responses bounded and never expose its result after cancellation.

## Migration Plan

Apply and test the transport change on `master`, then rebase the
`feat/offline-token-estimation` branch and adapt its test-only calibration
decoder to the new helper. No external API or configuration migration is
required.
