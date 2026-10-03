# Spec Delta

## Purpose

Let Nushell users discover the model names exposed to their TypeSafe account as ordinary tabular data without changing how evaluations select models.

## ADDED Requirements

### Requirement: List model metadata as Nu rows

`jev models` SHALL accept no state or questions and SHALL return the authenticated service's model entries as a Nu list of records with string `name`, `description`, and `release_date` fields. It SHALL preserve the service's entry order and string values, including the release-date text without coercing it to a Nu date. An empty `models` array SHALL return an empty list. Nonempty pipeline input SHALL be rejected without consuming a stream or contacting the service. The command SHALL expose `--base-url`, `--timeout`, and `--config` for applicable existing settings, but SHALL NOT require a model name, table scheduling settings, or question-related flags.

#### Scenario: Native table operations

- **WHEN** the service returns two model entries and the caller runs `jev models`
- **THEN** Nu receives two records containing the returned names, descriptions, and release-date strings in service order
- **AND** native `where`, `select`, and `sort-by` can operate on those fields

#### Scenario: Empty account listing

- **WHEN** the service returns `{models: []}`
- **THEN** `jev models` returns an empty Nu list rather than a fabricated default model

#### Scenario: Input is not silently ignored

- **WHEN** a nonempty list stream or other state is piped into `jev models`
- **THEN** the command rejects it before consuming the stream or sending an HTTP request

### Requirement: Model discovery selects TOML per invocation

`jev models` SHALL select and read its local and user TOML configuration for each invocation using the existing live-command precedence. `--config <path>` SHALL select the local TOML file for that invocation, overriding `NU_PLUGIN_JEV_CONFIG` and the implicit local filename. A reused plugin process SHALL NOT retain parsed TOML settings between invocations.

#### Scenario: File changes between listings

- **WHEN** a TOML setting changes between two `jev models` invocations in the same plugin process
- **THEN** the second invocation uses the updated setting without restarting the plugin

#### Scenario: Explicit file selection

- **WHEN** the caller passes `--config <path>` to `jev models` while `NU_PLUGIN_JEV_CONFIG` names another file
- **THEN** the flag-selected file supplies the local TOML layer for that invocation

### Requirement: Model discovery is informational and fresh

Each `jev models` invocation SHALL make its own logical listing request and SHALL NOT return a process-cached model list. The result SHALL NOT update the configured model, automatically select a model, or cause `jev ask` or `jev annotate` to preflight their model names against the list. Existing evaluation calls SHALL make no additional models request.

#### Scenario: Listing changes in a reused plugin process

- **WHEN** the service returns different model entries on two `jev models` invocations using one plugin process
- **THEN** each invocation returns its own service response without a plugin restart

#### Scenario: Evaluation remains independent

- **WHEN** the caller invokes `jev ask` or `jev annotate` without first listing models
- **THEN** the evaluation follows its existing model-selection behavior without a `GET /v1/models` preflight
