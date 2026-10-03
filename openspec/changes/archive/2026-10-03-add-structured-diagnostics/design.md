# Design

## Context

See [proposal.md](proposal.md) for motivation and [the shell-integration delta](specs/jev-shell-integration/spec.md) for the observable contract. `src/tracing.rs` already installs a non-blocking stderr writer before the explicit Tokio runtime starts. It currently accepts only a bare `NU_PLUGIN_JEV_LOG` level and filters to the `nu_plugin_jev` target, so records emitted through dependencies' `log` API never enter its subscriber. Existing real-Nu tests inspect text diagnostics and request IDs. The plugin may serve concurrent commands in one process, while callers can change their Nu environment between invocations.

## Goals / Non-Goals

**Goals:**

- Make one process-start filter control both plugin and explicitly selected Rust dependency targets without changing bare-level behavior.
- Preserve structured event fields and evaluation span context in a Nushell-readable, newline-delimited format.
- Keep diagnostic I/O off Tokio worker threads and outside the plugin protocol.

**Non-Goals:**

- A plugin-managed log file, rotation, per-command log filters, or changes to Nu command output.
- Enabling undocumented dependency instrumentation, `reqwest::ClientBuilder::connection_verbose`, Hyper's unstable tracing feature, or span lifecycle events merely because a target is selected.
- Recovering typed fields from a third-party message that was emitted only as formatted text, or promising lossless diagnostics when the non-blocking queue drops records.

## Decisions

### One target/level filter with a separate output-format setting

Retain `NU_PLUGIN_JEV_LOG` for verbosity. Normalize its existing bare values to `nu_plugin_jev=<level>`. Parse an advanced list of explicit `target=level` directives, starting with `nu_plugin_jev=warn` and letting a user-supplied plugin base directive replace it. Do not set a global level: an unlisted dependency remains off. Prefer `tracing_subscriber::filter::Targets` over the broader `EnvFilter`, because only target and level selection is needed; span-field regular expressions add complexity and are not part of this contract. Validate the complete value strictly and emit a value-redacted startup error on malformed input. Ignore `RUST_LOG` and `JEV_LOG` rather than introducing hidden precedence.

Use `NU_PLUGIN_JEV_LOG_FORMAT=text|nuon`, defaulting to text. Level selection and serialization are orthogonal, so combining both into one environment variable would create a plugin-specific mini-language. Both values are read once before the runtime starts; `plugin stop jev` is required after changing them in an existing Nu session. Do not read them from per-invocation TOML or ordinary plugin config, which cannot safely reconfigure a process-global subscriber while concurrent calls run.

Compile the NUON formatter only with the `nuon-tracing-format` Cargo feature and include that feature in `default`. Keep the text formatter and target filter available without default features. Make the formatter's time dependency optional; shared JSON dependencies remain unconditional because the API uses them. In a build without `nuon-tracing-format`, reject an explicit `NU_PLUGIN_JEV_LOG_FORMAT=nuon` at startup with a value-redacted feature-unavailable error rather than silently changing formats. Gate NUON-specific tests, while running the core text diagnostics tests in both feature configurations.

### Bridge `log` records once at process startup

Install `tracing_log::LogTracer` alongside the global `tracing` subscriber before entering Tokio. This converts dependency `log` records, including those from `reqwest`, into events carrying their original target and level. Route them through the same target filter and non-blocking stderr writer as native `tracing` events. Check the existing feature graph and avoid double-reporting plugin events if a `tracing`-to-`log` compatibility feature is enabled. Set the bridge's global `log` maximum to the highest level required by any enabled target directive, including the plugin target; the target filter still rejects all unlisted dependencies.

Selecting `reqwest=trace` cannot manufacture events that the crate does not emit. In particular, it does not switch on its separate connection-verbose option. The bridge also does not intercept direct `stderr` writes, panics, or logging systems other than `log` and `tracing`.

### Emit a compact NUON record for each event

Use a custom `tracing_subscriber::fmt::FormatEvent` and field visitor for `nuon` mode while retaining the existing text formatter for `text`. The record envelope is `{timestamp, level, target, message, fields, spans}`. `spans` is ordered root-to-leaf and carries recorded span fields such as `command` and local `request_id`; it is context, not a stream of synthetic span lifecycle events. Store typed span fields in subscriber extensions so they survive until event formatting. Record booleans, finite numbers within Nu's representable range, and strings as their corresponding NUON values. Render debug-only, error, non-finite, or out-of-range values as escaped strings. Keep `message` separate from other event fields.

Serialize one compact record atomically per event. Use NUON record syntax with safely escaped keys and string values; JSON-compatible string literals provide unambiguous escaping without adding another Nushell engine dependency. Verify output against Nu 0.116's `from nuon` and standard-library `from ndnuon`, including quotes, newlines, Unicode, and unusual field names. The format is newline-delimited NUON, not one NUON document containing all events. Keep ANSI disabled and use the existing bounded non-blocking writer and shutdown guard. A captured whole-Nu stderr stream can also contain Nu's own messages, so only plugin-authored lines are guaranteed to follow this schema.

### Make the third-party privacy boundary explicit

The plugin controls its own diagnostic fields and continues to omit credentials and request bodies there. It cannot comprehensively redact arbitrary messages emitted by dependencies after a caller explicitly enables their targets. Keep those targets off by default; document that users should treat enabled dependency output, particularly `trace`, as potentially sensitive. Do not broaden HTTP-client verbosity or add a file sink implicitly. This changes the existing blanket logging promise only for explicitly selected third-party events; return values, previews, errors, and plugin-authored logs retain their existing protections.

## Risks / Trade-offs

- **Third-party record contains secrets** -> Keep dependency targets off by default, require explicit target directives, and state the risk next to configuration examples. Test that default and bare-level modes remain credential-redacted.
- **Formatter turns one event into several physical lines or invalid NUON** -> Escape values centrally and round-trip diverse events through real Nu parsers in tests.
- **NUON formatter obscures request correlation** -> Preserve span fields in the `spans` list and assert that retries and cache hits retain the current local identity.
- **Extra logging overhead** -> Keep a bounded non-blocking writer, use a static target-level filter, and limit the `log` bridge's global maximum when no dependency targets are requested.
- **Mixed stderr from Nushell** -> Document that redirecting a whole Nu process can include non-plugin messages; the plugin controls only its own emitted records.
- **Log or tracing event unavailable** -> Document that filters cannot enable instrumentation omitted by a dependency build or disabled by an independent client option.

## Migration Plan

No migration is required for existing `NU_PLUGIN_JEV_LOG=<level>` settings: text stderr and plugin-only filtering remain the defaults. Implement the filter and bridge first, then the NUON formatter and round-trip tests, followed by README and repository-skill guidance. Rollback can restore the previous initializer and remove the new format variable and bridge without changing command data or stored user data.
