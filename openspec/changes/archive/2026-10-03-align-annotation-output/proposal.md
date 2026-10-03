# Proposal

## Why

`jev ask` exposes decisions under `answers`, while `jev annotate` defaults to `jev`. The mismatch makes equivalent single-state and row-wise Nu pipelines harder to read and reuse.

## What Changes

- **BREAKING**: Make `answers` the default top-level destination for successful `jev annotate` rows. Keep `--into <name>` for an explicit alternative.
- Preserve streaming, deduplication, and offline-preview behavior. Apply the terminal output-destination collision rule from `add-request-metrics` to the new default `answers` field: `--on-error keep|record` must not pass a conflicting source row through. That separate change owns fixed, always-present `jev_meta`, optional fixed `jev_metrics` via `--metrics`, and removal of the old `--meta` flag; annotation has no `--meta-into` or `--metrics-into`.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `jev-table-streaming`: Align the default annotation answer destination with the single-state `ask` result.

## Impact

This changes the default answer path in `jev annotate`. It affects annotation row construction, command help, tests, README, the repository usage skill, and changelog. It does not change request bodies, HTTP transport, or the `ask`/`models` results defined by `add-request-metrics`. With that change applied first, successful live rows contain `answers` and always-present `jev_meta`, plus optional `jev_metrics`.
