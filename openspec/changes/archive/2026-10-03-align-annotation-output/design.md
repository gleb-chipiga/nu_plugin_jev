# Design

## Context

See [proposal.md](proposal.md) for the motivation and [spec.md](specs/jev-table-streaming/spec.md) for the target behavior. Today `jev annotate` defaults to an answer field named `jev`; `jev ask` returns an envelope whose decision field is named `answers`. The separate `add-request-metrics` change owns always-present `jev_meta`, optional `jev_metrics`, and removal of the old `--meta` flag.

## Goals / Non-Goals

**Goals:** Make `answers` the default answer path for both `ask` and annotation, while preserving configurable row destinations without replacing source data.

**Non-Goals:** Change metrics or metadata flags and fields, HTTP behavior, deduplication/streaming semantics, or `ask` and `models` output.

## Decisions

### Change only the default answer destination

Default `--into` to `answers` rather than `jev` when no explicit name is supplied. Keep `--into <name>` as an exact literal field override. The successful row still contains the original source fields plus the selected answer field; it does not acquire a new envelope. The alternative of wrapping every row in a new result object would break native table processing and change the meaning of `--into`.

`add-request-metrics` independently defines fixed, always-present `jev_meta`, optional fixed `jev_metrics` via `--metrics`, and removal of `--meta` without a destination override for annotation. This change neither duplicates nor supersedes those contracts; a successful live row using both changes has `answers` and `jev_meta` as sibling fields, plus `jev_metrics` when requested.

### Apply terminal collision handling to the new destination

Run destination checks against the new default name as soon as each row is read, including when source projection excludes that field. A source `answers` field terminates annotation before its request is constructed or submitted, even with `--on-error keep|record`; the row must not be returned unchanged or with `jev_error`, where downstream could mistake its existing field for a new Jev answer. Stop admission and cancel outstanding local evaluations without waiting for an earlier ordered result. Invalid combinations of output names still fail before consuming any input. Ordinary state and HTTP failures retain `fail`/`keep`/`record` handling. Callers can choose another answer destination with `--into`; this change relies on the general terminal-collision rule owned by `add-request-metrics`.

This is an output-name change only. Ordered and unordered streams, completed-cache hits, retries, cancellation, and backpressure need no new coordination. Dry-run and token-estimation paths remain request previews rather than fabricated answer rows.

## Risks / Trade-offs

- **Default `answers` collides with a common source field** → Terminate immediately when the conflicting row is read; keep `--into` as an explicit alternative and document the migration.
- **Older examples still use `jev.*` paths** → Update help, README, the usage skill, tests, and changelog alongside the implementation; document `--into jev` as a compatibility option.

## Migration Plan

Change old paths such as `jev.spam.noul` to `answers.spam.noul`, or pass `--into jev` for a compatible answer destination. Removal of the old `--meta` flag and separate optional `jev_metrics` are documented and implemented by `add-request-metrics`, not here. Update command help, README, usage skill, changelog, and tests in the same implementation change. No stored data or API-side migration is required; rollback can restore the old default with corresponding documentation and tests.
