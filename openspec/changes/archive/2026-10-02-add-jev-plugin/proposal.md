# Proposal

## Why

Nushell can already build structured context and transform tables, but it lacks a native primitive for asking TypeSafe Jev for typed probabilistic decisions. `nu_plugin_jev` will connect ordinary Nu values to System One while keeping context construction, thresholds, sorting, joins, and other data processing in Nushell.

## What Changes

- Add the Rust binary and Cargo crate `nu_plugin_jev`, exposing the `jev` command namespace through the standard Nushell plugin registration flow.
- Add a minimal command surface: offline `jev` guidance, `jev ask` for one state and multiple named questions, `jev annotate` for rows, and offline `jev question noul|choice|score` constructors.
- Preserve structured JSON state and the API's typed answers; add explicit context wrapping, validation, per-setting configuration precedence, and credential handling. Optional per-user `nu_plugin_jev/config.toml` and an automatically discovered caller-directory `.nu_plugin_jev.toml` provide file-based defaults and API-key fallback. Plugin-specific environment settings use `NU_PLUGIN_JEV_*`; the service credential remains `TYPESAFE_API_KEY`.
- Add streaming `jev annotate` with cell-path state selection or explicit `--fields` projection, preserving complete source rows while sending only selected data; include bounded scheduling, ordered or unordered output, cancellation, and row error policies.
- Coalesce identical in-flight evaluations and reuse successful results through an invocation-local LRU cache bounded by both entry count and approximate bytes. Eviction permits a fresh evaluation; failures are not cached. Preserve request identities for interpreting shared usage metadata.
- Add exact request-body dry runs, retry handling, HTTP/2 negotiation, automatic standard HTTP/SOCKS proxy discovery with a Jev-specific override, local mock-server coverage, plugin command tests, and installation and pipeline documentation.
- Add a repository-owned usage skill for composing Jev/Nushell pipelines, maintained alongside user-facing plugin behavior and documentation.
- Keep independent row-state semantics: each distinct request is evaluated separately, subject to deduplication and retries. Client-side packing into a shared state is technically possible without a batch endpoint, but is outside this change because it changes the context visible to each decision and needs separate quality, cost, and failure-handling validation.
- Use native Nu field access for scalar projections and native `where`, `sort-by`, `group-by`, and other commands after annotation. Exclude `jev noul|choice|score`, `jev where`, and `jev models` from this change.
- Keep the scope focused: no context DSL, closure execution, semantic sort/group/count commands, agent orchestration, summarization, embeddings, image input, persistent cache, or automatic row packing.

## Capabilities

### New Capabilities

- `jev-shell-integration`: Plugin identity, command discovery, Nushell compatibility, configuration, and credentials.
- `jev-value-conversion`: Recursive Nu/JSON conversion, supported special values, state admissibility, and context wrapping.
- `jev-question-contracts`: Named typed questions, offline constructors, and schema-aligned field validation.
- `jev-system-one-commands`: Single-state evaluations with complete typed answer envelopes, native Nu field access, and exact dry runs.
- `jev-http-transport`: Reusable authenticated HTTP clients, HTTP/2 negotiation, proxy routing, typed responses, deadlines, retry policy, and transport errors.
- `jev-table-streaming`: Row-preserving annotation for native downstream processing, explicit state selection/projection, independent evaluations, bounded ordered processing, cancellation, row failures, single-flight deduplication, bounded result caching, and usage metadata.

### Modified Capabilities

None. These capabilities remain part of the open change; the interim configuration-name migration is tracked within it rather than as a separate capability.

## Impact

Implementation will introduce `Cargo.toml`, `Cargo.lock`, `README.md`, a repository-owned skill at `skills/jev-nushell/SKILL.md`, binary modules under `src/`, and tests. Direct Nushell dependencies will be the public `nu-plugin` and `nu-protocol` crates, with matching `nu-plugin-test-support` for tests, targeting Nu `0.116.x`. Other dependencies cover HTTP, asynchronous execution, serialization, TOML parsing, cancellation, errors, and non-blocking tracing. File-backed configuration is read per invocation from the calling Nu context; the persistent plugin process retains HTTP clients and its runtime, not one process-global working directory or configuration snapshot.

Live `jev ask` and `jev annotate` invocations send the explicitly constructed state and questions to TypeSafe's authenticated System One endpoint; offline constructors and dry runs do not send requests. The current working implementation uses interim environment names and implicit TOML paths; existing local installations need to rename those settings or move those files. `TYPESAFE_API_KEY`, `$env.config.plugins.jev`, and explicit `--config` remain unchanged.
