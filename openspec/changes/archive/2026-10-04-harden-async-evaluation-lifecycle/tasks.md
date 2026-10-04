# Tasks

## 1. Table scheduler and cancellation

- [x] 1.1 Register queued duplicate rows before retiring a completed in-flight group; verify deterministic success/error boundary tests make one HTTP request and share the outcome.
- [x] 1.2 Check cancellation around upstream row reads and prevent post-cancel dispatch; verify gated upstream tests for both interrupt and output drop.
- [x] 1.3 Document the uninterruptible external `next()` boundary in README and the usage skill; verify both describe the implemented cancellation behavior without weakening HTTP cancellation.

## 2. HTTP lifetime and client pools

- [x] 2.1 Enforce one deadline through retries, decoding, and contract validation for evaluations and model listing; verify controlled validation-timeout and cancellation tests return no late success.
- [x] 2.2 Offload large response processing from Tokio workers and abort queued work on dropped waits; verify deadline, cancellation, and no late success with stalled blocking-task tests.
- [x] 2.3 Construct alternate HTTP clients outside the pool mutex and double-check before insertion; verify concurrent policy selection reuses a bounded cached client.

## 3. Scheduler configuration

- [x] 3.1 Reject selected `jobs` above the scheduler's permit capacity before input consumption; verify the boundary and an excessive value produce a configuration error without panic.

## 4. Integration verification

- [x] 4.1 Run `cargo fmt`, `cargo clippy --all-targets --all-features`, `cargo nextest run --all-features --all-targets --locked`, and strict OpenSpec validation; verify clean results and no affected-spec INFO findings.
