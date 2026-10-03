# Proposal

## Why

The plugin can evaluate a chosen model but cannot show which model names the authenticated TypeSafe account currently exposes. Users must consult external documentation instead of discovering names in the Nu session where they will use them.

## What Changes

- Add `jev models` as one authenticated, input-free call to TypeSafe's `GET /v1/models`, returning the response's model entries as an ordinary Nu list/table with `name`, `description`, and `release_date` string fields.
- Apply the existing caller-scoped API key, service root, proxy, timeout, retry, and TOML/config precedence to this call. Do not require or validate a configured evaluation model, table jobs, or cache limits.
- Keep discovery informational: do not cache the list, select a model automatically, or make `jev ask`/`jev annotate` perform a listing preflight. Retain native Nu filtering and sorting rather than adding model-specific flags.
- Update command help, README, and the repository usage skill when the command is implemented; tests use a local mock service rather than a live key.

## Capabilities

### New Capabilities

- `jev-model-discovery`: Define `jev models` input, output, and model-list freshness semantics.

### Modified Capabilities

- `jev-shell-integration`: Register the additional command and update offline namespace guidance.
- `jev-http-transport`: Specify the authenticated model-list GET, response validation, and existing transport safety policy for that endpoint.

## Impact

Planning affects command registration/root guidance, a new models command, API types and client method, scoped configuration resolution, local-mock and real-Nu tests, README, and `skills/jev-nushell/SKILL.md`. No new API endpoint, framework layer, or evaluation behavior is introduced. The separate `add-request-metrics` change owns the later always-present `meta.base_url` wrapper and optional listing measurements.
