# Proposal

## Why

`NU_PLUGIN_JEV_LOG` currently controls only one level for plugin-owned `tracing` events. It cannot select diagnostics from Rust dependencies such as `reqwest`, and the human-readable stderr output is not directly usable as structured Nushell data.

## What Changes

- Extend `NU_PLUGIN_JEV_LOG` to accept per-target levels for plugin and dependency diagnostics while preserving `off`, `error`, `warn`, `info`, `debug`, and `trace` as plugin-only shorthand values. Dependency targets remain disabled by default.
- Bridge `log` records into the existing `tracing` subscriber so a single filter and formatter cover both instrumentation systems.
- Add an opt-in `NU_PLUGIN_JEV_LOG_FORMAT=nuon` formatter that emits one compact, parseable NUON record per event on stderr. Keep the current text format as the default and preserve the plugin protocol's stdout.
- Put the NUON formatter behind the `nuon-tracing-format` Cargo feature, enabled in the default feature set. A build without it keeps text diagnostics and rejects an explicit `nuon` selection.
- Document that explicitly enabled third-party diagnostics may contain sensitive data outside the plugin's redaction control. Keep plugin-owned events credential-redacted, and do not enable connection-level payload tracing implicitly.
- Keep logging configuration process-scoped: changes take effect after restarting the plugin, not between concurrent command invocations. No plugin-owned log file or command flags are added.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `jev-shell-integration`: Expand process-level diagnostics to support target filters, `log` bridging, an opt-in NUON output format, and an explicit third-party logging safety boundary.

## Impact

Planning affects `src/tracing.rs`, startup initialization, the tracing dependency set, real-Nu diagnostic tests, README diagnostics guidance, and `skills/jev-nushell/SKILL.md`. The command inventory, HTTP requests, response values, and default stderr presentation remain unchanged.
