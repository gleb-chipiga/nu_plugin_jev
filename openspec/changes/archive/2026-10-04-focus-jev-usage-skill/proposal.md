# Proposal

## Why

The repository usage skill includes low-level cancellation mechanics that do not help an agent compose Jev/Nushell pipelines. Such details consume context and obscure the command and result guidance the skill is meant to provide.

## What Changes

- Remove the blocked-iterator and local HTTP cancellation explanation from the usage skill.
- Keep the skill focused on implemented, actionable command, result, configuration, and safety guidance; leave internal guarantees in engineering specs and tests.
- Record this scope boundary in the repository's skill-maintenance guidance.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `jev-shell-integration`: define the content boundary for the repository-owned usage skill.

## Impact

Only `skills/jev-nushell/SKILL.md`, `AGENTS.md`, and the shell-integration specification change. Plugin commands, HTTP behavior, Rust code, and user-facing README guidance remain unchanged.
