# Proposal

## Why

The plugin currently negotiates HTTP/2 but accepts HTTP/1.1 fallback. Default
builds should use reqwest's HTTP/2 prior-knowledge mode for every live API
call, while a non-default Cargo build can retain the existing negotiation.

## What Changes

- **BREAKING for default builds:** Force HTTP/2 for System One evaluations and
  model discovery, without HTTP/1.1 fallback.
- Add a default-enabled `http2-prior-knowledge` Cargo feature. Disabling this
  feature restores the existing HTTP version negotiation at build time, with
  no command, environment, or TOML switch.
- Keep request bodies, authentication, retry policy, proxy selection, and
  result shapes unchanged; incompatible routes fail as transport errors.
- Use a ready-made HTTP/2 server for network tests of the real plugin binary;
  keep pure unit tests independent of network transport. Cover both Cargo
  feature modes and document the endpoint compatibility requirement.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `jev-http-transport`: make HTTP/2 mandatory in default builds for
  evaluations and model discovery, retain negotiation when the feature is
  disabled, and cover protocol metrics and streamed responses.

## Impact

`Cargo.toml`, `src/api/client.rs`, local HTTP test fixtures, and README change.
Default builds stop working with HTTP/1.1-only
custom endpoints and routes; a build without `http2-prior-knowledge` retains
the prior behavior. No runtime configuration source or flag is added.
