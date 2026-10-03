# jev-shell-integration Specification

## Purpose

Expose Jev decisions through a standard Nushell plugin with consistent naming, discoverable commands, configuration precedence, and caller-scoped credentials.

## Requirements

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

### Requirement: Focused short flag aliases

`jev ask` SHALL accept `-c` as an alias for `--context` and `-m` as an alias for `--model`. `jev annotate` SHALL accept those aliases plus `-j` for `--jobs`, `-s` for `--state`, `-f` for `--fields`, and `-i` for `--into`. The long flags SHALL remain available. Each alias SHALL share its long flag's value parsing, validation, configuration precedence, dry-run behavior, and live behavior. Other flags, including `--config`, SHALL remain long-only in this change.

#### Scenario: Ask short aliases

- **WHEN** the same state and questions are passed to `jev ask --dry-run` once with `-c` and `-m` and once with `--context` and `--model`
- **THEN** both invocations produce the same request body

#### Scenario: Annotate short aliases

- **WHEN** equivalent `jev annotate` invocations use `-j`, `-s`, and `-i` or their long spellings, and separate invocations use `-f` or `--fields`
- **THEN** each short-form invocation has the same state selection, request body, output field, and scheduling behavior as its long-form counterpart
- **AND** `--state` and `--fields` remain mutually exclusive regardless of which spelling is used

### Requirement: Declared Nushell compatibility

The plugin SHALL target Nu `0.116.x` and SHALL document that different pre-1.0 minor releases require a matching plugin build. Installation documentation SHALL include `cargo install --path . --locked` and the standard registration flow.

#### Scenario: Compatible installation

- **WHEN** a user follows the documented installation flow with Nu `0.116.x`
- **THEN** the plugin can be loaded and its command help inspected

### Requirement: Selectable process allocator

The Cargo package SHALL enable its optional `mimalloc` feature by default and use mimalloc as the plugin process allocator. A build with `--no-default-features` SHALL use Rust's default system allocator. Both builds SHALL expose the same Nu commands and wire behavior.

#### Scenario: Default and system allocator builds

- **WHEN** the package is built with default features or with `--no-default-features`
- **THEN** both configurations build successfully
- **AND** the former selects mimalloc while the latter retains the system allocator

### Requirement: Per-setting configuration precedence

Applicable settings SHALL resolve independently from command flags, `$env.config.plugins.jev`, caller environment variables, selected local TOML, user TOML, and defaults, in that order. This SHALL preserve the pre-existing order of flags, Nu plugin config, and environment above the new file-backed layers. Every TOML field, including `api_key` and each nested cache limit, SHALL be optional; empty TOML files SHALL be valid, and omitted fields SHALL inherit independently from lower-priority sources. The selected value SHALL be validated, and an invalid selected value SHALL cause an error rather than fall through. Configuration SHALL be resolved for each invocation and remain consistent for its rows.

#### Scenario: Flag overrides config and environment

- **WHEN** model values are present in a command flag, plugin config, and `NU_PLUGIN_JEV_MODEL`
- **THEN** the request uses the flag value

#### Scenario: Existing sources override both TOML layers

- **WHEN** different model values are present in a command flag, Nu plugin config, caller environment, local TOML, and user TOML
- **THEN** the flag value is used
- **AND** omitting the flag selects Nu plugin config, then caller environment, then local TOML, then user TOML in that order as each higher source is omitted
- **AND** an unrelated setting may still come from either TOML file independently

#### Scenario: Partial files compose independently

- **WHEN** local TOML contains only `jobs`, user TOML contains only `api_key` and `model`, and neither higher-priority settings nor a model flag are present
- **THEN** a table invocation uses local jobs, user model and key, and defaults for all other settings
- **AND** an empty local TOML leaves all selected values unchanged

#### Scenario: Invalid explicit value

- **WHEN** the selected jobs value is zero even though a lower-priority source supplies a positive value
- **THEN** invocation fails before sending any request

### Requirement: Plugin-scoped configuration names

Plugin-owned environment settings SHALL use the `NU_PLUGIN_JEV_` prefix, including model, base URL, timeout, jobs, retries, proxy, explicit config-file selection, and process-level tracing. The implicit local file SHALL be `.nu_plugin_jev.toml`, and the per-user file SHALL be `nu_plugin_jev/config.toml` below the platform's configuration root. `$env.config.plugins.jev` SHALL retain the Nu command-namespace spelling. `TYPESAFE_API_KEY` SHALL remain the caller-scoped service credential. The replaced `TYPESAFE_MODEL`, `TYPESAFE_BASE_URL`, `TYPESAFE_TIMEOUT_MS`, `TYPESAFE_JOBS`, `TYPESAFE_RETRIES`, `JEV_PROXY`, `JEV_CONFIG`, and `JEV_LOG` variables SHALL NOT be read as aliases. The old `.jev.toml` and `jev/config.toml` files SHALL NOT be discovered implicitly, though `--config` MAY explicitly select a file of either name. Documentation SHALL provide the old-to-new mapping and explain that existing local settings must be renamed or moved.

#### Scenario: Old environment settings do not override plugin defaults

- **WHEN** only `TYPESAFE_MODEL` supplies a model and a valid dry run has no model flag, Nu plugin-config value, new environment variable, or TOML value
- **THEN** the request body uses the default `jev-latest` model

#### Scenario: Old local file is not discovered

- **WHEN** the caller directory contains only `.jev.toml` and neither `--config` nor `NU_PLUGIN_JEV_CONFIG` is supplied
- **THEN** the file is not loaded implicitly
- **AND** selecting it explicitly with `--config` applies its permitted settings

### Requirement: Documented settings and defaults

The request and scheduling settings SHALL be model (`NU_PLUGIN_JEV_MODEL`, default `jev-latest`), base URL (`NU_PLUGIN_JEV_BASE_URL`, default `https://api.typesafe.ai`), timeout (`NU_PLUGIN_JEV_TIMEOUT_MS`, default `30sec`), table jobs (`NU_PLUGIN_JEV_JOBS`, default `16`), retries (`NU_PLUGIN_JEV_RETRIES`, default `3`), and proxy policy (`NU_PLUGIN_JEV_PROXY`, default `auto`). Nu plugin-config keys SHALL be `model`, `base_url`, `timeout`, `jobs`, `retries`, and `proxy`. TOML keys SHALL be `model`, `base_url`, `timeout_ms`, `jobs`, `retries`, and `proxy`. Model, base URL, timeout, and table jobs SHALL have their corresponding command flags on applicable commands. Retries and proxy policy SHALL be configurable through Nu config, environment, and TOML without per-setting command flags. Timeout SHALL be a positive Nu duration in flags/Nu config or a positive integer millisecond count in environment/TOML; jobs SHALL be a positive integer and retries a nonnegative integer. The proxy policy SHALL be `auto`, `direct`, or an explicit `http://` or `socks5h://` URL.

#### Scenario: Defaults without optional configuration

- **WHEN** a live evaluation is invoked with a valid key and no optional settings
- **THEN** it selects `jev-latest`, the default service root, a 30-second evaluation deadline, and three additional retry attempts
- **AND** a table invocation selects 16 jobs
- **AND** the proxy policy is `auto`

#### Scenario: Environment fallback

- **WHEN** plugin config and flags omit timeout and `NU_PLUGIN_JEV_TIMEOUT_MS` is `5000`
- **THEN** the evaluation deadline is five seconds

### Requirement: Configurable completed-cache limits

Table invocations SHALL resolve completed-cache limits independently from `$env.config.plugins.jev.cache.max_entries` and `cache.max_approx_bytes`, then `[cache]` values in local and user TOML, with defaults of `1024` entries and `16777216` approximate bytes (16 MiB). Both selected values SHALL be positive integers, and the byte limit SHALL be expressed in bytes. Cache limits SHALL have no command flags or environment-variable overrides. Invalid selected limits SHALL fail before input consumption or HTTP dispatch. These limits SHALL NOT reduce or disable mandatory in-flight deduplication.

#### Scenario: Independent cache defaults

- **WHEN** plugin config supplies `cache: {max_entries: 64}` without a byte limit
- **THEN** the table invocation limits its completed cache to 64 entries and 16 MiB of approximate accounted bytes

#### Scenario: Invalid cache limit

- **WHEN** a table invocation selects a zero, negative, or non-integer cache limit
- **THEN** invocation fails without consuming input rows or sending a request

### Requirement: Invocation-scoped TOML discovery and file selection

`jev ask` and `jev annotate` SHALL optionally read `nu_plugin_jev/config.toml` from the platform's per-user configuration directory and one local TOML file. On Unix, caller-scoped absolute `XDG_CONFIG_HOME` SHALL select the user configuration root; otherwise the platform default SHALL apply. The implicit local file SHALL be `.nu_plugin_jev.toml` in the calling Nu invocation's current directory, with no parent-directory search. `--config <path>` SHALL select a different local file; caller-scoped `NU_PLUGIN_JEV_CONFIG` SHALL do so when the flag is absent. Relative selected paths SHALL resolve against `EngineInterface::get_current_dir()` for that invocation, not the plugin executable's process working directory. Missing implicit files SHALL be ignored; a missing or unreadable explicitly selected file SHALL fail before input rows are read or HTTP begins. A selected explicit path identical to the user path SHALL be loaded only once. Present files SHALL be parsed with a bounded size, and malformed TOML or unknown keys SHALL fail safely; lower-priority recognized values SHALL receive semantic validation only when selected. Files SHALL be read once per invocation on a non-Tokio-worker thread and SHALL NOT be cached as a process-global configuration snapshot. The plugin SHALL NOT change its process working directory. Root usage and offline question constructors SHALL not require or load TOML files.

#### Scenario: Automatically discovered local defaults

- **WHEN** the caller runs `jev ask` from a directory containing `.nu_plugin_jev.toml` and omits `--config` and `NU_PLUGIN_JEV_CONFIG`
- **THEN** that file supplies settings absent from higher-priority sources
- **AND** no file from a parent directory is loaded

#### Scenario: Explicit local-file selection

- **WHEN** `--config` and `NU_PLUGIN_JEV_CONFIG` name different files
- **THEN** the flag-selected file is used as the sole local TOML layer
- **AND** a missing flag-selected file is an error rather than a fallback to `.nu_plugin_jev.toml`

#### Scenario: Reused process serves different directories

- **WHEN** two invocations from different Nu current directories reuse the same plugin process
- **THEN** each resolves its own local file and keeps its settings for all rows in that invocation
- **AND** the shared runtime and HTTP client pools remain reusable without changing process current directory

#### Scenario: File edit between invocations

- **WHEN** a TOML file changes after one invocation and before the next invocation in the same process
- **THEN** the next invocation sees the updated setting without restarting the plugin
- **AND** the earlier invocation's snapshot remains unchanged

#### Scenario: Offline preview with TOML defaults

- **WHEN** a valid TOML file supplies model and an optional key, and the caller runs `jev ask --dry-run`
- **THEN** the preview uses the selected model without requiring a key or opening an HTTP connection
- **AND** the preview does not include the TOML key or file contents

### Requirement: Automatically discovered local transport boundary

An implicitly discovered `.nu_plugin_jev.toml` SHALL reject `base_url` and `proxy` fields, including when a higher-priority source also supplies either field. This prevents a project file from silently redirecting a key selected from the caller environment or user file, or changing proxy routing. A local file selected explicitly by `--config` or `NU_PLUGIN_JEV_CONFIG` MAY set these fields; the existing base-URL and proxy validation SHALL still apply. The error SHALL identify the forbidden field without printing its value.

#### Scenario: Untrusted project endpoint

- **WHEN** an automatically discovered `.nu_plugin_jev.toml` contains `base_url` or `proxy`
- **THEN** the invocation fails before reading input rows or sending a request
- **AND** no caller environment or user-file key is sent to that endpoint

#### Scenario: Explicitly trusted local endpoint

- **WHEN** the caller explicitly selects that file through `--config` or `NU_PLUGIN_JEV_CONFIG`
- **THEN** its valid `base_url` or `proxy` participates in ordinary per-setting precedence

### Requirement: Caller-scoped environment credentials

Live HTTP commands SHALL select `TYPESAFE_API_KEY` from the calling Nu environment, then `api_key` from selected local TOML, then `api_key` from user TOML on each invocation. A present but empty or invalid higher-priority credential SHALL fail before HTTP rather than falling back. Missing credentials SHALL cause a call-level error before HTTP dispatch. The plugin SHALL NOT accept credentials through command flags or ordinary Nu plugin config. Question constructors, root usage, and dry runs SHALL work without a key.

#### Scenario: Key changes in a reused plugin process

- **WHEN** the caller changes `TYPESAFE_API_KEY` between two invocations
- **THEN** each invocation authenticates with its own caller-scoped value

#### Scenario: File-backed key fallback

- **WHEN** the caller omits `TYPESAFE_API_KEY` and local TOML supplies a nonempty `api_key` while user TOML supplies a different one
- **THEN** the live invocation uses the local key
- **AND** removing the local key makes the user key effective on the next invocation without restarting the plugin

#### Scenario: Explicit empty environment key

- **WHEN** `TYPESAFE_API_KEY` is present but empty and a TOML file contains a valid key
- **THEN** the live invocation fails before HTTP instead of using the file key

#### Scenario: Key-bearing file permissions

- **WHEN** a TOML file containing `api_key` is accessible to group or other users on Unix
- **THEN** the live invocation rejects it before HTTP without printing the key
- **AND** documentation explains private-file permissions on other platforms without claiming portable ACL enforcement

#### Scenario: Offline request inspection

- **WHEN** a valid dry run is performed without `TYPESAFE_API_KEY`
- **THEN** the command returns its request body successfully without authentication or network access

### Requirement: Caller-scoped Jev proxy selection

Live invocations SHALL resolve `$env.config.plugins.jev.proxy` before caller-scoped `NU_PLUGIN_JEV_PROXY`, then permitted local TOML, then user TOML, defaulting to `auto` when all are absent. The selected Jev proxy policy SHALL be validated before input consumption or HTTP dispatch and remain fixed for that invocation. An invalid selected value SHALL NOT fall back to an ordinary system proxy or another Jev setting. Changes to the Jev-specific policy between invocations SHALL work in a reused plugin process. Ordinary process proxy environment and OS proxy settings SHALL be captured by the automatic client at plugin startup; changing them SHALL require restarting that process.
User-facing documentation SHALL distinguish this process-start automatic proxy policy from per-invocation Jev-specific selection and SHALL explain that an explicit Jev proxy is not bypassed by global `NO_PROXY`.

#### Scenario: Plugin config overrides Jev proxy environment

- **WHEN** plugin config sets `proxy: direct` and the caller supplies `NU_PLUGIN_JEV_PROXY=socks5h://proxy.example:1080`
- **THEN** that invocation uses a direct connection

#### Scenario: Jev proxy changes without process restart

- **WHEN** consecutive invocations change caller-scoped `NU_PLUGIN_JEV_PROXY` from `auto` to an explicit proxy URL while reusing the plugin process
- **THEN** the second invocation uses the explicit proxy and the first invocation's policy is unchanged

#### Scenario: Invalid selected policy

- **WHEN** the selected `proxy` value is not `auto`, `direct`, or a valid supported proxy URL
- **THEN** the invocation fails before consuming input rows or opening an HTTP connection

#### Scenario: Ordinary proxy setting changes

- **WHEN** ordinary proxy environment or OS settings change while the plugin process is already running
- **THEN** the automatic client retains its previous settings until the plugin process is restarted

### Requirement: Repository-owned usage skill

The repository SHALL include `skills/jev-nushell/SKILL.md` for agents composing Nushell pipelines with the plugin. It SHALL describe implemented commands and safety-relevant behavior rather than presenting pending OpenSpec tasks as available. User-visible behavior changes SHALL update the skill alongside user documentation, including changes to command signatures, proxy policy, and external-data disclosure.

#### Scenario: Proxy transport becomes available

- **WHEN** the Jev-specific proxy policy is implemented and documented for users
- **THEN** the repository skill is updated to describe the implemented policy and its distinction from ordinary process/OS proxy discovery

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
