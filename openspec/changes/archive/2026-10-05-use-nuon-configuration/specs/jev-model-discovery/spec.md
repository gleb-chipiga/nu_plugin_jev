# Spec Delta

## RENAMED Requirements

- FROM: `### Requirement: Model discovery selects TOML per invocation`
- TO: `### Requirement: Model discovery selects NUON per invocation`

## MODIFIED Requirements

### Requirement: Model discovery selects NUON per invocation

`jev models` SHALL select and read its local and user NUON configuration for each invocation using the existing live-command precedence. `--config <path>` SHALL select the local NUON file for that invocation, overriding `NU_PLUGIN_JEV_CONFIG` and the implicit local filename. A reused plugin process SHALL NOT retain parsed NUON settings between invocations.

#### Scenario: File changes between listings

- **WHEN** a NUON setting changes between two `jev models` invocations in the same plugin process
- **THEN** the second invocation uses the updated setting without restarting the plugin

#### Scenario: Explicit file selection

- **WHEN** the caller passes `--config <path>` to `jev models` while `NU_PLUGIN_JEV_CONFIG` names another file
- **THEN** the flag-selected file supplies the local NUON layer for that invocation
