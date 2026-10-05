# Design

## Context

`ConfigSources` currently reads a user and a local TOML file before command input
is consumed. Each field resolves independently, and live commands enforce file
permissions and implicit-local endpoint restrictions. See proposal.md for scope.

## Goals / Non-Goals

Use native Nu values end to end for file settings while retaining the existing
resolver, per-invocation snapshot, 64 KiB file bound, and transport/key safeguards.
Do not add settings, reload watchers, persistent caches, automatic migration,
TOML compatibility, command flags, or runtime code evaluation.

## Decisions

- Use `nuon = "=0.116"`, matching the plugin's Nu crates, rather than a handwritten
  parser or invoking Nushell. `nuon::from_nuon` returns `nu_protocol::Value` and
  rejects executable expressions and duplicate record fields. Its internal
  dependencies are not called directly by this plugin.
- Replace `TomlSettings` with private file settings containing the parsed native
  values. Remove the TOML-to-Nu conversion and the direct `toml` dependency.
- Preserve file keys and types, including integer `timeout_ms` and integer byte
  cache limits. NUON support does not add a `timeout` alias or duration coercion.
  Lower-priority recognized values still receive semantic validation only when
  relevant and selected; unknown keys remain structural errors.
- Treat empty, whitespace-only, and comment-only sources as empty records;
  otherwise require one root record. Preserve current input limits and read on
  the synchronous command side, not Tokio workers.
- Discard NUON parser diagnostics rather than exposing their source snippets,
  inner errors, or arbitrary keys. Report a sanitized configuration-layer error.
- Discover only the new filenames; an explicit path always means NUON regardless
  of extension. README provides `open <old.toml> | save <new.nuon>` and private
  permission guidance. No real credentials or user files are inspected.

## Risks / Trade-offs

- Existing TOML is incompatible → document conversion rather than silently use
  stale settings or maintain two parser paths.
- NUON's parser adds production dependencies → reuse matching public crates
  already present through test support and verify feature-independent builds.
- Rich parser errors can contain keys → drop them at the file boundary and test
  complete error records for credential leakage.

## Migration Plan

Users convert their selected TOML file with native `open` and `save`, preserve
owner-only permissions, and update explicit selection paths. Existing files are
not removed. Reverting the plugin version requires retaining the original TOML.
