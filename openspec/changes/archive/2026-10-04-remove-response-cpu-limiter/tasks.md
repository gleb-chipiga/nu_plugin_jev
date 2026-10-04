# Tasks

## 1. Remove the global response-CPU gate

- [x] 1.1 Remove the shared semaphore and client field while keeping large-response offload and abort-on-drop; verify the client builds with `cargo check --all-features --locked`.
- [x] 1.2 Replace permit-dependent tests with deterministic deadline, cancellation, and queued-task-abort tests; verify the affected client tests with nextest.
- [x] 1.3 Sync the removed transport requirement into the canonical spec; verify strict OpenSpec validation has no findings.

## 2. Integration verification

- [x] 2.1 Run `cargo fmt --all`, Clippy for all targets/features, and the full locked nextest suite; verify all checks pass.
