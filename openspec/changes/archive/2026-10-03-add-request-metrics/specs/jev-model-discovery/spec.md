# Spec Delta

## MODIFIED Requirements

### Requirement: List model metadata as Nu rows

`jev models` SHALL accept no state or questions and SHALL return `{models: <authenticated service's model entries>, meta: {base_url: <selected validated service root>}}` on validated success, even without `--metrics`. Every model entry SHALL remain a record with string `name`, `description`, and `release_date` fields. The `models` list SHALL preserve the service's entry order and string values, including opaque release-date text; an empty service list SHALL remain `models: []`. Individual model records SHALL NOT repeat `base_url`, and the lookup SHALL NOT fabricate `model` or `usage`. Nonempty pipeline input SHALL be rejected without consuming a stream or contacting the service. The command SHALL expose `--base-url`, `--timeout`, and `--config` for applicable existing settings, but SHALL NOT require a model name, table scheduling settings, or question-related flags. The outer metadata destination SHALL always be `meta`. With `--metrics`, the result SHALL additionally contain `metrics: {request_bytes, response_bytes, elapsed, attempt_elapsed, attempts, http_version}`; the bodyless GET SHALL report `request_bytes: 0`. The selected `base_url` SHALL appear only in `meta`, not inside `metrics`. Failures SHALL return errors without fabricated success fields. `--metrics` SHALL NOT enable caching, a model-selection side effect, or an extra API request.

#### Scenario: Native table operations

- **WHEN** the service returns two model entries and the caller runs `jev models`
- **THEN** `result.models` contains two records with the returned names, descriptions, and release-date strings in service order
- **AND** native `where`, `select`, and `sort-by` can operate on them after `get models`

#### Scenario: Empty account listing

- **WHEN** the service returns `{models: []}`
- **THEN** `jev models` returns `{models: [], meta: {base_url: <selected root>}}` rather than a fabricated default model

#### Scenario: Input is not silently ignored

- **WHEN** a nonempty list stream or other state is piped into `jev models`
- **THEN** the command rejects it before consuming the stream or sending an HTTP request

#### Scenario: Listing with metrics

- **WHEN** the service returns two models and the caller runs `jev models --metrics`
- **THEN** `result.models` contains those two ordinary model records in service order
- **AND** `result.meta.base_url` names the selected root while `result.metrics.request_bytes` is zero and its other fields describe the completed lookup
- **AND** no model row has a `base_url` field

#### Scenario: Empty listing still has metrics

- **WHEN** the service returns `{models: []}` and the caller runs `jev models --metrics`
- **THEN** the result is `{models: [], meta: {base_url: <selected root>}, metrics: <successful lookup measurements>}`

#### Scenario: Unflagged listing retains provenance

- **WHEN** the caller runs `jev models` without `--metrics`
- **THEN** the command returns `{models: <ordinary list>, meta: {base_url: <selected root>}}` without `metrics`
- **AND** native row processing uses `jev models | get models`

#### Scenario: Fixed metadata destination

- **WHEN** the caller runs `jev models` and the lookup succeeds
- **THEN** the result contains `meta: {base_url: <selected root>}` beside `models`

#### Scenario: Invalid input and failed lookup have no success metrics

- **WHEN** pipeline input is supplied or the lookup fails before a validated model-list response exists
- **THEN** the command returns the existing error without a `models`/`meta`/`metrics` success envelope
