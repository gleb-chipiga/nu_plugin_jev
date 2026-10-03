# Tasks

## 1. Scoped configuration and model-list transport

- [x] 1.1 Add a models-only configuration scope and transport settings type that reuse existing precedence, credential, proxy, and TOML validation without requiring `model`, `jobs`, or cache settings; verify unit tests for flags/config/environment/file precedence, missing or changed caller keys, unrelated invalid settings, and the implicit-local-file transport boundary.
- [x] 1.2 Add typed model-list API structs and a pooled authenticated `GET /v1/models` client path, sharing bounded retry/deadline/cancellation policy with POST without changing evaluation behavior; verify local mock tests for bodyless GET, trailing-slash root, empty and multi-entry lists, extra/missing/wrong-typed fields, opaque release-date strings, retry guidance, 401, redirects, timeout, cancellation, and credential-redacted errors.
- [x] 1.3 Run `cargo fmt`, `cargo clippy --all-targets --all-features`, and `cargo nextest run --all-features --all-targets --locked` after transport/config work; verify the existing POST suite still passes using the normal Cargo target directory.

## 2. Nushell command and user guidance

- [x] 2.1 Implement `jev models` as a non-materializing `PluginCommand` with `--base-url`, `--timeout`, and `--config`, producing ordered Nu records from typed metadata; verify command tests for native field types, empty lists, missing key, no unrelated settings requirement, and early rejection of nonempty `PipelineData` without an HTTP call.
- [x] 2.2 Register the seventh command and update offline root/help guidance; verify real-Nu tests for `help jev models`, command inventory, `jev models | where/select/sort-by`, input-stream rejection, fresh results across two invocations in one plugin process, and no listing preflight for `jev ask` or `jev annotate`.
- [x] 2.3 Update README, command examples, CHANGELOG, and `skills/jev-nushell/SKILL.md` with the implemented listing, key requirement, transport flags, and informational/non-cached semantics; verify documented examples against a mock service, then run `cargo fmt`, `cargo clippy --all-targets --all-features`, and `cargo nextest run --all-features --all-targets --locked` for this stage.

## 3. Integration checks

- [x] 3.1 Run the full formatting, Clippy, and locked nextest checks plus strict OpenSpec validation; verify the public command output and user guidance agree, no real TypeSafe key is needed for tests, and no unrelated pending change is altered.
