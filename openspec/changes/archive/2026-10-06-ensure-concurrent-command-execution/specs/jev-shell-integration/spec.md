# Spec Delta

## MODIFIED Requirements

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

## ADDED Requirements

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
