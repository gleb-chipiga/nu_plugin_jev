# jev-shell-integration Specification

## Purpose

Expose Jev decisions through a standard Nushell plugin with consistent naming, discoverable commands, configuration precedence, and caller-scoped credentials.

## Requirements

### Requirement: Consistent plugin identity and discovery

The repository, crate, and binary SHALL be named `nu_plugin_jev`, with Nu namespace `jev`. Registration SHALL support `plugin add <binary-path>` then `plugin use jev`. Registered commands SHALL be exactly `jev`, `jev ask`, `jev annotate`, `jev models`, `jev question noul`, `jev question choice`, and `jev question score`. Scalar projection and filtering SHALL use Nu; `jev noul`, `jev choice`, `jev score`, and `jev where` SHALL NOT be registered.

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

### Requirement: Truthful Nushell command input and output types

Nu signatures SHALL declare `jev`: `nothing -> string`; `jev ask`: `any -> record`; `jev annotate`: `any -> list<any>`; `jev models` and each question constructor: `nothing -> record`. `any` SHALL allow runtime validation, not imply every state or row is valid. Annotation's output type SHALL allow unchanged non-record rows under `--on-error keep`. Bare `jev` SHALL reject nonempty pipeline input rather than ignore it.

#### Scenario: Command type discovery

- **WHEN** Nu inspects the seven registered command signatures
- **THEN** it reports the declared input and output types for each command
- **AND** `jev annotate` does not claim that every emitted item is a record

#### Scenario: Root guidance receives pipeline input

- **WHEN** a nonempty value is piped to bare `jev`
- **THEN** the invocation fails instead of discarding that value and returning guidance

#### Scenario: Annotation keeps a non-record row

- **WHEN** a non-record input row is handled under `jev annotate --on-error keep`
- **THEN** the unchanged row remains representable by the command's declared output type

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

Each invocation-scoped setting SHALL resolve independently in this order:
command flag, `$env.config.plugins.jev`, caller environment, selected local
NUON, user NUON, default. Flags, Nu config, and environment SHALL retain their
precedence above file-backed layers. Resolution SHALL occur per invocation
and stay fixed for its rows.

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

### Requirement: Documented settings and defaults

Settings SHALL have these environment/default pairs: `NU_PLUGIN_JEV_MODEL`/`jev-latest`, `NU_PLUGIN_JEV_BASE_URL`/`https://api.typesafe.ai`, `NU_PLUGIN_JEV_TIMEOUT_MS`/`30sec`, `NU_PLUGIN_JEV_JOBS`/`16`, `NU_PLUGIN_JEV_RETRIES`/`3`, and `NU_PLUGIN_JEV_PROXY`/`auto`. Jobs apply to tables. Documentation SHALL state these defaults.

#### Scenario: Defaults without optional configuration

- **WHEN** a live evaluation is invoked with a valid key and no optional settings
- **THEN** it selects `jev-latest`, the default service root, a 30-second evaluation deadline, and three additional retry attempts
- **AND** a table invocation selects 16 jobs
- **AND** the proxy policy is `auto`

### Requirement: Setting names and command flags

Invocation Nu config SHALL use `model`, `base_url`, `timeout`, `jobs`,
`retries`, and `proxy`; NUON SHALL use those keys except `timeout_ms`, plus
startup-only `max_in_flight`. Applicable commands SHALL expose model, base
URL, timeout, and table-jobs flags. Retries and proxy SHALL be configurable
through Nu config, environment, and NUON without per-setting flags.

#### Scenario: Environment fallback

- **WHEN** plugin config and flags omit timeout and `NU_PLUGIN_JEV_TIMEOUT_MS` is `5000`
- **THEN** the evaluation deadline is five seconds

#### Scenario: Process-only field in invocation files

- **WHEN** a running process serves a command whose selected NUON contains an edited or invalid `max_in_flight`
- **THEN** the field is recognized without validating or applying it as an invocation setting
- **AND** ordinary settings still resolve normally and the process budget remains unchanged

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

After successful process startup, root usage and offline question
constructors SHALL neither require nor load invocation NUON. Dry runs SHALL
use selected invocation NUON defaults but SHALL NOT require a key or connect
to the service. Startup process-limit validation SHALL still apply before
any command is served.

#### Scenario: Root and constructors ignore NUON

- **WHEN** startup succeeds and a local NUON file becomes malformed before a root or question-constructor call
- **THEN** the offline command remains available without loading the invocation file

#### Scenario: Root and constructors ignore TOML

- **WHEN** startup succeeds and bare `jev` or a question constructor is invoked with malformed legacy local TOML
- **THEN** the offline command remains available without loading that file

#### Scenario: Offline preview with NUON defaults

- **WHEN** startup succeeds, a valid NUON file supplies model and an optional key, and the caller runs `jev ask --dry-run`
- **THEN** the preview uses the selected model without requiring a key or opening an HTTP connection
- **AND** the preview does not include the NUON key or file contents

#### Scenario: Offline preview with TOML defaults

- **WHEN** startup succeeds and valid TOML defaults with model and optional key are converted to selected NUON before `jev ask --dry-run`
- **THEN** the preview uses the converted model without requiring a key or opening an HTTP connection
- **AND** the preview includes neither the key nor configuration contents

#### Scenario: Invalid startup value prevents offline command servicing

- **WHEN** a selected startup attempt-limit value is invalid and the first command is bare `jev`
- **THEN** startup fails before serving that command instead of bypassing process-policy validation

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

### Requirement: Automatic proxy settings are process-scoped

The automatic client SHALL capture ordinary process environment and OS proxy settings at startup; changes SHALL require a plugin restart. Documentation SHALL distinguish these from per-invocation Jev policy and explain that global `NO_PROXY` does not bypass an explicit Jev proxy.

#### Scenario: Ordinary proxy setting changes

- **WHEN** ordinary proxy environment or OS settings change while the plugin process is already running
- **THEN** the automatic client retains its previous settings until the plugin process is restarted

### Requirement: Repository-owned usage skill

The repository SHALL include `skills/jev-nushell/SKILL.md` for agents composing Nushell pipelines with the plugin. It SHALL describe implemented commands and safety-relevant behavior rather than presenting pending OpenSpec tasks as available. User-visible behavior changes SHALL update the skill alongside user documentation, including changes to command signatures, proxy policy, and external-data disclosure.

#### Scenario: Proxy transport becomes available

- **WHEN** the Jev-specific proxy policy is implemented and documented for users
- **THEN** the repository skill is updated to describe the implemented policy and its distinction from ordinary process/OS proxy discovery

### Requirement: Task-focused repository usage skill

The repository-owned usage skill SHALL include implemented guidance only when it affects command or flag choice, outbound data, result or error interpretation, access configuration, or material disclosure risks, excluding internal mechanics with no such effect.

#### Scenario: Internal cancellation mechanism

- **WHEN** an engineering spec describes what happens to an external iterator blocked in `next()` during cancellation
- **THEN** the usage skill omits that mechanism because it does not change how an agent composes or consumes a Jev pipeline

#### Scenario: Actionable outbound-state selection

- **WHEN** `jev annotate` offers options that change which source fields are sent to the API
- **THEN** the usage skill explains those options because they change what data leaves Nushell

### Requirement: Opt-in newline-delimited NUON diagnostics

`NU_PLUGIN_JEV_LOG_FORMAT` SHALL select `text` or `nuon` at plugin startup, defaulting to text. An invalid explicit value SHALL fail startup without writing diagnostics into the plugin protocol.

#### Scenario: Default presentation and invalid format

- **WHEN** `NU_PLUGIN_JEV_LOG_FORMAT` is unset
- **THEN** diagnostics retain their existing human-readable stderr presentation
- **AND** an invalid explicit format is rejected before serving plugin commands rather than silently selecting another format

### Requirement: One typed NUON record per diagnostic event

In `nuon` mode, each stderr event SHALL be one physical line of compact NUON with UTC RFC3339 `timestamp`, string `level`, `target`, `message`, record `fields`, and root-to-leaf record list `spans` (`name`, `fields`). Primitive event/span fields SHALL retain types; unsupported debug-only fields SHALL become strings. Newlines SHALL be escaped. Span creation/closure SHALL NOT become events unless instrumentation emits them.

#### Scenario: Parse mixed-source diagnostics in Nu

- **WHEN** plugin-owned `tracing` events and explicitly enabled dependency `log` events are emitted with `NU_PLUGIN_JEV_LOG_FORMAT=nuon`
- **THEN** each plugin-authored diagnostic line is independently parseable with `from nuon` and a stream of those lines is parseable with Nu's `from ndnuon`
- **AND** records identify their originating `target` and `level`

#### Scenario: Preserve request correlation and types

- **WHEN** an evaluation event includes numeric `duration_ms` and an enclosing span contains its local `request_id`
- **THEN** the NUON record retains `duration_ms` as a number and the request identity in `spans`
- **AND** a message containing a newline remains one parseable physical line

### Requirement: NUON formatter is feature-gated

The `nuon-tracing-format` Cargo feature SHALL compile the NUON formatter and be enabled by default. Without it, text diagnostics and target filtering SHALL remain available; explicit `NU_PLUGIN_JEV_LOG_FORMAT=nuon` SHALL fail startup with a value-redacted feature-unavailable error.

#### Scenario: Build without NUON diagnostics

- **WHEN** the crate is built with `--no-default-features`
- **THEN** text diagnostics and target filtering remain available
- **AND** an explicit `NU_PLUGIN_JEV_LOG_FORMAT=nuon` is rejected before serving commands

### Requirement: Process-level evaluation diagnostics

The plugin SHALL initialize non-blocking diagnostics before entering Tokio and write to stderr, never Nu pipeline values or stdout protocol. `NU_PLUGIN_JEV_LOG` SHALL select a process-wide target/level filter at startup; filter or format changes SHALL require restart. Neither `RUST_LOG` nor legacy `JEV_LOG` SHALL be a fallback.

#### Scenario: Change tracing level in a Nu session

- **WHEN** the caller changes `NU_PLUGIN_JEV_LOG` and restarts the plugin process
- **THEN** subsequent evaluations use the newly selected diagnostic level

### Requirement: Plugin-only diagnostic filter shorthand

Absent `NU_PLUGIN_JEV_LOG`, the filter SHALL enable `nu_plugin_jev=warn` and no dependency targets. Bare `off`, `error`, `warn`, `info`, `debug`, and `trace` SHALL keep their plugin-only meaning.

#### Scenario: Existing shorthand remains scoped

- **WHEN** `NU_PLUGIN_JEV_LOG=debug` is selected
- **THEN** plugin-owned debug diagnostics appear without enabling `reqwest` or other dependency diagnostics

### Requirement: Explicit dependency diagnostic targets

Comma-separated `target=level` directives SHALL allow explicit plugin/dependency targets, retain `nu_plugin_jev=warn` unless overridden, disable unlisted dependencies, and let more-specific targets override broader ones. Invalid explicit filters SHALL fail startup. Rust `log` and `tracing` records SHALL share the filter and formatter; enabling a target SHALL NOT guarantee dependency events or connection-verbose behavior.

#### Scenario: Explicit dependency target

- **WHEN** `NU_PLUGIN_JEV_LOG=nu_plugin_jev=info,reqwest=debug` is selected and a `reqwest` log record is emitted at `debug`
- **THEN** that record uses the same configured stderr format as plugin-owned events
- **AND** unrelated dependency targets remain disabled

#### Scenario: Invalid filter fails visibly

- **WHEN** `NU_PLUGIN_JEV_LOG` contains a malformed target directive or level
- **THEN** plugin initialization fails with a diagnostic that does not reproduce credentials or request content
- **AND** no plugin command or HTTP request is executed

### Requirement: Correlated evaluation diagnostic content

At plugin `info`, evaluation starts, completions, and failures SHALL carry local `request_id` and elapsed time on completion/failure. Success SHALL include returned model and usage; failure SHALL include error kind and HTTP status when available. At `debug`, diagnostics SHALL also identify attempts, retry delays, and table-result reuse without request bodies.

#### Scenario: Correlated request diagnostics

- **WHEN** an evaluation retries before succeeding with `NU_PLUGIN_JEV_LOG=debug`
- **THEN** its attempts and completion diagnostics appear on stderr under the same local `request_id`
- **AND** the Nu response remains ordinary command data without diagnostic records

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

### Requirement: Explicit third-party diagnostic privacy boundary

Dependency targets SHALL be disabled by default. Because explicitly enabled third-party events are outside the plugin's redaction boundary, documentation SHALL warn that they may contain sensitive URLs, headers, or payloads and SHALL NOT promise automatic redaction.

#### Scenario: Third-party logging is explicit

- **WHEN** `NU_PLUGIN_JEV_LOG` does not name a third-party target
- **THEN** records from that target are absent from both text and NUON diagnostics
- **AND** documentation explains the privacy risk before showing an example that enables it

### Requirement: Correlated successful HTTP measurements in diagnostics

At `info`, each successful evaluation or model lookup SHALL emit one plugin-owned `evaluation completed` or `model listing completed` event. It SHALL carry integer `request_bytes`, `response_bytes`, `elapsed_ns`, `attempt_elapsed_ns`, `attempts`, and string `http_version`, `base_url`. HTTP fields SHALL match optional `--metrics`; `base_url` SHALL be the selected validated root also returned in `meta.base_url` (`ask`, `models`) or `jev_meta.base_url` (`annotate`), even without the flag.

#### Scenario: Unflagged success is measured in tracing

- **WHEN** `jev ask` succeeds with `NU_PLUGIN_JEV_LOG=info` and without `--metrics`
- **THEN** its completion diagnostic includes `base_url`, `request_bytes`, `response_bytes`, `elapsed_ns`, `attempt_elapsed_ns`, `attempts`, and `http_version` under the same local `request_id`
- **AND** the unflagged Nu result includes the same selected root at `meta.base_url`, along with the returned model and usage

#### Scenario: Model-list lookup has the same tracing contract

- **WHEN** `jev models` succeeds with or without `--metrics` and plugin `info` tracing enabled
- **THEN** one `model listing completed` event includes the selected `base_url`, zero `request_bytes`, response bytes, both durations, explicit GET attempt count, and final HTTP version
- **AND** the event's `base_url` equals `meta.base_url`, and with `--metrics` its HTTP fields agree with the returned `metrics`
- **AND** the event preserves its model count without inventing `usage` tokens

### Requirement: Trace measurements match returned measurements

`elapsed_ns` and `attempt_elapsed_ns` SHALL match optional Nu `metrics`/`jev_metrics` durations in nanoseconds; `duration_ms` SHALL remain a separate broader operation duration. Events SHALL retain local `request_id`; evaluation completions SHALL retain model and usage, while model listings SHALL retain model count without token usage. `base_url` SHALL be neither an appended endpoint nor proxy URL. Events SHALL omit state, questions, bodies, credentials, full request URLs, and proxy details.

#### Scenario: Tracing and returned metrics agree

- **WHEN** `jev ask` succeeds with `--metrics`
- **THEN** the completion event's `base_url` equals returned `meta.base_url`, while its byte counts, attempt count, and HTTP version equal the returned `metrics` fields
- **AND** both nanosecond fields equal their respective returned Nu durations, while `duration_ms` retains its existing separate meaning

### Requirement: Trace one completion per actual HTTP evaluation

Joining an in-flight evaluation or serving a completed cache entry SHALL NOT emit another successful evaluation completion event. Failed, cancelled, dry-run, and offline token-estimation operations SHALL NOT fabricate successful measurement fields.

#### Scenario: Retry and deduplication do not multiply completions

- **WHEN** a retry succeeds and duplicate annotation rows share that evaluation, including a later completed-cache hit
- **THEN** exactly one successful completion event is produced for the actual evaluation, with `attempts` counting its HTTP attempts and `elapsed_ns` including the retry wait
- **AND** `attempt_elapsed_ns` excludes the earlier attempt and wait while including final response validation
- **AND** no additional completion event is produced for rows that reuse its result
- **AND** with `--metrics`, every reused row's `jev_metrics.request_id` equals that completion event's local `request_id`

#### Scenario: No fabricated successful measurements

- **WHEN** an evaluation fails or is cancelled, or a request is only previewed or token-estimated offline
- **THEN** no successful evaluation completion event with measurement fields is produced

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

### Requirement: Native concurrent command execution

Commands invoked concurrently through native Nu facilities SHALL be able to
make overlapping HTTP progress in one plugin process when shared attempt
capacity is available, without a plugin-specific parallel-launch flag.

#### Scenario: Real Nu table invocations overlap

- **WHEN** one Nu process runs two one-row `jev annotate --jobs 1` invocations through `par-each --threads 2`, with distinct states and an attempt limit of at least two
- **THEN** a server withholding replies until both requests arrive receives both requests before releasing either response
- **AND** each branch fully consumes its annotation result and the pipeline finishes within a protective test timeout

#### Scenario: Single-state calls overlap

- **WHEN** two distinct `jev ask` calls run concurrently with sufficient shared attempt capacity
- **THEN** both requests can reach the server before either response is returned

#### Scenario: Different command families overlap

- **WHEN** `jev ask`, `jev annotate`, and `jev models` run concurrently with sufficient shared attempt capacity
- **THEN** one unfinished command does not serialize the other commands' HTTP attempts

### Requirement: Independent invocation settings

Concurrent invocations SHALL use their own resolved request settings, inputs,
questions, and context without replacing the configuration of another
invocation in the same process.

#### Scenario: Distinct callers retain their request configuration

- **WHEN** concurrent calls use different API roots, synthetic keys, requested models, states, questions, and context
- **THEN** each outbound request uses only its caller's resolved settings and data
- **AND** each result is delivered to the caller whose request produced it

### Requirement: Independent local invocation termination

A local timeout, invocation failure, or output drop SHALL NOT cancel a
different invocation or disable its access to the shared HTTP budget.
Plugin-wide Nu interrupt behavior SHALL remain distinct from local
termination.

#### Scenario: Expected failure does not abort its neighbor

- **WHEN** one concurrent Nu branch catches its expected request error within that branch while another branch awaits a valid response
- **THEN** the healthy branch completes normally without an induced plugin-wide interrupt

#### Scenario: Output drop remains local

- **WHEN** one annotation output is dropped while a different invocation is still active
- **THEN** local work for the dropped output stops and the other invocation continues

### Requirement: Interrupt registration has no unchecked gap

Live commands SHALL register their interrupt handler before checking current
engine signal state and beginning work. Interrupts arriving before registration,
during registration, or afterward SHALL prevent dispatch or cancel protected
work. A subsequent Reset SHALL NOT revive local work already cancelled.

#### Scenario: Interrupt at registration boundary

- **WHEN** Nu signals an interrupt before or during registration for a live command
- **THEN** the command detects that signal before starting its protected work

#### Scenario: Interrupt after checking

- **WHEN** Nu signals an interrupt after registration and the state check
- **THEN** the registered handler cancels the protected operation
- **AND** a later signal Reset does not resume it

### Requirement: Interrupt handlers protect the full operation lifetime

A live command SHALL retain its interrupt registration throughout its protected
operation. An annotation SHALL retain that registration until its returned
stream ends or is dropped, not merely until its command handler returns.

#### Scenario: Interrupt after returning annotation output

- **WHEN** annotation has returned its stream and Nu interrupts while that stream still has unfinished work
- **THEN** its registered handler still cancels the invocation

#### Scenario: Handler guard is released with its operation

- **WHEN** a protected operation ends or its returned stream is dropped
- **THEN** its interrupt registration is released instead of accumulating process-wide handlers

### Requirement: Startup process HTTP attempt limit

The shared HTTP attempt limit SHALL resolve at plugin startup in this order:
process `NU_PLUGIN_JEV_MAX_IN_FLIGHT`, selected local NUON `max_in_flight`,
user NUON `max_in_flight`, then `128`. An invalid selected value SHALL fail
startup without falling back. Lower-priority files SHALL NOT be loaded solely
for this setting when a higher-priority value is present.

#### Scenario: Default process budget

- **WHEN** the plugin starts without an environment or NUON attempt-limit value
- **THEN** its shared HTTP attempt limit is 128 regardless of which command runs first

#### Scenario: Partial files inherit independently

- **WHEN** startup local NUON contains only `jobs: 8` and user NUON contains `max_in_flight: 2`
- **THEN** the shared limit is two and table jobs resolve independently to eight

#### Scenario: Local file overrides user value

- **WHEN** startup local and user NUON contain limits of two and four, with no environment override
- **THEN** the shared limit is two

#### Scenario: Environment overrides file values

- **WHEN** startup `NU_PLUGIN_JEV_MAX_IN_FLIGHT=2` is present while lower-priority files contain different or invalid values
- **THEN** the shared limit is two without loading those files for this setting

#### Scenario: Invalid selected file value does not fall back

- **WHEN** startup local NUON contains an invalid `max_in_flight` while user NUON contains a valid value
- **THEN** startup fails with a redacted local `max_in_flight` diagnostic rather than using the user value

### Requirement: Startup attempt-limit file selection

Startup attempt-limit resolution SHALL select the local file using process
`NU_PLUGIN_JEV_CONFIG` relative to absolute startup Nu `PWD` (OS cwd fallback),
or `.nu_plugin_jev.nuon` there. It SHALL select user `nu_plugin_jev/config.nuon`
from the startup platform config root, honoring absolute `XDG_CONFIG_HOME`. Explicit files
SHALL be required and implicit files optional, using the existing bounded
data-only NUON parser and field restrictions.

#### Scenario: Explicit relative startup file

- **WHEN** startup `NU_PLUGIN_JEV_CONFIG=custom.nuon` selects a file containing `max_in_flight: 2`
- **THEN** that path is resolved against startup Nu `PWD` and establishes the process limit
- **AND** a missing selected file fails startup without disclosing its contents or path

#### Scenario: Nu runs the executable from another directory

- **WHEN** Nu spawns the plugin beside its binary but passes an absolute startup `PWD`
- **THEN** local file selection uses that `PWD`, not the executable's directory
- **AND** absent or relative startup `PWD` falls back to OS cwd

#### Scenario: Startup user directory

- **WHEN** absolute startup `XDG_CONFIG_HOME` selects a user root containing `nu_plugin_jev/config.nuon`
- **THEN** attempt-limit resolution uses that file after any higher-priority source

#### Scenario: Unsafe configuration syntax

- **WHEN** a startup file read for the limit is malformed, oversized, executable, or contains unknown fields
- **THEN** startup fails through the existing redacted NUON validation without executing file content

### Requirement: Immutable process HTTP attempt limit

The resolved HTTP attempt limit SHALL remain fixed for the process lifetime.
Invocation flags, Nu plugin config, caller environment, caller directories,
and later NUON reads or edits SHALL NOT resize it. Applying a different limit
SHALL require a plugin restart with updated startup sources.

#### Scenario: Startup override survives later caller changes

- **WHEN** the plugin starts with `NU_PLUGIN_JEV_MAX_IN_FLIGHT=2` and a later invocation changes the caller's value or request configuration
- **THEN** all subsequent calls in that process still share the limit of two
- **AND** changing the process limit requires restarting the plugin with the new startup value

#### Scenario: Startup file survives later file and caller changes

- **WHEN** the process starts from local NUON `max_in_flight: 2`, that file is edited to nine, and later calls use another directory or `--config` file with a different limit
- **THEN** all calls still share the process limit of two
- **AND** ordinary invocation settings retain their per-call resolution behavior

### Requirement: Valid startup HTTP attempt limit

A selected `NU_PLUGIN_JEV_MAX_IN_FLIGHT` SHALL be a positive base-10 string;
selected NUON `max_in_flight` SHALL be a positive native integer. Both SHALL
fit the supported limiter capacity. Invalid values SHALL fail startup with
an actionable diagnostic naming the setting and source, without panic or
raw-value disclosure.

#### Scenario: Valid explicit budget

- **WHEN** the plugin starts with a supported positive integer such as `2`
- **THEN** it serves commands with that shared attempt limit

#### Scenario: Invalid startup budget

- **WHEN** the startup value is empty, zero, negative, fractional, nonnumeric, non-Unicode, or exceeds the supported capacity
- **THEN** startup fails before serving commands and names `NU_PLUGIN_JEV_MAX_IN_FLIGHT` as invalid
- **AND** the diagnostic neither panics nor reproduces the supplied raw value

#### Scenario: Invalid NUON type or capacity

- **WHEN** selected NUON `max_in_flight` is a string, float, boolean, null, zero, negative, or exceeds the supported capacity
- **THEN** startup fails naming the file layer and `max_in_flight` without echoing the value
