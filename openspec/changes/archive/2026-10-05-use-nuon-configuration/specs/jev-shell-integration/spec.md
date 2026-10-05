# Spec Delta

## RENAMED Requirements

- FROM: `### Requirement: Plugin-scoped TOML file names`
- TO: `### Requirement: Plugin-scoped NUON file names`

- FROM: `### Requirement: Invocation-scoped TOML discovery and file selection`
- TO: `### Requirement: Invocation-scoped NUON discovery and file selection`

- FROM: `### Requirement: Explicit local TOML selection`
- TO: `### Requirement: Explicit local NUON selection`

- FROM: `### Requirement: Safe invocation-scoped TOML reading`
- TO: `### Requirement: Safe invocation-scoped NUON reading`

- FROM: `### Requirement: Offline usage does not require TOML`
- TO: `### Requirement: Offline usage does not require NUON`

## MODIFIED Requirements

### Requirement: Per-setting configuration precedence

Each applicable setting SHALL resolve independently in this order: command flag, `$env.config.plugins.jev`, caller environment, selected local NUON, user NUON, default. Flags, Nu config, and environment SHALL retain their precedence above file-backed layers. Resolution SHALL occur per invocation and stay fixed for its rows.

#### Scenario: Flag overrides config and environment

- **WHEN** model values are present in a command flag, plugin config, and `NU_PLUGIN_JEV_MODEL`
- **THEN** the request uses the flag value

#### Scenario: Existing sources override both TOML layers

- **WHEN** former local and user TOML layers are converted to NUON, with different model values also in a command flag, Nu plugin config, and caller environment
- **THEN** the flag value is used
- **AND** omitting the flag selects Nu plugin config, then caller environment, then local NUON, then user NUON in that order as each higher source is omitted
- **AND** an unrelated setting may still come from either NUON file independently

### Requirement: Partial configuration and selected-value validation

Every NUON field, including `api_key` and nested cache limits, SHALL be optional. Empty files SHALL be valid, and omitted fields SHALL inherit independently from lower-priority sources. A selected value SHALL be validated; an invalid value SHALL fail rather than fall through to another source.

#### Scenario: Partial files compose independently

- **WHEN** local NUON contains only `jobs`, user NUON contains only `api_key` and `model`, and neither higher-priority settings nor a model flag are present
- **THEN** a table invocation uses local jobs, user model and key, and defaults for all other settings
- **AND** an empty local NUON leaves all selected values unchanged

#### Scenario: Invalid explicit value

- **WHEN** the selected jobs value is zero even though a lower-priority source supplies a positive value
- **THEN** invocation fails before sending any request

### Requirement: Plugin-scoped configuration names

Plugin-owned environment settings SHALL use `NU_PLUGIN_JEV_` for model, base URL, timeout, jobs, retries, proxy, config-file selection, and process tracing. `$env.config.plugins.jev` SHALL keep the Nu namespace; `TYPESAFE_API_KEY` SHALL remain the caller credential. Old `TYPESAFE_MODEL`, `TYPESAFE_BASE_URL`, `TYPESAFE_TIMEOUT_MS`, `TYPESAFE_JOBS`, `TYPESAFE_RETRIES`, `JEV_PROXY`, `JEV_CONFIG`, and `JEV_LOG` SHALL NOT be aliases.

#### Scenario: Old environment settings do not override plugin defaults

- **WHEN** only `TYPESAFE_MODEL` supplies a model and a valid dry run has no model flag, Nu plugin-config value, new environment variable, or NUON value
- **THEN** the request body uses the default `jev-latest` model

### Requirement: Plugin-scoped NUON file names

The implicit local file SHALL be `.nu_plugin_jev.nuon`; the per-user file SHALL be `nu_plugin_jev/config.nuon` under the platform configuration root. Former TOML paths SHALL NOT be discovered implicitly. Explicitly selected files SHALL contain NUON regardless of their extension, without TOML fallback.

#### Scenario: Old local file is not discovered

- **WHEN** the caller directory contains only `.jev.toml` and neither `--config` nor `NU_PLUGIN_JEV_CONFIG` is supplied
- **THEN** the file is not loaded implicitly
- **AND** selecting its TOML contents explicitly with `--config` fails as malformed NUON

#### Scenario: Former plugin-scoped TOML paths are ignored

- **WHEN** only `.nu_plugin_jev.toml` or user `nu_plugin_jev/config.toml` exists
- **THEN** it supplies no configuration to the invocation

#### Scenario: Explicit filename does not select a legacy parser

- **WHEN** an explicitly selected file has any extension and contains a valid NUON record
- **THEN** its settings participate in normal precedence

### Requirement: Setting names and command flags

Nu config SHALL use `model`, `base_url`, `timeout`, `jobs`, `retries`, and `proxy`; NUON SHALL use the same keys except `timeout_ms` for timeout. Applicable commands SHALL expose flags for model, base URL, timeout, and table jobs. Retries and proxy SHALL be configurable through Nu config, environment, and NUON without per-setting flags.

#### Scenario: Environment fallback

- **WHEN** plugin config and flags omit timeout and `NU_PLUGIN_JEV_TIMEOUT_MS` is `5000`
- **THEN** the evaluation deadline is five seconds

### Requirement: Validate configured setting types

Timeout SHALL be a positive Nu duration in flags/Nu config or positive integer
milliseconds in environment/NUON. Jobs SHALL be a positive integer that the
bounded scheduler can represent; retries SHALL be nonnegative. Proxy policy
SHALL be `auto`, `direct`, or an explicit `http://` or `socks5h://` URL.

#### Scenario: Invalid timeout setting

- **WHEN** a selected timeout from environment or NUON is zero or not an integer millisecond count
- **THEN** the invocation rejects it before HTTP dispatch rather than using another source

#### Scenario: Jobs exceed scheduler capacity

- **WHEN** a positive selected `jobs` value would exceed the scheduler's admission capacity
- **THEN** the invocation rejects it before reading input or starting HTTP rather than panicking

### Requirement: Configurable completed-cache limits

Table completed-cache limits SHALL resolve independently from `$env.config.plugins.jev.cache.max_entries` and `cache.max_approx_bytes`, then local and user NUON `cache` records, defaulting to `1024` entries and `16777216` approximate bytes (16 MiB). Limits SHALL be positive integers, with bytes expressed as bytes; no command flag or environment override SHALL apply. Invalid limits SHALL fail before input or HTTP and SHALL NOT disable mandatory in-flight deduplication.

#### Scenario: Independent cache defaults

- **WHEN** plugin config supplies `cache: {max_entries: 64}` without a byte limit
- **THEN** the table invocation limits its completed cache to 64 entries and 16 MiB of approximate accounted bytes

#### Scenario: Invalid cache limit

- **WHEN** a table invocation selects a zero, negative, or non-integer cache limit
- **THEN** invocation fails without consuming input rows or sending a request

### Requirement: Invocation-scoped NUON discovery and file selection

`jev ask` and `jev annotate` SHALL optionally read user `nu_plugin_jev/config.nuon` and one local NUON file. On Unix, caller-scoped absolute `XDG_CONFIG_HOME` SHALL select the user configuration root; otherwise the platform default applies. Implicit local `.nu_plugin_jev.nuon` SHALL come only from the calling Nu invocation's current directory, without parent search. Missing implicit files SHALL be ignored.

#### Scenario: Automatically discovered local defaults

- **WHEN** the caller runs `jev ask` from a directory containing `.nu_plugin_jev.nuon` and omits `--config` and `NU_PLUGIN_JEV_CONFIG`
- **THEN** that file supplies settings absent from higher-priority sources
- **AND** no file from a parent directory is loaded

### Requirement: Explicit local NUON selection

`--config <path>` SHALL select the local file; absent that flag, caller-scoped `NU_PLUGIN_JEV_CONFIG` SHALL select it. Relative paths SHALL resolve against that invocation's `EngineInterface::get_current_dir()`, not plugin process cwd. Missing or unreadable explicit files SHALL fail before rows or HTTP. An explicit path identical to the user file SHALL load only once. The plugin SHALL NOT change its process cwd.

#### Scenario: Explicit local-file selection

- **WHEN** `--config` and `NU_PLUGIN_JEV_CONFIG` name different files
- **THEN** the flag-selected file is used as the sole local NUON layer
- **AND** a missing flag-selected file is an error rather than a fallback to `.nu_plugin_jev.nuon`

### Requirement: Safe invocation-scoped NUON reading

Present NUON SHALL be size-bounded and read once per invocation on a non-Tokio-worker thread, never kept as a process-global snapshot. Malformed NUON and unknown keys SHALL fail safely. Recognized lower-priority values SHALL receive semantic validation only if selected. Settings SHALL stay fixed for the invocation while later invocations may see file edits.

#### Scenario: Reused process serves different directories

- **WHEN** two invocations from different Nu current directories reuse the same plugin process
- **THEN** each resolves its own local file and keeps its settings for all rows in that invocation
- **AND** the shared runtime and HTTP client pools remain reusable without changing process current directory

#### Scenario: File edit between invocations

- **WHEN** a NUON file changes after one invocation and before the next invocation in the same process
- **THEN** the next invocation sees the updated setting without restarting the plugin
- **AND** the earlier invocation's snapshot remains unchanged

### Requirement: Offline usage does not require NUON

Root usage and offline question constructors SHALL neither require nor load NUON. Dry runs SHALL use selected NUON defaults when present but SHALL NOT require a key or connect to the service.

#### Scenario: Root and constructors ignore NUON

- **WHEN** the user invokes bare `jev` or a question constructor with malformed local NUON
- **THEN** the offline command remains available without loading that file

#### Scenario: Root and constructors ignore TOML

- **WHEN** the user invokes bare `jev` or a question constructor with malformed legacy local TOML
- **THEN** the offline command remains available without loading that file

#### Scenario: Offline preview with NUON defaults

- **WHEN** a valid NUON file supplies model and an optional key, and the caller runs `jev ask --dry-run`
- **THEN** the preview uses the selected model without requiring a key or opening an HTTP connection
- **AND** the preview does not include the NUON key or file contents

#### Scenario: Offline preview with TOML defaults

- **WHEN** valid TOML defaults with model and optional key are converted to a selected NUON file before `jev ask --dry-run`
- **THEN** the preview uses the converted model without requiring a key or opening an HTTP connection
- **AND** the preview includes neither the key nor configuration contents

### Requirement: Automatically discovered local transport boundary

Implicit `.nu_plugin_jev.nuon` SHALL reject `base_url` and `proxy`, even if a higher-priority source supplies them, so project files cannot redirect caller/user credentials or proxy routing. Files explicitly selected by `--config` or `NU_PLUGIN_JEV_CONFIG` MAY set those fields, subject to normal validation. Errors SHALL name the forbidden field without printing its value.

#### Scenario: Untrusted project endpoint

- **WHEN** an automatically discovered `.nu_plugin_jev.nuon` contains `base_url` or `proxy`
- **THEN** the invocation fails before reading input rows or sending a request
- **AND** no caller environment or user-file key is sent to that endpoint

#### Scenario: Explicitly trusted local endpoint

- **WHEN** the caller explicitly selects that file through `--config` or `NU_PLUGIN_JEV_CONFIG`
- **THEN** its valid `base_url` or `proxy` participates in ordinary per-setting precedence

### Requirement: Caller-scoped environment credentials

On each invocation, live commands SHALL select caller Nu `TYPESAFE_API_KEY`, then local NUON `api_key`, then user NUON `api_key`. An empty or invalid selected key SHALL fail before HTTP, without fallback; a missing key SHALL cause a call-level error. Credentials SHALL NOT come from flags or ordinary Nu plugin config. Constructors, root usage, and dry runs SHALL work without a key.

#### Scenario: Key changes in a reused plugin process

- **WHEN** the caller changes `TYPESAFE_API_KEY` between two invocations
- **THEN** each invocation authenticates with its own caller-scoped value

#### Scenario: File-backed key fallback

- **WHEN** the caller omits `TYPESAFE_API_KEY` and local NUON supplies a nonempty `api_key` while user NUON supplies a different one
- **THEN** the live invocation uses the local key
- **AND** removing the local key makes the user key effective on the next invocation without restarting the plugin

#### Scenario: Explicit empty environment key

- **WHEN** `TYPESAFE_API_KEY` is present but empty and a NUON file contains a valid key
- **THEN** the live invocation fails before HTTP instead of using the file key

#### Scenario: Key-bearing file permissions

- **WHEN** a NUON file containing `api_key` is accessible to group or other users on Unix
- **THEN** the live invocation rejects it before HTTP without printing the key
- **AND** documentation explains private-file permissions on other platforms without claiming portable ACL enforcement

#### Scenario: Offline request inspection

- **WHEN** a valid dry run is performed without `TYPESAFE_API_KEY`
- **THEN** the command returns its request body successfully without authentication or network access

### Requirement: Caller-scoped Jev proxy selection

Live proxy policy SHALL resolve from `$env.config.plugins.jev.proxy`, caller `NU_PLUGIN_JEV_PROXY`, permitted local NUON, user NUON, then `auto`. It SHALL be validated before input or HTTP and fixed per invocation. Invalid selected policy SHALL fail without falling back to a system proxy or another Jev setting. Jev-specific policy changes SHALL work across invocations in a reused plugin process.

#### Scenario: Plugin config overrides Jev proxy environment

- **WHEN** plugin config sets `proxy: direct` and the caller supplies `NU_PLUGIN_JEV_PROXY=socks5h://proxy.example:1080`
- **THEN** that invocation uses a direct connection

#### Scenario: Jev proxy changes without process restart

- **WHEN** consecutive invocations change caller-scoped `NU_PLUGIN_JEV_PROXY` from `auto` to an explicit proxy URL while reusing the plugin process
- **THEN** the second invocation uses the explicit proxy and the first invocation's policy is unchanged

#### Scenario: Invalid selected policy

- **WHEN** the selected `proxy` value is not `auto`, `direct`, or a valid supported proxy URL
- **THEN** the invocation fails before consuming input rows or opening an HTTP connection

### Requirement: Credentials are not exposed

Returned values, previews, errors, help, and plugin-authored diagnostics SHALL NOT expose API keys, authorization headers, proxy credentials, or configured proxy URLs. NUON syntax/permission errors SHALL NOT quote source lines or raw parser snippets. Default diagnostics SHALL omit state and question bodies. Diagnostics SHALL NOT enter the stdout plugin protocol.

#### Scenario: Failed authenticated request

- **WHEN** a mock service returns an authentication error with default dependency filtering
- **THEN** the user-facing error preserves the relevant error classification and status
- **AND** captured outputs and plugin-authored diagnostics contain no credential value or authorization header

#### Scenario: Proxy connection failure with credentials

- **WHEN** an explicit proxy URL with userinfo fails to connect or rejects a request
- **THEN** the error identifies the transport failure without exposing the configured proxy URL or its credentials in output or plugin-authored diagnostics

#### Scenario: Malformed key-bearing NUON

- **WHEN** a loaded NUON file has a syntax error on a line containing an API key
- **THEN** the error identifies the configuration layer without quoting source text, parser snippets, or the key
- **AND** no request is dispatched

#### Scenario: Malformed key-bearing TOML

- **WHEN** an explicitly selected file contains legacy TOML with an API key
- **THEN** it is rejected as malformed NUON without printing source text, parser snippets, or the key
- **AND** no request is dispatched

## ADDED Requirements

### Requirement: Data-only NUON configuration

Configuration files SHALL be parsed as NUON data, never evaluated as Nu programs. Executable expressions, pipelines, closures, variable references, and interpolated strings SHALL fail before HTTP.

#### Scenario: Configuration attempts to execute code

- **WHEN** a selected configuration contains a command substitution, variable reference, closure, or pipeline
- **THEN** the invocation rejects it without executing that code or sending a request

### Requirement: NUON configuration root and unique fields

A nonempty, non-comment-only configuration SHALL contain exactly one NUON record. Empty or comment-only files SHALL act as empty records. Duplicate keys and unknown top-level or cache-record keys SHALL fail, including when a higher-priority source would override them.

#### Scenario: Empty configuration forms

- **WHEN** a configuration file is empty, contains only whitespace or comments, or contains `{}`
- **THEN** it contributes no setting and preserves lower-priority values

#### Scenario: Invalid root or duplicate fields

- **WHEN** a configuration contains a list, scalar, multiple values, or duplicate keys in its root or nested cache record
- **THEN** the invocation rejects it before input or HTTP rather than silently replacing a value

#### Scenario: Unknown configuration key

- **WHEN** a file contains an unknown top-level key or a record-shaped cache contains an unknown key
- **THEN** the invocation rejects the file without printing the unknown value or file contents

### Requirement: Documented TOML-to-NUON migration

User documentation and the repository usage skill SHALL identify the NUON paths, record shape, retained file setting names, and lack of TOML fallback. Migration guidance SHALL convert TOML through native Nu data commands without printing credentials or suggesting that renaming the extension converts the content.

#### Scenario: Existing private TOML configuration

- **WHEN** a user follows the documented migration from an existing TOML configuration
- **THEN** the resulting NUON record retains its settings, including `api_key` and nested cache limits, without printing the key
- **AND** guidance requires private permissions for key-bearing files and updating any explicit file path
