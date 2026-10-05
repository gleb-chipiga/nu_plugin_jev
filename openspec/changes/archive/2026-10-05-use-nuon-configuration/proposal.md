# Proposal

## Why

NUON lets Nushell users inspect, compose, and save plugin configuration as native
records instead of maintaining a second data format. Replace the TOML layer while
preserving existing setting precedence and credential safeguards.

## What Changes

- **BREAKING**: Read NUON records instead of TOML for all file-backed configuration.
  Discover `.nu_plugin_jev.nuon` locally and `nu_plugin_jev/config.nuon` per user;
  explicit paths also use NUON, without a TOML fallback.
- Retain optional fields, `timeout_ms`, cache limits, per-invocation reading,
  environment/flag precedence, and implicit-local transport restrictions.
- Parse data only, rejecting executable expressions, duplicate keys, unknown
  fields, and non-record roots without exposing parser snippets or credentials.
- Update command help, README migration guidance, and the repository usage skill.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `jev-shell-integration`: NUON file discovery, schema, safety, and migration.
- `jev-model-discovery`: Per-invocation model-list configuration uses NUON.
- `jev-http-transport`: Runtime protocol policy is independent of NUON settings.

## Impact

`src/config.rs`, command help, unit and real-Nu tests, README, and the usage skill.
Replace the direct `toml` dependency with the official `nuon` crate matching
Nu `0.116`. Existing TOML configuration requires explicit conversion; no user
configuration or API key is read, rewritten, or migrated by this change.
