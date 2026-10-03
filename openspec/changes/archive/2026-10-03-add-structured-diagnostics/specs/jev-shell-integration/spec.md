# Spec Delta

## ADDED Requirements

### Requirement: Opt-in newline-delimited NUON diagnostics

`NU_PLUGIN_JEV_LOG_FORMAT` SHALL select `text` or `nuon` at plugin process startup, defaulting to the current text format. An invalid explicit format SHALL cause a startup error without writing diagnostic data to the plugin protocol. In `nuon` mode, each diagnostic event actually written to stderr SHALL occupy exactly one physical line containing one compact NUON record. Each record SHALL have `timestamp` (UTC RFC3339 string), `level` (string), `target` (string), `message` (string), `fields` (record), and `spans` (root-to-leaf list of records containing `name` and `fields`). The formatter SHALL preserve supported primitive event and span fields as typed values, render unsupported debug-only values as strings, and escape embedded newlines so one event never occupies multiple physical lines. The plugin SHALL NOT represent a span's creation or closure as an event unless instrumentation explicitly emits one.

The `nuon-tracing-format` Cargo feature SHALL compile the NUON formatter and SHALL be included in the crate's default features. Without that feature, text diagnostics and target filtering SHALL remain available, while an explicit `NU_PLUGIN_JEV_LOG_FORMAT=nuon` SHALL fail startup with a value-redacted feature-unavailable error.

#### Scenario: Parse mixed-source diagnostics in Nu

- **WHEN** plugin-owned `tracing` events and explicitly enabled dependency `log` events are emitted with `NU_PLUGIN_JEV_LOG_FORMAT=nuon`
- **THEN** each plugin-authored diagnostic line is independently parseable with `from nuon` and a stream of those lines is parseable with Nu's `from ndnuon`
- **AND** records identify their originating `target` and `level`

#### Scenario: Preserve request correlation and types

- **WHEN** an evaluation event includes numeric `duration_ms` and an enclosing span contains its local `request_id`
- **THEN** the NUON record retains `duration_ms` as a number and the request identity in `spans`
- **AND** a message containing a newline remains one parseable physical line

#### Scenario: Default presentation and invalid format

- **WHEN** `NU_PLUGIN_JEV_LOG_FORMAT` is unset
- **THEN** diagnostics retain their existing human-readable stderr presentation
- **AND** an invalid explicit format is rejected before serving plugin commands rather than silently selecting another format

#### Scenario: Build without NUON diagnostics

- **WHEN** the crate is built with `--no-default-features`
- **THEN** text diagnostics and target filtering remain available
- **AND** an explicit `NU_PLUGIN_JEV_LOG_FORMAT=nuon` is rejected before serving commands

## MODIFIED Requirements

### Requirement: Process-level evaluation diagnostics

The plugin SHALL initialize non-blocking diagnostics before entering its Tokio runtime and write them to stderr, not to Nu pipeline values or the stdout plugin protocol. `NU_PLUGIN_JEV_LOG` SHALL configure one process-wide target/level filter at plugin startup. When absent, it SHALL enable `nu_plugin_jev=warn` and no dependency targets. The existing bare values `off`, `error`, `warn`, `info`, `debug`, and `trace` SHALL retain their plugin-only meaning. An advanced value of one or more comma-separated `target=level` directives SHALL accept explicit plugin or dependency targets, keep `nu_plugin_jev=warn` unless the plugin base target is explicitly overridden, and leave all unlisted dependency targets disabled. More-specific target directives SHALL override matching broader directives. An invalid explicit filter SHALL cause a startup error rather than silently dropping directives; neither `RUST_LOG` nor legacy `JEV_LOG` SHALL act as a fallback. Rust `log` records and `tracing` events that reach the plugin process SHALL obey the same filter and selected formatter; enabling a target SHALL NOT imply that a dependency emits any particular event or enable additional connection-verbose behavior. Changing the filter or format in an already running plugin SHALL require a process restart. At plugin `info`, logical evaluation starts, completions, and failures SHALL be observable with the local `request_id` and elapsed time when the operation ends. Completion events SHALL include the returned model and usage; failure events SHALL include error kind and HTTP status when available. At plugin `debug`, diagnostics SHALL additionally identify HTTP attempts, retry delays, and table-result reuse without revealing request bodies.

#### Scenario: Correlated request diagnostics

- **WHEN** an evaluation retries before succeeding with `NU_PLUGIN_JEV_LOG=debug`
- **THEN** its attempts and completion diagnostics appear on stderr under the same local `request_id`
- **AND** the Nu response remains ordinary command data without diagnostic records

#### Scenario: Change tracing level in a Nu session

- **WHEN** the caller changes `NU_PLUGIN_JEV_LOG` and restarts the plugin process
- **THEN** subsequent evaluations use the newly selected diagnostic level

#### Scenario: Explicit dependency target

- **WHEN** `NU_PLUGIN_JEV_LOG=nu_plugin_jev=info,reqwest=debug` is selected and a `reqwest` log record is emitted at `debug`
- **THEN** that record uses the same configured stderr format as plugin-owned events
- **AND** unrelated dependency targets remain disabled

#### Scenario: Existing shorthand remains scoped

- **WHEN** `NU_PLUGIN_JEV_LOG=debug` is selected
- **THEN** plugin-owned debug diagnostics appear without enabling `reqwest` or other dependency diagnostics

#### Scenario: Invalid filter fails visibly

- **WHEN** `NU_PLUGIN_JEV_LOG` contains a malformed target directive or level
- **THEN** plugin initialization fails with a diagnostic that does not reproduce credentials or request content
- **AND** no plugin command or HTTP request is executed

### Requirement: Credentials are not exposed

The plugin SHALL NOT include API keys, authorization headers, proxy credentials, or configured proxy URLs in returned values, request previews, error records, help, or plugin-authored diagnostic events. TOML syntax/permission errors SHALL NOT quote source lines or raw parser snippets that might contain secrets. Default diagnostics SHALL NOT include state or question bodies. Dependency targets SHALL be disabled by default. Because explicitly enabled third-party events are authored outside the plugin's redaction boundary, user-facing documentation SHALL warn that they may contain sensitive URLs, headers, or payload details and SHALL NOT promise automatic redaction of those events. Diagnostics SHALL NOT write into the plugin's stdout protocol stream.

#### Scenario: Failed authenticated request

- **WHEN** a mock service returns an authentication error with default dependency filtering
- **THEN** the user-facing error preserves the relevant error classification and status
- **AND** captured outputs and plugin-authored diagnostics contain no credential value or authorization header

#### Scenario: Proxy connection failure with credentials

- **WHEN** an explicit proxy URL with userinfo fails to connect or rejects a request
- **THEN** the error identifies the transport failure without exposing the configured proxy URL or its credentials in output or plugin-authored diagnostics

#### Scenario: Malformed key-bearing TOML

- **WHEN** a loaded TOML file has a syntax error on a line containing an API key
- **THEN** the error identifies the file and location without quoting that line or the key
- **AND** no request is dispatched

#### Scenario: Third-party logging is explicit

- **WHEN** `NU_PLUGIN_JEV_LOG` does not name a third-party target
- **THEN** records from that target are absent from both text and NUON diagnostics
- **AND** documentation explains the privacy risk before showing an example that enables it
