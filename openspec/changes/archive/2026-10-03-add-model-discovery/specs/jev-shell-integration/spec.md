# Spec Delta

## MODIFIED Requirements

### Requirement: Consistent plugin identity and discovery

The project SHALL use `nu_plugin_jev` as its repository, Cargo crate, and binary name, and SHALL expose `jev` as its Nushell namespace. Registration SHALL support `plugin add <binary-path>` followed by `plugin use jev`. The complete registered command inventory SHALL be `jev`, `jev ask`, `jev annotate`, `jev models`, `jev question noul`, `jev question choice`, and `jev question score`. Scalar projection and filtering SHALL remain ordinary Nu operations; this change SHALL NOT register `jev noul`, `jev choice`, `jev score`, or `jev where`.

#### Scenario: Register and inspect the plugin

- **WHEN** the installed binary is registered and loaded in a compatible Nu session
- **THEN** all specified commands are available through the `jev` namespace
- **AND** `help jev` and command-specific help describe their inputs, outputs, and flags

#### Scenario: Minimal command inventory

- **WHEN** the loaded plugin's command names are inspected
- **THEN** exactly the seven declared commands are registered
- **AND** there are no scalar shortcut or semantic-filter commands

#### Scenario: Root command is offline

- **WHEN** `jev` is invoked without credentials
- **THEN** it returns usage guidance without sending an HTTP request
