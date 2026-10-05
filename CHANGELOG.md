# Changelog

Notable changes to `nu_plugin_jev` are documented here.

## [Unreleased]

### Added

- Share a startup-configured HTTP attempt budget across concurrent Nu commands
  and retries; startup `NU_PLUGIN_JEV_MAX_IN_FLIGHT` overrides local/user NUON
  `max_in_flight`, then defaults to 128 independently of annotation's unchanged
  `--jobs 16` default. Changes require a plugin restart.
- Add `jev models` for uncached, authenticated model discovery with ordinary
  model records and shared transport settings.
- Report exact compact JSON body bytes in offline `jev ask --dry-run` and
  `jev annotate --dry-run` previews.
- Add opt-in HTTP measurements to `jev ask`, `jev annotate`, and `jev models`.
- Add selected service-root and HTTP measurements to successful completion diagnostics.

### Changed

- Replace file-backed TOML configuration with native NUON records in
  `.nu_plugin_jev.nuon` and user `nu_plugin_jev/config.nuon`; retain precedence,
  per-invocation reloads, and credential safeguards. Convert existing TOML
  contents rather than renaming the extension.
- Share table questions and preconvert valid static context across rows.
- Reject successful API response bodies larger than 16 MiB with a nonretryable
  response error before JSON decoding, including chunked bodies.
- Default `jev annotate` answers to the `answers` field, aligning row paths
  with `jev ask`; use `--into jev` to retain the former default path.
- Wrap successful dry-run bodies as `{request, request_bytes}`; access former
  top-level request fields through `request`.
- Return always-present `meta` on live `jev ask` and `jev models`, and
  `jev_meta` on successful annotation rows. Use `get models` for model-table
  pipelines; annotation's former `--meta` flag is removed.

### Fixed

- Stop remaining annotation work before waiting to deliver an observed terminal
  error to a slow consumer, preserving the original failure.
- Keep dispatched annotation requests and deadlines progressing with full
  output so slow consumers cannot retain shared HTTP slots indefinitely.
- Prevent interrupts from being missed between signal checking and handler
  registration in live commands.

## [0.1.1]

### Changed

- Run Rust and real-Nushell integration tests with cargo-nextest locally and in CI.
- Update Rust dependencies and pinned CI actions.
- Trim unused `tracing-subscriber` features.

### Added

- Show the published crates.io version in the README.

## [0.1.0]

### Added

- Initial Nushell plugin for TypeSafe Jev System One with structured state,
  named Noul, Choice, and Score questions, and typed answers.
- `jev ask`, streaming `jev annotate`, and offline `jev question` constructors.
- Request previews, row projections, bounded concurrency, cancellation,
  retries, in-flight deduplication, and a bounded per-invocation result cache.
- Layered TOML and environment configuration, proxy policy, and redacted
  tracing diagnostics.
- GitHub CI and release automation for seven platform archives, checksums,
  attestations, and crates.io publication.
