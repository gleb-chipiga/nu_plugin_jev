# Tasks

## 1. Package and Nushell integration

- [x] 1.1 Create the MIT-licensed binary-only `nu_plugin_jev` crate with a lockfile, Rust edition 2024, matching public Nushell `0.116.x` dependencies, and a default-enabled optional `mimalloc` feature; verify the system-allocator build with `--no-default-features`.
- [x] 1.2 Keep `main.rs` thin: initialize non-blocking stderr tracing before an explicitly built Tokio runtime, then serve the plugin through MessagePack. Share the runtime and reusable HTTP clients without blocking Tokio workers or writing diagnostics to the protocol stream.
- [x] 1.3 Register exactly `jev`, `jev ask`, `jev annotate`, and the three offline `jev question` constructors. Verify discovery, command help, and offline root guidance in Nushell.
- [x] 1.4 Document and verify locked installation and registration with a compatible Nu `0.116.x` binary in an isolated session.

## 2. API contracts and state construction

- [x] 2.1 Implement typed System One requests, tagged questions and answers, and response validation. Preserve question names, omitted versus explicit-null optional fields, probabilities, confidence, fractional scores, model, and usage; reject invalid answer contracts.
- [x] 2.2 Convert supported Nu values recursively to structured JSON and responses back to ordinary Nu values. Define date, duration, and filesize representations; reject unsupported or lossy values with location-aware errors.
- [x] 2.3 Validate final string/object/array state and compose explicit `--context` as `{input, context}` without merging or stringifying records. Convert only the selected outbound row fields.
- [x] 2.4 Implement offline Noul, Choice, and Score question constructors with structured instructions and descriptions, documented cardinality limits, and validation distinct from the raw API schema.

## 3. Invocation configuration and credentials

- [x] 3.1 Resolve each setting independently from flags, `$env.config.plugins.jev`, caller environment, selected local TOML, user TOML, then defaults. Validate selected values and snapshot configuration once per invocation, including positive cache limits for table commands.
- [x] 3.2 Use `NU_PLUGIN_JEV_MODEL`, `NU_PLUGIN_JEV_BASE_URL`, `NU_PLUGIN_JEV_TIMEOUT_MS`, `NU_PLUGIN_JEV_JOBS`, `NU_PLUGIN_JEV_RETRIES`, `NU_PLUGIN_JEV_PROXY`, and `NU_PLUGIN_JEV_CONFIG` for caller-scoped plugin settings, plus process-start `NU_PLUGIN_JEV_LOG`. Verify prior setting names are not aliases.
- [x] 3.3 Discover only caller-local `.nu_plugin_jev.toml` and per-user `nu_plugin_jev/config.toml`, without ancestor search or implicit legacy-file fallback. Keep `--config` ahead of `NU_PLUGIN_JEV_CONFIG`, resolve relative paths from the calling Nu directory, permit explicit selection of an old-named file, and reload files for the next invocation.
- [x] 3.4 Select live credentials from caller `TYPESAFE_API_KEY`, then local TOML `api_key`, then user TOML `api_key`; provide no key flag or ordinary plugin-config key. Keep root guidance, constructors, and dry runs usable without credentials.
- [x] 3.5 Bound TOML size, reject malformed or unknown fields and insecure key-bearing file permissions on Unix, and prohibit `base_url` or `proxy` in automatically discovered local TOML. Verify credentials, proxy URLs, and raw file contents are redacted from errors and diagnostics.

## 4. HTTP transport and diagnostics

- [x] 4.1 Reuse authenticated reqwest clients for `POST /v1/systemone`, validate service roots, prevent authenticated redirect forwarding, and negotiate HTTP/2 with HTTP/1.1 fallback.
- [x] 4.2 Implement `auto`, `direct`, and authoritative HTTP/SOCKS5h proxy policies with bounded client-pool reuse. Verify per-invocation plugin-proxy changes, process-start ordinary proxy discovery, `NO_PROXY` behavior, and no silent fallback from an explicit proxy.
- [x] 4.3 Retry only the configured HTTP statuses with the additional-attempt budget, `retry-after-ms`/`Retry-After` precedence, cancellable bounded backoff, and one total evaluation deadline. Disable reqwest's hidden protocol-NACK retries and verify zero retries means one wire attempt.
- [x] 4.4 Emit stable, redacted error categories and non-blocking diagnostics correlated by local `request_id`; include only bounded, sanitized server request IDs at debug level without changing Nu response shapes.

## 5. Evaluation commands and Nu composition

- [x] 5.1 Implement `jev ask` for one string, record, list, or finite stream state and all named questions in one evaluation. Return the complete typed API envelope; document that a list is one array state, not independent row requests.
- [x] 5.2 Implement `jev annotate` for independent record rows with whole-row, `--state` cell-path, or `--fields` literal-field input selection. Preserve the complete source row while adding answers and optional model/usage/request metadata.
- [x] 5.3 Validate selectors and output destinations before dispatch, prevent overwrites, and implement `--on-error fail|keep|record` without fabricating decisions or hiding upstream errors.
- [x] 5.4 Make `--dry-run` on both evaluation commands emit the exact request body without a key or network request; keep annotation previews incremental, ordered, and duplicate-preserving.
- [x] 5.5 Add the focused short flag aliases and verify their long-form equivalence. Document native Nu `get`, `where`, `sort-by`, and `reject` for projection and filtering; do not add plugin commands for those operations or automatic row packing.

## 6. Streaming, cancellation, and reuse

- [x] 6.1 Bound table admission and output queues, retain at most `2 * jobs` unconsumed row outcomes and `jobs` unique evaluations, and provide ordered output by default with optional `--unordered` readiness output.
- [x] 6.2 Cancel input admission, pending HTTP/retry work, and blocked bridge operations on engine interruption or downstream output drop. Verify `first 10`, stalled requests, and ordered-output backpressure through real Nu subprocess tests.
- [x] 6.3 Share identical in-flight requests by canonical complete body and service root, including across retries, without retaining credentials in keys or admitting unbounded duplicate waiters.
- [x] 6.4 Retain successful results only in an invocation-local LRU bounded by entry count and approximate bytes. Verify cache hits, eviction, oversized-entry bypass, failure non-caching, and fresh evaluation after an entry is forgotten.
- [x] 6.5 Preserve one local request identity across retries, in-flight sharing, and cache reuse; assign a new identity to a fresh evaluation so reported usage can be counted once per logical request.

## 7. Documentation and verification

- [x] 7.1 Keep README, command help, and `skills/jev-nushell/SKILL.md` aligned with implemented commands, configuration names and migration, proxy lifetimes, credential safety, retry behavior, and native Nu workflows.
- [x] 7.2 Cover contracts, configuration precedence, transport, streaming, cancellation, and command examples with unit, local mock-server, plugin-command, and real Nu tests; verify a synthetic live request without exposing the key.
- [x] 7.3 Run `cargo fmt`, `cargo clippy --all-targets --all-features`, `cargo test --all-features`, the skill validator, and strict OpenSpec validation against the final implementation.
