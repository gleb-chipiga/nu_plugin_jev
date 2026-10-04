# Tasks

## 1. Bound successful HTTP bodies

- [x] 1.1 Enforce the 16 MiB per-response limit for both endpoints before JSON decoding, with early `Content-Length` rejection, incremental body checks, and a redacted nonretryable response error; verify declared, chunked, and exact-boundary cases with local mock tests.
- [x] 1.2 Document the compatibility limit in README, the repository usage skill, and the changelog; verify all three state the same threshold and error behavior.

## 2. Reuse static table inputs

- [x] 2.1 Share typed questions per annotation invocation and convert valid static context once while preserving invalid-context row-error precedence; verify exact prepared request bytes, dry-run/live equivalence, and existing cache/stream tests.
- [x] 2.2 Run a paired local benchmark of legacy and reused row preparation for small and large fixtures, verify byte-for-byte request equivalence before timing, and report median construction and prepared-body times over repeated 1000-row runs.

## 3. Integration verification

- [x] 3.1 Run `cargo fmt`, `cargo clippy --all-targets --all-features`, `cargo nextest run --all-features --all-targets --locked`, and strict OpenSpec validation; verify no main spec was edited directly and report any remaining failures.
