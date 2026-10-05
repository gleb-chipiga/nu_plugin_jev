# jev-model-discovery Specification

## Purpose

Let Nushell users discover the model names exposed to their TypeSafe account as ordinary tabular data without changing how evaluations select models.

## Requirements

### Requirement: List model metadata as Nu rows

On validated success, `jev models` SHALL return `{models: <authenticated service entries>, meta: {base_url: <selected validated root>}}`, even without `--metrics`. Each entry SHALL have string `name`, `description`, and `release_date`. Order and string values, including opaque dates, SHALL be preserved; an empty list SHALL remain empty. Rows SHALL NOT repeat `base_url` or fabricate `model` or `usage`; the outer metadata field SHALL always be `meta`.

#### Scenario: Native table operations

- **WHEN** the service returns two model entries and the caller runs `jev models`
- **THEN** `result.models` contains two records with the returned names, descriptions, and release-date strings in service order
- **AND** native `where`, `select`, and `sort-by` can operate on them after `get models`

#### Scenario: Empty account listing

- **WHEN** the service returns `{models: []}`
- **THEN** `jev models` returns `{models: [], meta: {base_url: <selected root>}}` rather than a fabricated default model

#### Scenario: Unflagged listing retains provenance

- **WHEN** the caller runs `jev models` without `--metrics`
- **THEN** the command returns `{models: <ordinary list>, meta: {base_url: <selected root>}}` without `metrics`
- **AND** native row processing uses `jev models | get models`

#### Scenario: Fixed metadata destination

- **WHEN** the caller runs `jev models` and the lookup succeeds
- **THEN** the result contains `meta: {base_url: <selected root>}` beside `models`

### Requirement: Model discovery rejects irrelevant input

`jev models` SHALL accept no state or questions. Nonempty pipeline input SHALL fail before consuming a stream or contacting the service. The command SHALL expose `--base-url`, `--timeout`, and `--config` for applicable settings, without requiring a model name, table scheduling settings, or question-related flags.

#### Scenario: Input is not silently ignored

- **WHEN** a nonempty list stream or other state is piped into `jev models`
- **THEN** the command rejects it before consuming the stream or sending an HTTP request

### Requirement: Optional model-list metrics

With `--metrics`, success SHALL also return `metrics: {request_bytes, response_bytes, elapsed, attempt_elapsed, attempts, http_version}`; the bodyless GET SHALL report zero request bytes. `base_url` SHALL remain only in `meta`. Failures SHALL return errors without fabricated success fields. `--metrics` SHALL NOT enable caching, select a model, or send another API request.

#### Scenario: Listing with metrics

- **WHEN** the service returns two models and the caller runs `jev models --metrics`
- **THEN** `result.models` contains those two ordinary model records in service order
- **AND** `result.meta.base_url` names the selected root while `result.metrics.request_bytes` is zero and its other fields describe the completed lookup
- **AND** no model row has a `base_url` field

#### Scenario: Empty listing still has metrics

- **WHEN** the service returns `{models: []}` and the caller runs `jev models --metrics`
- **THEN** the result is `{models: [], meta: {base_url: <selected root>}, metrics: <successful lookup measurements>}`

#### Scenario: Invalid input and failed lookup have no success metrics

- **WHEN** pipeline input is supplied or the lookup fails before a validated model-list response exists
- **THEN** the command returns the existing error without a `models`/`meta`/`metrics` success envelope

### Requirement: Model discovery selects NUON per invocation

`jev models` SHALL select and read its local and user NUON configuration for each invocation using the existing live-command precedence. `--config <path>` SHALL select the local NUON file for that invocation, overriding `NU_PLUGIN_JEV_CONFIG` and the implicit local filename. A reused plugin process SHALL NOT retain parsed NUON settings between invocations.

#### Scenario: File changes between listings

- **WHEN** a NUON setting changes between two `jev models` invocations in the same plugin process
- **THEN** the second invocation uses the updated setting without restarting the plugin

#### Scenario: Explicit file selection

- **WHEN** the caller passes `--config <path>` to `jev models` while `NU_PLUGIN_JEV_CONFIG` names another file
- **THEN** the flag-selected file supplies the local NUON layer for that invocation

### Requirement: Model discovery is informational and fresh

Each `jev models` invocation SHALL make its own logical listing request and SHALL NOT return a process-cached model list. The result SHALL NOT update the configured model, automatically select a model, or cause `jev ask` or `jev annotate` to preflight their model names against the list. Existing evaluation calls SHALL make no additional models request.

#### Scenario: Listing changes in a reused plugin process

- **WHEN** the service returns different model entries on two `jev models` invocations using one plugin process
- **THEN** each invocation returns its own service response without a plugin restart

#### Scenario: Evaluation remains independent

- **WHEN** the caller invokes `jev ask` or `jev annotate` without first listing models
- **THEN** the evaluation follows its existing model-selection behavior without a `GET /v1/models` preflight
