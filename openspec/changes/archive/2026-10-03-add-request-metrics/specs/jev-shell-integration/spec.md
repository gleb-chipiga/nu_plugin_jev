# Spec Delta

## ADDED Requirements

### Requirement: Correlated successful HTTP measurements in diagnostics

For each successful live System One evaluation or model-list lookup, the plugin SHALL add `request_bytes`, `response_bytes`, `elapsed_ns`, `attempt_elapsed_ns`, and `attempts` as integer fields and `http_version` and `base_url` as string fields to its plugin-owned `evaluation completed` or `model listing completed` tracing event at `info`. The HTTP fields SHALL come from the same successful measurement used for optional `--metrics` output; `base_url` SHALL come from the same selected, validated service root returned in `meta.base_url` on `ask`/`models` or `jev_meta.base_url` on `annotate`, whether or not `--metrics` is present. It SHALL not be the appended endpoint or proxy URL. `elapsed_ns` and `attempt_elapsed_ns` SHALL represent the same intervals as optional `metrics.elapsed`/`metrics.attempt_elapsed` on `ask`/`models`, or `jev_metrics.elapsed`/`jev_metrics.attempt_elapsed` on `annotate`, expressed in nanoseconds; the existing `duration_ms` SHALL remain a separate, broader operation-duration field. Each event SHALL preserve its local `request_id` correlation; evaluation completion retains model and usage, while model listing retains its model count and SHALL NOT fabricate token usage. Apart from the explicitly requested service root, events SHALL contain no state, questions, request/response body, credentials, full request URL, or proxy details. Joining an in-flight evaluation or serving a completed cache entry SHALL NOT create another successful evaluation completion event. Failed, cancelled, dry-run, and offline token-estimation operations SHALL NOT produce fabricated successful measurement fields.

#### Scenario: Unflagged success is measured in tracing

- **WHEN** `jev ask` succeeds with `NU_PLUGIN_JEV_LOG=info` and without `--metrics`
- **THEN** its completion diagnostic includes `base_url`, `request_bytes`, `response_bytes`, `elapsed_ns`, `attempt_elapsed_ns`, `attempts`, and `http_version` under the same local `request_id`
- **AND** the unflagged Nu result includes the same selected root at `meta.base_url`, along with the returned model and usage

#### Scenario: Tracing and returned metrics agree

- **WHEN** `jev ask` succeeds with `--metrics`
- **THEN** the completion event's `base_url` equals returned `meta.base_url`, while its byte counts, attempt count, and HTTP version equal the returned `metrics` fields
- **AND** both nanosecond fields equal their respective returned Nu durations, while `duration_ms` retains its existing separate meaning

#### Scenario: Retry and deduplication do not multiply completions

- **WHEN** a retry succeeds and duplicate annotation rows share that evaluation, including a later completed-cache hit
- **THEN** exactly one successful completion event is produced for the actual evaluation, with `attempts` counting its HTTP attempts and `elapsed_ns` including the retry wait
- **AND** `attempt_elapsed_ns` excludes the earlier attempt and wait while including final response validation
- **AND** no additional completion event is produced for rows that reuse its result
- **AND** with `--metrics`, every reused row's `jev_metrics.request_id` equals that completion event's local `request_id`

#### Scenario: Model-list lookup has the same tracing contract

- **WHEN** `jev models` succeeds with or without `--metrics` and plugin `info` tracing enabled
- **THEN** one `model listing completed` event includes the selected `base_url`, zero `request_bytes`, response bytes, both durations, explicit GET attempt count, and final HTTP version
- **AND** the event's `base_url` equals `meta.base_url`, and with `--metrics` its HTTP fields agree with the returned `metrics`
- **AND** the event preserves its model count without inventing `usage` tokens

#### Scenario: No fabricated successful measurements

- **WHEN** an evaluation fails or is cancelled, or a request is only previewed or token-estimated offline
- **THEN** no successful evaluation completion event with measurement fields is produced
