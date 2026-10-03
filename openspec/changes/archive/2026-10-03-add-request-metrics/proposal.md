# Proposal

## Why

Users can inspect a Jev request before sending it, but cannot see its JSON size or the payload size and latency of a completed evaluation or model-list lookup. Successful evaluations should identify the selected service root, returned model, and token usage without an opt-in flag; HTTP measurements remain optional. The three live commands need to keep provenance separate from transport metrics, with plugin-prefixed fields only on annotation rows.

## What Changes

- **BREAKING:** Make each successful `--dry-run` result `{request: <exact request body>, request_bytes: <integer>}`. Consumers of top-level `model`, `state`, or `questions` migrate through `request`.
- **BREAKING:** Return `{answers, meta: {base_url, model, usage}}` from successful live `jev ask`. Successful `jev annotate` rows preserve their source fields and add answers plus fixed `jev_meta: {base_url, model, usage}`. Successful `jev models` returns `{models: <ordinary list>, meta: {base_url}}`, including when the list is empty; direct table processing migrates through `get models`. Metadata destinations are fixed: `meta` for `ask` and `models`, `jev_meta` for `annotate`. The old `jev annotate --meta <name>` flag is removed.
- Add opt-in `--metrics` to all three live commands. It adds a separate `metrics` record on `ask` and `models`, or fixed `jev_metrics` on `annotate`, containing HTTP body sizes, total and final-attempt durations, attempt count, and final HTTP version; annotation also includes the local `request_id` needed to identify shared evaluations. `base_url`, `model`, and `usage` belong to the always-present metadata record (`meta` on `ask` and `models`, `jev_meta` on `annotate`), never to the metrics record. Metrics destinations are fixed.
- Add the measured fields and selected `base_url` to plugin-owned `info` completion tracing events for every successful live evaluation or model-list lookup, even without `--metrics`. Keep one event per actual HTTP operation, not per annotated row or cache hit.
- Report `request_bytes` for one serialized JSON request body (`0` for the bodyless models GET), `response_bytes` for the final successful JSON response body consumed by the client, `elapsed` from the first HTTP attempt through validated success (including retries and waits), `attempt_elapsed` from the final successful attempt's start through that same validation, `attempts` as the explicit HTTP-attempt count, and `http_version` as the final successful response's HTTP protocol version. Both durations are Nu duration values in command output and integer nanoseconds in tracing. Byte fields measure application bodies, not total wire traffic or cumulative retry bytes. No connection-new/reused flag is promised.
- Preserve annotation row ordering, duplicate sharing, cache identity, and `--on-error` behavior for ordinary row failures. Output-destination name conflicts are terminal regardless of `--on-error`: reject conflicting flags before input consumption and stop at the first conflicting source row before its HTTP dispatch. Rows sharing one evaluation reuse its provenance, optional metrics, and request identity; failed rows receive no fabricated success fields.
- Keep dry-run byte measurement available without API credentials or token-estimation support. Keep live metrics opt-in and unavailable in offline `--dry-run` or `--estimate-tokens` modes.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `jev-system-one-commands`: Wrap dry-run output and add always-present live metadata plus optional metrics to `jev ask`.
- `jev-table-streaming`: Wrap streaming previews, add always-present annotation metadata, and keep metrics separate and optional.
- `jev-model-discovery`: Wrap the model list with always-present root metadata and optional lookup metrics.
- `jev-http-transport`: Define the selected service-root provenance, measured HTTP body sizes, elapsed-time boundaries, final response protocol version, and retry/validation semantics.
- `jev-shell-integration`: Expose successful evaluation measurements and service-root provenance as correlated tracing fields.

## Impact

This changes successful live output for `ask`, `annotate`, and `models`, wraps dry-run results, removes the old annotation `--meta` flag, and adds optional metrics and tracing fields. All metadata and metrics destinations are fixed, with plugin-prefixed `jev_meta` and `jev_metrics` only on annotation rows. It affects shared request serialization, response-body decoding and timing, annotation result/cache provenance, tracing, plugin tests, command help, README, CHANGELOG, and `skills/jev-nushell/SKILL.md`. No token model, Cargo feature, or new API endpoint is required. The separate `add-offline-token-estimation` change owns `--estimate-tokens`; the completed `add-model-discovery` change describes the existing bare-list behavior that this change replaces. Archive or sync `add-model-discovery` first so this change's MODIFIED model-list requirement has a durable target.
