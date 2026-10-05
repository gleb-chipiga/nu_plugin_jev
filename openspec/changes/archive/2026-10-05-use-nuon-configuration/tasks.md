# Tasks

## 1. Native NUON file settings

- [x] 1.1 Replace the direct TOML dependency and converter with matching public NUON parsing into native values; verify unit tests for partial settings, types, precedence, empty files, and invocation reloads.
- [x] 1.2 Preserve the 64 KiB bound, key permissions, implicit-local transport restrictions, and sanitized errors; verify tests reject executable expressions, duplicate/unknown keys, invalid roots, UTF-8, and oversized files without leaking credentials.

## 2. Discovery and user guidance

- [x] 2.1 Switch all live-command file discovery and help to NUON; verify real-Nu tests cover caller directories, explicit/env selection, old paths and TOML rejection, reloads, dry runs, model listings, annotation, and private file-backed keys.
- [x] 2.2 Update README, usage skill, and changelog with native record examples and a safe TOML conversion; verify the documented data-only conversion and retained setting names using synthetic files.

## 3. Integration verification

- [x] 3.1 Run fmt, all-target/all-feature Clippy, locked nextest, feature-off configuration tests, and prek; verify no findings and no real API requests or credential exposure.
- [x] 3.2 Check implemented behavior against every delta and task, and verify the planned specification changes preserve unrelated requirements before synchronization and archival.
