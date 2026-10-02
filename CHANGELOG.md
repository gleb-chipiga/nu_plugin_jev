# Changelog

Notable changes to `nu_plugin_jev` are documented here.

## [Unreleased]

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
