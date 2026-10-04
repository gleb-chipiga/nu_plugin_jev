# Proposal

## Why

An already queued duplicate can miss its in-flight evaluation at the completion
boundary, and response processing can outlive the configured deadline or retain
work after cancellation. The current cancellation promise also exceeds what a
synchronous upstream iterator can guarantee.

## What Changes

- Make request admission and completion ordering precise so queued duplicates
  share one evaluation even when its result cannot be cached.
- Bound the entire HTTP, decoding, and answer-validation operation by one
  deadline, and abort queued response processing when its waiter is cancelled.
- Stop plugin-controlled work promptly on cancellation while documenting the
  unavoidable limit of an upstream iterator blocked inside `next()`.
- Reject scheduler sizes that exceed Tokio's semaphore capacity before reading
  input, and shorten the HTTP client-pool critical section.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `jev-table-streaming`: Define request admission at the scheduler boundary and
  qualify cancellation of an uninterruptible external source.
- `jev-http-transport`: Include decoding and contract validation in the total
  deadline, without returning a late response after cancellation.
- `jev-shell-integration`: Reject an unrepresentable table `jobs` value as a
  configuration error instead of panicking.

## Impact

The row scheduler, HTTP client, and configuration validation change. README and
the repository usage skill receive a concise cancellation note. Commands,
flags, response fields, and normal request behavior remain unchanged.
