# Tasks

## 1. Exact request-body previews

- [x] 1.1 Add a feature-independent compact-JSON byte counter for typed System One requests and verify its result equals live encoded-body length for nested state, mixed questions, UTF-8, and escaped characters in unit and mock-HTTP tests.
- [x] 1.2 Wrap successful `jev ask --dry-run` and lazy `jev annotate --dry-run` results as `{request, request_bytes}` while retaining ordinary invalid-row policies and terminal destination collisions; verify keyless, duplicate, projected-state, collision, cancellation, and `--no-default-features` preview tests.
- [x] 1.3 Update dry-run help, README, usage skill, and changelog for `get request` migration and body-byte scope; verify all documented examples match command output and run `cargo fmt`, Clippy, and nextest for this stage.

## 2. Successful HTTP measurement

- [x] 2.1 Return typed success provenance containing the selected normalized `base_url` plus a separate HTTP measurement for every validated System One or model-list success, regardless of `--metrics`; verify bodyless GET reports `request_bytes: 0`, both operations measure only the final successful response body, both durations and explicit attempts obey retry/deadline boundaries, and HTTP/1.1, HTTP/2, explicit roots, malformed responses, timeout, cancellation, and redaction do not change retry behavior.
- [x] 2.2 Preserve each success's `base_url`, response model/usage, measurement, and `request_id` in the shared in-flight/completed annotation result; verify duplicate and cache-hit rows reuse identical `jev_meta`, optional `jev_metrics`, and one request identity while errors cache no success fields.
- [x] 2.3 Add `base_url`, measured byte/count/version fields, and both durations as `elapsed_ns` and `attempt_elapsed_ns` to successful `info` evaluation and model-list completion events, leaving `duration_ms` distinct; verify isolated-process tests cover unflagged success, parity with returned `meta.base_url` on `ask`/`models` or `jev_meta.base_url` on `annotate` and their respective optional metric durations, retries, duplicate/cache reuse, GET bodylessness, and no successful measurements on failure or offline paths.
- [x] 2.4 Document both completion events, their `info` level, visible selected service root, total-versus-final-attempt timing boundary, body-byte scope, and one-event-per-HTTP-operation semantics in README and the usage skill; verify examples against captured diagnostics and run `cargo fmt`, Clippy, and nextest for this stage.

## 3. Opt-in live command output

- [x] 3.1 Make successful live `jev ask` return `{answers, meta: {base_url, model, usage}}` with optional separate HTTP-only `metrics`; verify original API answer fields remain typed, both Nu durations and byte/count/version measurements are correct, and `--metrics` with `--dry-run`/`--estimate-tokens` fails early.
- [x] 3.2 Replace `jev annotate --meta` with always-present fixed `jev_meta: {base_url, model, usage}` and optional `--metrics` adding fixed `jev_metrics` with HTTP measurements plus local `request_id`. Verify answer-name conflicts fail before input consumption, source collisions terminate before HTTP despite `--on-error keep|record`, a later collision overtakes a stalled earlier row and cancels outstanding work, and normal/unordered/duplicate/cache/retry behavior and offline-mode rejection remain correct.
- [x] 3.3 Make successful `jev models` return `{models, meta: {base_url}}` with optional separate `metrics` and report `request_bytes: 0` for GET; verify destination overrides are unavailable, unflagged, empty, and multiple model lists, retries, native `get models` table operations, one GET per logical lookup, input rejection, and no fabricated success fields on errors in mock and real-Nu tests.
- [x] 3.4 Update flag help, README, usage skill, and changelog with the three command shapes, removal of annotation `--meta` without a destination override, always-visible `base_url` in `meta` on `ask`/`models` or `jev_meta` on `annotate`, `ask` model/usage path migration, default `models | get models` migration, GET bodylessness, byte boundaries, both timing intervals, HTTP version meaning, and distinct-`request_id` aggregation guidance; verify examples against mock responses and run `cargo fmt`, Clippy, and nextest for this stage.

## 4. Cross-path verification

- [x] 4.1 Archive or sync completed `add-model-discovery` before archiving this change so the MODIFIED model-list requirement replaces its durable bare-list contract. Run `cargo fmt --check`, `cargo clippy --all-targets --all-features`, `cargo nextest run --all-features --all-targets --locked`, relevant no-default-feature checks, and `openspec validate --all --strict --no-interactive`; verify selected `base_url` consistency across POST and GET under each command's metadata field, separation from optional metrics, preview/body-size parity, and all new unflagged live output shapes across the combined changes.
