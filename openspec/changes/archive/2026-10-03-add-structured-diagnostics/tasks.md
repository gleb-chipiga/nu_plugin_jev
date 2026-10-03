# Tasks

## 1. Target selection and the `log` bridge

- [x] 1.1 Add the minimal `tracing-log` dependency and required subscriber features; verify the locked dependency graph builds and does not enable a `tracing`-to-`log` loop.
- [x] 1.2 Parse `NU_PLUGIN_JEV_LOG` into a strict target/level filter with the existing bare-level shorthand, implicit plugin `warn`, explicit overrides, and no `RUST_LOG`/`JEV_LOG` fallback; verify unit tests for defaults, nested targets, `off`, and malformed values.
- [x] 1.3 Initialize `LogTracer` and the non-blocking subscriber before Tokio, with a global `log` maximum derived from enabled directives; verify isolated-process tests that native `tracing` and synthetic dependency-target `log` records share the selected filter, while unlisted targets stay silent and startup errors are redacted.
- [x] 1.4 Document target syntax, startup/restart behavior, and the privacy risk of explicitly enabled dependency logs in README and `skills/jev-nushell/SKILL.md`; verify the examples match subprocess filter tests and still describe text output as the default.
- [x] 1.5 Run `cargo fmt`, `cargo clippy --all-targets --all-features`, and `cargo nextest run --all-features --all-targets --locked` after the filter and bridge stage; verify all pass before proceeding to the formatter.

## 2. NUON diagnostics

- [x] 2.1 Implement a compact NUON event serializer with safe string/key escaping and typed primitive field capture; verify unit round trips for quotes, newlines, Unicode, unusual keys, booleans, and finite numbers, plus string fallback for unsupported values.
- [x] 2.2 Capture recorded span fields in subscriber extensions and format `spans` root-to-leaf without synthetic lifecycle events; verify tests preserve `command` and `request_id` alongside typed event fields.
- [x] 2.3 Add strict startup parsing for `NU_PLUGIN_JEV_LOG_FORMAT=text|nuon` and route both native `tracing` events and bridged `log` records through the selected formatter and existing bounded non-blocking stderr writer; verify default text compatibility and explicit-format startup failures in isolated-process tests.
- [x] 2.4 Add real-Nu tests that parse plugin-authored NUON lines using `from nuon` and `from ndnuon`, check one physical line per emitted event, retain retry/cache request correlation, leave command stdout unchanged, and keep plugin-authored secrets redacted; verify those integration tests pass.
- [x] 2.5 Update README and `skills/jev-nushell/SKILL.md` with the NUON variable, compact record shape, a short Nushell parsing example, and the mixed-stderr caveat; verify the example against the real-Nu test fixture and keep planned behavior out of the usage skill until implementation lands.
- [x] 2.6 Run `cargo fmt`, `cargo clippy --all-targets --all-features`, and `cargo nextest run --all-features --all-targets --locked` after the NUON stage; verify all pass before final integration review.

## 3. Integration checks

- [x] 3.1 Run `cargo fmt --check`, `cargo clippy --all-targets --all-features`, and `cargo nextest run --all-features --all-targets --locked`; verify all pass without rebuilding dependencies in an alternate Cargo target directory.
- [x] 3.2 Run strict OpenSpec validation and inspect the final README/skill diff; verify the documented safety boundary matches default-off dependency logging and the implemented restart semantics.

## 4. Default-enabled NUON feature

- [x] 4.1 Put the NUON formatter and its exclusive dependencies behind a `nuon-tracing-format` Cargo feature included in `default`; retain text diagnostics without it and reject explicit NUON selection with a redacted startup error.
- [x] 4.2 Cover default/all-feature NUON behavior and no-default-feature text/filter behavior in tests; run formatting, clippy, nextest, and strict OpenSpec validation for both configurations.
- [x] 4.3 Document the default-enabled feature and opt-out behavior in README; keep `skills/jev-nushell/SKILL.md` focused on Nu-facing format selection and parsing, preserving the warning about explicitly enabled dependency logs.
