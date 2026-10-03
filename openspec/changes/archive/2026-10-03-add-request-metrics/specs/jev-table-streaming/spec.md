# Spec Delta

## MODIFIED Requirements

### Requirement: Output destinations do not overwrite input data

The answer destination selected by `--into` SHALL be a nonempty literal name distinct from fixed `jev_meta` and, when `--metrics` is enabled, fixed `jev_metrics`. An invalid answer destination SHALL fail before input consumption. An existing enabled destination field in a row SHALL cause a terminal field-collision error as soon as that row is read and before its request is constructed or submitted, including when the field is excluded from outbound state. The collision SHALL stop input admission, cancel outstanding local evaluations, and terminate the stream independently of `--on-error fail|keep|record` or ordered output; the conflicting row SHALL NOT be returned unchanged or with `jev_error`. Rows already delivered cannot be rolled back, and the plugin SHALL NOT materialize the stream to search for later collisions. Error-record insertion SHALL NOT overwrite an existing `jev_error`; inability to attach an error record SHALL terminate with an error. With `--on-error record`, the answer destination SHALL differ from `jev_error`.

#### Scenario: Annotation field collision

- **WHEN** an input row already has the configured annotation field
- **THEN** the stream terminates with a field-collision error before that row is sent to the service, even with `--on-error keep` or `record`
- **AND** its existing field is not overwritten or emitted as if it contained a new Jev answer

#### Scenario: Identical annotation and metadata destinations

- **WHEN** `--into jev_meta` is supplied
- **THEN** invocation fails before consuming input rows because the answer and metadata destinations coincide

#### Scenario: Removed metadata switch

- **WHEN** the formerly supported `--meta ai` is supplied
- **THEN** invocation rejects that switch before consuming input rows; successful live rows instead receive fixed `jev_meta`

#### Scenario: Identical answer and metrics destinations

- **WHEN** `--into jev_metrics --metrics` is supplied
- **THEN** invocation fails before consuming input rows because the answer and enabled metrics destinations coincide

#### Scenario: Disabled metrics destination is not reserved for metrics

- **WHEN** `--into jev_metrics` is supplied without `--metrics`
- **THEN** the name is accepted as the answer destination if it does not collide with a source field

#### Scenario: Metrics field collision

- **WHEN** a row already contains the enabled `jev_metrics` destination, even if `--fields` excludes that source field
- **THEN** the stream terminates with a field-collision error before that row is sent to the service, without overwriting its field

#### Scenario: Meta field collision

- **WHEN** a row already contains the fixed `jev_meta` destination, even if `--fields` excludes it
- **THEN** the stream terminates with a field-collision error before that row is sent to the service, even without `--metrics`

#### Scenario: Later collision is not held behind an earlier request

- **WHEN** an earlier row's evaluation is stalled and a later row has an enabled output destination in its source record
- **THEN** the collision stops admission, cancels outstanding local work, and terminates without waiting for the earlier row's ordered result
- **AND** no request is submitted for the conflicting row

#### Scenario: Error destination remains reserved

- **WHEN** `--on-error record` is supplied with `--into jev_error`
- **THEN** invocation fails before consuming input rows

### Requirement: Keep and record row failure policies

With `--on-error keep`, an ordinary failed row SHALL be returned unchanged. With `--on-error record`, an ordinary failed record SHALL be preserved with `jev_error: {kind, message, status}`, where non-HTTP status is null. Failed rows SHALL receive no new annotation or metadata. Non-record rows SHALL be errors; `keep` SHALL return them unchanged, while `record` SHALL terminate because it cannot attach a field. Output-destination collisions and an existing `jev_error` field that prevents recording SHALL instead terminate the invocation immediately, regardless of `keep` or `record`; neither policy SHALL pass through a conflicting row as an apparent success.

#### Scenario: Annotation keeps a failed row

- **WHEN** an HTTP error occurs with `--on-error keep`
- **THEN** the original row is returned without new decision or metadata fields

#### Scenario: Annotation records a failed judgment

- **WHEN** a row cannot be evaluated for a non-collision reason with `--on-error record`
- **THEN** the row is included with `jev_error`
- **AND** the error record identifies the category, message, and HTTP status or null

#### Scenario: Keep and record do not suppress destination collisions

- **WHEN** a source row already contains an enabled output destination and `--on-error keep` or `record` is selected
- **THEN** the stream terminates at that row without returning it as an unchanged or error-bearing record

### Requirement: Streaming request previews

`--dry-run` on `jev annotate` SHALL emit one `{request: <exact request body>, request_bytes: <integer>}` record per valid input row in input order without collecting the whole table, requiring credentials, making HTTP calls, or collapsing duplicates. `request_bytes` SHALL equal the compact UTF-8 JSON serialization length of the nested body, excluding the preview wrapper and HTTP transport overhead. Previewed state SHALL respect cell paths, explicit field projection, and context. The wrapper SHALL not pretend that an API response or live `jev_meta` or `jev_metrics` exists. `--metrics` SHALL be rejected before input consumption in preview mode. Ordinary row validation failures SHALL follow `fail`, `keep`, or `record`; destination collisions SHALL remain terminal before a request preview is emitted for the conflicting row. Documentation SHALL explain that keep/record previews can therefore contain unchanged or error-bearing source rows for non-collision failures among preview wrappers. The output contract SHALL not depend on the `token-estimation` Cargo feature.

#### Scenario: Duplicate rows in preview

- **WHEN** two identical valid rows are processed with `--dry-run`
- **THEN** two corresponding preview wrappers are emitted in input order and no request is submitted

#### Scenario: Named questions in annotation preview

- **WHEN** a valid `jev annotate` invocation with Noul, Choice, and Score questions is run with `--dry-run`
- **THEN** each nested request body contains its selected state and all supplied questions under their original names
- **AND** no decisions or synthetic annotation fields are fabricated

#### Scenario: Projected preview matches the live request

- **WHEN** annotation is run first with `--fields [message sender] --context $policy --dry-run` and then against a mock service with identical inputs and settings
- **THEN** each preview's `request` is structurally identical to the corresponding captured live request
- **AND** each `request_bytes` equals that request's captured JSON body byte length
- **AND** excluded source fields never appear in either body

#### Scenario: Failed row keeps its existing preview policy

- **WHEN** one row lacks a projected field and `--on-error record` is selected
- **THEN** that row receives `jev_error` without a fabricated request or byte count
- **AND** other valid rows continue producing preview wrappers

#### Scenario: Preview collision is terminal

- **WHEN** a row already has the configured answer destination and `jev annotate --dry-run --on-error keep` is used
- **THEN** preview processing terminates at that row without returning it unchanged or emitting a request wrapper for it

#### Scenario: Preview does not fabricate live fields

- **WHEN** a valid row is processed with `jev annotate --dry-run`
- **THEN** its preview wrapper contains `request` and `request_bytes`, but no `jev_meta` or `jev_metrics`

#### Scenario: Downstream truncates preview

- **WHEN** a long input is processed with `jev annotate --dry-run | first 10`
- **THEN** only a bounded prefix is consumed and wrapped
- **AND** no API request is sent

## ADDED Requirements

### Requirement: Always-present `jev_meta` and optional `jev_metrics` on annotations

Every successfully evaluated live `jev annotate` row SHALL preserve its complete source record and add answers plus fixed `jev_meta: {base_url, model, usage}`. `base_url` SHALL be the selected validated service root; `model` and `usage` SHALL come from the validated API response. `--metrics` SHALL additionally add fixed `jev_metrics: {request_id, request_bytes, response_bytes, elapsed, attempt_elapsed, attempts, http_version}`. The old `--meta` flag SHALL be rejected. `request_id` is a local identity for the logical evaluation, not a server idempotency key or billing receipt. Neither `base_url`, `model`, nor `usage` SHALL be repeated in `jev_metrics`. Rows sharing an in-flight or cached evaluation SHALL reuse provenance, measurements, and request identity; retries SHALL retain that identity, while re-evaluation after eviction SHALL receive a new one. Documentation SHALL instruct users to enable `--metrics` and aggregate usage and body sizes once per distinct `request_id` when accounting for shared evaluations. `--metrics` SHALL fail before input consumption when combined with `--dry-run` or, when available, `--estimate-tokens`.

#### Scenario: Default metadata without metrics

- **WHEN** a row is successfully annotated without `--metrics`
- **THEN** the original row and answers remain intact beside `jev_meta: {base_url, model, usage}`
- **AND** no `jev_metrics` field is added

#### Scenario: Metadata destination is fixed

- **WHEN** a row is successfully annotated with a custom answer `--into ai`
- **THEN** its selected root, returned model, and usage remain under `jev_meta` beside `ai`

#### Scenario: Default metrics destination

- **WHEN** a row is successfully annotated with `--metrics`
- **THEN** `jev_meta` contains the selected root, returned model, and usage
- **AND** `jev_metrics` contains local request identity and HTTP measurements without `base_url`, `model`, or `usage`

#### Scenario: Metrics destination is fixed

- **WHEN** a row is successfully annotated with `--metrics --into ai`
- **THEN** local request identity and HTTP measurements remain under `jev_metrics`, while provenance remains under `jev_meta` and answers appear under `ai`

#### Scenario: Metrics destination collides with source data

- **WHEN** a row already has `jev_metrics` and `--metrics` enables that destination
- **THEN** the stream terminates immediately with a field-collision error without overwriting source data or sending that row

#### Scenario: Duplicate rows share one evaluation's metrics

- **WHEN** two rows join one in-flight evaluation or reuse its completed cache entry with `--metrics`
- **THEN** their `jev_meta` records contain the same model and usage, and their `jev_metrics` records contain the same `request_id` and HTTP measurements
- **AND** both durations refer to that one shared evaluation, not the later rows' time waiting or cache lookup
- **AND** documentation tells callers to count usage and body sizes once per distinct `request_id`, not once per row

#### Scenario: Re-evaluation after eviction has new identity

- **WHEN** an identical request is evaluated again after its completed-cache entry was evicted
- **THEN** its `jev_metrics.request_id` differs from the evicted evaluation's identity
- **AND** usage and body sizes from both logical evaluations remain countable

#### Scenario: Failed row has no success metrics

- **WHEN** an ordinary row failure is handled under `--on-error keep` or `record`
- **THEN** that row receives neither new answers nor fabricated `jev_meta` or `jev_metrics` fields

#### Scenario: Metrics reject offline modes

- **WHEN** `--metrics` is combined with `--dry-run` or an available `--estimate-tokens` switch
- **THEN** invocation fails before consuming input rows or sending HTTP

## REMOVED Requirements

### Requirement: Optional metadata identifies shared usage

**Reason**: Provenance is now present on every successful live row under fixed `jev_meta`, independent of optional HTTP measurements. The old `--meta <name>` flag is unnecessary when metadata is always included.

**Migration**: Remove `jev annotate --meta <name>` and read model and usage from fixed `jev_meta`. The local `request_id` moves to separate, optional `jev_metrics` and requires `--metrics`. Neither metadata nor metrics destination can be renamed in `annotate`.
