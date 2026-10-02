# Design

## Context

See [proposal.md](proposal.md) for motivation and capability boundaries. At the start of this change, the repository contained OpenSpec configuration and project instructions but no plugin implementation. This is a new binary plugin with no pre-existing behavior to migrate.

The current [TypeSafe OpenAPI](https://api.typesafe.ai/openapi.json) identifies the contract as `0.2.0`. The [API reference](https://docs.typesafe.ai/api) additionally describes operating limits and HTTP `529`. The design uses the schema for wire shapes and treats documented service limits separately. The [Nushell plugin guide](https://www.nushell.sh/contributor-book/plugins.html) supports the proposed naming, public crates, configuration access, and streaming command model.

## Goals / Non-Goals

**Goals:**

- Share one request builder and reuse HTTP clients by effective proxy policy so `ask`, `annotate`, and previews agree on the submitted body.
- Expose only the evaluation primitives and question constructors; use native Nu commands to extract values, choose thresholds, and transform annotated rows.
- Keep API types independent of Nu values, and keep scheduling independent of individual command signatures.
- Make concurrency, cancellation, ordering, duplicate evaluations, and row errors explicit and testable without a live account.
- Preserve a small binary entry point and predictable resource ownership throughout a returned stream's lifetime.

**Non-Goals:**

- A new policy language, callbacks into Nu for per-row closures, or a planner for downstream Nu operations.
- Scalar shortcut commands `jev noul|choice|score`, semantic filtering through `jev where`, and model discovery through `jev models`.
- Automatic or opt-in packing of independent rows into a shared request state; evaluating such a mode requires a separate change and quality/cost validation.
- Account-wide concurrency coordination, persistent result caching, exactly-once execution at the remote service, or invoice reconciliation.
- An absolute process-memory ceiling for arbitrarily large individual states and outcomes.

## Decisions

### 1. Binary organization and process ownership

Use package/crate/binary `nu_plugin_jev`, package version `0.1.0`, Rust edition `2024`, and the `jev` namespace. Match the `0.116.x` line for `nu-plugin`, `nu-protocol`, and test support, and commit a lockfile. Depend directly only on the public Nushell plugin/protocol crates; private engine and protocol internals are not extension points.

Enable the optional `mimalloc` Cargo feature by default, using its `v3` allocator integration for the plugin process. Building with `--no-default-features` uses Rust's default system allocator. This choice changes process memory management, not Nu command or wire contracts.

The planned module boundaries are:

```text
src/
  main.rs             process entry and explicit runtime construction
  app.rs              plugin application entry
  tracing.rs          non-blocking tracing initialization
  plugin.rs           shared client pool/runtime and command registration
  config.rs           per-call configuration snapshots
  error.rs            transport and user-facing error mapping
  api/
    client.rs         authenticated HTTP and retries
    types.rs          wire types and validation
  nu/
    value.rs          recursive conversion and request-state composition
    stream.rs         scheduling, cancellation, deduplication, output lifetime
  commands/
    root.rs, ask.rs, annotate.rs
    question/         offline question constructors
tests/                process-level Nu integration
```

There is no `lib.rs`. Internal types use the narrowest visibility required; module, type, and function documentation is brief and in English. Use specific types for validated jobs, deadlines, sequence numbers, and request identities where they prevent accidental misuse.

Initialize tracing before entering the runtime. `main.rs` creates the runtime with `tokio::runtime::Builder`, retains the tracing worker guard, and delegates to one application entry function. `JevPlugin` owns a shared runtime and a bounded cache of reusable `reqwest::Client` instances keyed by effective proxy policy; invocation data and cancellation are separate. The default automatic-proxy client is created at process startup. Runtime worker threads perform asynchronous work only. Nu iterator reads, blocking channel operations, and engine configuration calls stay on command or dedicated producer threads; client construction also stays outside Tokio workers.

A non-blocking tracing writer targets stderr, preserving stdout for the plugin protocol and Nu pipeline values for command results. `NU_PLUGIN_JEV_LOG` selects the process-wide level at startup, defaulting to `warn`; changing it in an already running Nu session requires restarting the plugin process. `info` exposes logical evaluation starts, completions, and failures with local request identities, duration, and available model/usage or error details. `debug` additionally exposes HTTP attempts, retry waits, and table-result reuse. Events never include bearer credentials or state/question bodies. Configure authentication per invocation rather than in a process-global default header.

Alternative: a library crate or separate runtime/client per command would add packaging and connection-pool complexity without a consumer that needs it. A single immutable client for every proxy policy would also prevent caller-scoped Jev proxy changes from taking effect in a reused plugin process.

### 2. Request building and Nu value conversion

Centralize conversion of ordinary Nu values to JSON. Records and lists remain objects and arrays; strings are not interpreted as JSON or NUON. Convert dates to RFC3339, durations to an exact signed nanosecond string such as `1000000000ns`, and file sizes to integer byte counts. Reject binary, closure, range, custom, cell-path-as-data, non-finite float, and other unsupported values with an error that identifies the offending path. Preserve an incoming error rather than converting it into data.

Choose each table row's outbound input before conversion: the whole record by default, the value selected by `--state`, or the record projected by `--fields`. Convert only that chosen input, then apply context wrapping if the flag is present. Unselected fields remain in the source row without being converted or transmitted. Context presence is distinct from a supplied `null`:

```text
without --context: converted input
with --context:    {input: converted input, context: converted context}
```

Validate the final state as string/object/array. Thus nested scalars remain valid, and a scalar input can become valid inside an explicitly requested context object. No implicit merge, text encoding, field selection, or summarization occurs; projection happens only when `--fields` is supplied.

Convert response JSON recursively to Nu values without retyping date-like strings. Reject integers that cannot be represented losslessly by a Nu integer. Nu source spans are attached by the command layer rather than stored in wire types or reused from another row.

Alternative: stringify everything would discard the structured-state contract and make request previews misleading.

### 3. Schema contract and constructor validation

Use tagged question/answer types and typed request/response envelopes. Validate the named question map before consuming a table. Keep omitted optional fields distinct from explicit `null`, so a raw request and its preview preserve that distinction.

At a question field's root, instructions and Noul/Choice descriptions accept string/object/array/null; numbers and booleans are valid only inside structured values. Score level descriptions accept string/object/array and exclude root null. The named question map is nonempty. This follows the current [wire schema](https://api.typesafe.ai/openapi.json).

`jev ask` and the raw question maps passed to `jev annotate` validate schema shapes and the schema's Score minimum of one level. They do not invent cardinality maxima missing from the schema. The question constructors provide a usable interface: Choice accepts 1-255 distinct names and Score accepts 1-10 levels, with documentation recommending at least two Score levels. The maxima come from the [service reference](https://docs.typesafe.ai/api). This distinction is documented rather than presented as schema enforcement.

Choice's list shorthand maps each string to a null description. Duplicate list names are errors rather than silently collapsed keys. Constructors emit ordinary records and do not read credentials or access the network. Required positional instructions use Nu's `any` shape, followed by the API-specific root-shape validation. Noul only emits criteria keys for explicitly supplied `--yes`/`--no` flags.

Check that a response's answer names and variants match the questions, its required fields are present, and numeric results are representable and within their stated domains. Preserve server `confidence`, distributions, legend, resolved model, and usage; do not recompute confidence or turn probabilities into booleans.

Alternative: requiring instructions everywhere or allowing arbitrary root JSON scalars would contradict the schema in opposite directions.

### 4. Configuration and command surface

Resolve each applicable setting independently using command flag, `$env.config.plugins.jev`, caller environment, selected local TOML, per-user TOML, then default. This inserts file-backed defaults below all existing sources without reordering them. Every TOML field is optional, including `api_key` and each nested cache limit; an empty TOML file is valid. Omitted values inherit from the next source, independently for each field. Validate the selected value; an invalid explicit value is an error rather than a fallback. A malformed present TOML file or unknown TOML key is an error even if another source supplies a value for a field. Fetch the caller context and read the small TOML files on the synchronous command thread before returning a stream; hold an immutable per-call snapshot without per-row filesystem or engine access.

Namespace plugin-owned environment settings and discovered file names under `nu_plugin_jev`, distinguishing them from settings for other Jev clients. Keep `$env.config.plugins.jev` because `jev` is the Nu command namespace, and keep `TYPESAFE_API_KEY` because it is a service credential rather than a plugin setting. Replace the interim `TYPESAFE_MODEL`, `TYPESAFE_BASE_URL`, `TYPESAFE_TIMEOUT_MS`, `TYPESAFE_JOBS`, `TYPESAFE_RETRIES`, `JEV_PROXY`, `JEV_CONFIG`, and `JEV_LOG` names without compatibility aliases. Do not implicitly discover the old `.jev.toml` or `jev/config.toml` paths: silent dual discovery obscures effective configuration. Explicit `--config` may still select a file with either old name. Document the rename and the actions required for existing local installations.

| Setting | Flag | Plugin config | Environment | TOML key | Default |
| --- | --- | --- | --- | --- | --- |
| Model | `--model` | `model` | `NU_PLUGIN_JEV_MODEL` | `model` | `jev-latest` |
| Base URL | `--base-url` | `base_url` | `NU_PLUGIN_JEV_BASE_URL` | `base_url` | `https://api.typesafe.ai` |
| Evaluation deadline | `--timeout` | `timeout` | `NU_PLUGIN_JEV_TIMEOUT_MS` | `timeout_ms` | `30sec` |
| Table jobs | `--jobs` | `jobs` | `NU_PLUGIN_JEV_JOBS` | `jobs` | `16` |
| Additional attempts | none | `retries` | `NU_PLUGIN_JEV_RETRIES` | `retries` | `3` |
| Jev proxy policy | none | `proxy` | `NU_PLUGIN_JEV_PROXY` | `proxy` | `auto` |
| Completed-cache entries | none | `cache.max_entries` | none | `cache.max_entries` | `1024` |
| Completed-cache approximate bytes | none | `cache.max_approx_bytes` | none | `cache.max_approx_bytes` | `16777216` (16 MiB) |

The optional per-user file is `nu_plugin_jev/config.toml` in the platform's user configuration directory: `$XDG_CONFIG_HOME/nu_plugin_jev/config.toml` when that caller-scoped variable is an absolute path, otherwise the platform default (for example `$HOME/.config/nu_plugin_jev/config.toml` on Linux). An absent implicit user file is ignored. Resolve the local file on every `jev ask` or `jev annotate` call from `EngineInterface::get_current_dir()`, never the plugin process working directory: Nu can reuse one plugin process for concurrent calls from different directories, while its process directory is the executable's directory. By default read only `<caller-current-dir>/.nu_plugin_jev.toml`, without ancestor traversal. `--config <path>` selects a different local file; caller-scoped `NU_PLUGIN_JEV_CONFIG` selects one when the flag is absent. Relative explicit paths are resolved against the caller's current directory. An explicitly selected missing or unreadable file is an error; an absent implicit `.nu_plugin_jev.toml` is ignored. If an explicit path happens to name the user file, load it once. File changes are seen on the next invocation without restarting the plugin; one invocation's rows keep their initial snapshot. Do not change the process current directory, cache a single global file snapshot, or construct a new HTTP client per row.

Automatically discovered local files are a lower-trust source: reject `base_url` or `proxy` in an implicit `.nu_plugin_jev.toml`, even if a higher-priority source supplies that setting. Otherwise a repository file could redirect a caller's environment/user-file API key or bypass the expected proxy. An explicitly selected local file is trusted to set those transport fields; the user may also choose them through existing flags, Nu config, or environment. Report this restriction without printing sensitive values. Local files may set other settings and `api_key`; users should keep key-bearing local files out of version control.

TOML uses native strings and integers, with `timeout_ms` as a positive integer count of milliseconds and `[cache]` for the two positive integer limits. A user file containing only `api_key = "..."` is valid; documentation must use a placeholder, never a real credential. Parse syntax before semantic precedence but validate each recognized setting only when selected, so an overridden lower-priority value cannot unexpectedly override a valid explicit choice. Bound file size and sanitize parse/IO errors: parser excerpts, debug output, logs, help, previews, and returned records must not reveal `api_key`, proxy credentials, or raw file contents. Key-bearing files should be owner-only; on Unix reject group/other-accessible key-bearing files before dispatch, and document platform-appropriate private-file permissions elsewhere.

Flag/config timeouts are positive Nu durations; the environment timeout is a positive integer count of milliseconds. Jobs are positive integers and retries are nonnegative integers. The base URL is an absolute HTTP(S) service root without credentials, query, or fragment; joining endpoint paths accepts a trailing slash. Plain HTTP allows local mock servers. The proxy policy is a string: `auto`, `direct`, or an explicit `http://` or `socks5h://` proxy URL. It has no command flag. An invalid selected policy fails rather than falling back to a lower-priority source. Explicit proxy URLs are sensitive configuration values and must be redacted from diagnostics, errors, and previews.

Cache limits apply to table invocations and resolve independently from the nested plugin-config `cache` record, local TOML, user TOML, or their defaults. Both are positive integers; `max_approx_bytes` is expressed in bytes. No cache-specific flag or environment variable is added. Validate selected limits before consuming rows. For example, `$env.config.plugins.jev.cache = {max_entries: 1024, max_approx_bytes: 16777216}` sets both defaults explicitly.

For live calls, select `TYPESAFE_API_KEY` from the caller's environment, then `api_key` from local TOML, then `api_key` from user TOML. A present but empty or invalid higher-priority key is an error, not a fallback. Do not accept a key flag or a plugin-config key. Offline constructors, root usage, and request previews do not need a key; they may still parse file-backed non-secret settings but must not output the key. No environment, filesystem, or configuration calls occur for each table row.

| Commands | Flags beyond help |
| --- | --- |
| `jev ask` | `--model`, `--base-url`, `--timeout`, `--context`, `--config`, `--dry-run` |
| `jev annotate` | ask flags plus `--jobs`, `--unordered`, `--state`, `--fields`, `--into`, `--meta`, `--on-error` |
| `jev question noul` | `--yes`, `--no` |
| `jev question choice`, `jev question score`, `jev` | no network flags |

Add short aliases only for frequently used options: `jev ask` supports `-c`/`--context` and `-m`/`--model`; `jev annotate` supports those plus `-j`/`--jobs`, `-s`/`--state`, `-f`/`--fields`, and `-i`/`--into`. Keep every long spelling and leave all other flags, including the new `--config`, long-only. Nushell command signatures attach each alias to its existing flag, so both spellings reach the same parsing and configuration path. In particular, mixing short and long spellings does not bypass the `--state`/`--fields` exclusion. Update command help, README examples, and the repository-owned usage skill when the aliases are implemented, not before.

The root command returns offline usage guidance. The complete public inventory is `jev`, `jev ask`, `jev annotate`, and the three `jev question` constructors. Single-answer extraction uses ordinary Nu field access, for example `$state | jev ask {spam: (jev question noul "Is this spam?")} | get answers.spam.noul`. Filtering uses `jev annotate` followed by native `where`; optional removal of answer fields uses native `reject`. Thresholds belong to the caller's Nu expression. No `--details` or `--min` flag is needed.

Alternative: reading all process environment once would miss caller-scoped changes while a plugin process is reused. `NU_PLUGIN_JEV_PROXY` is therefore read through `EngineInterface` per invocation, while ordinary OS and standard proxy environment settings belong to reqwest's automatic client and are captured when the plugin process starts; changing those ordinary settings requires a plugin restart.

### 5. One state versus table streaming

Implement `jev ask` and `jev annotate` as `PluginCommand`. Use `SimplePluginCommand` for the offline root and question constructors. Nushell's [public command API](https://docs.rs/nu-plugin/latest/nu_plugin/trait.PluginCommand.html) gives the evaluation commands direct control over stream collection and incremental row processing.

`ask` treats a finite input ListStream as one array state and necessarily collects it before a JSON request can be submitted. Empty pipeline input is an error; a supplied empty list is a valid array. Reject ByteStream input with guidance to decode it explicitly into a Nu value. Document single-state memory use; this change adds no arbitrary request-size cap.

`jev annotate` accepts record rows, including a single record as a one-row stream, and consumes list/stream input incrementally. Non-record rows are errors. `--state` uses public Nu cell-path behavior for existing fields and indices and never invokes a closure. `--fields <list<string>>` selects multiple literal top-level field names into a new outbound record; names containing dots remain literal rather than being interpreted as paths. The list must be nonempty and contain distinct strings. Reject malformed lists and simultaneous `--state`/`--fields` before consuming input, even with `--on-error keep|record` or `--dry-run`. A missing selected field is a per-row state error before HTTP dispatch, not an implicit null or silently skipped column. Nested selection remains available through `--state`, and more complex state construction belongs in ordinary Nu pipelines.

Successful annotation preserves the original record and adds only `answers` under a top-level literal field name (`jev` by default). Optional `--meta` adds a separate metadata record. State selection/projection does not mutate or remove source fields. Destination names must be nonempty and distinct, and existing destination fields are not overwritten, including fields excluded from the outbound state. A field collision is a row error checked before HTTP dispatch.

Dry runs use the same question parsing, state selection, context composition, and body builder. `ask` returns one body; `annotate` returns one body per valid input row in input order, including duplicates. They bypass authentication, deduplication, and HTTP.

For example, send only the judgment-relevant columns and an explicit policy while retaining identifiers and all other source fields:

```nu
let questions = open spam-policy.nuon
let policy = open moderation-policy.nuon

open messages.json
| jev annotate $questions --fields [message sender history] --context $policy --into ai
| where ai.spam.noul >= 0.98
```

Each request's state is `{input: {message: ..., sender: ..., history: ...}, context: $policy}`. Native `where` applies the caller's threshold to returned probabilities. Adding `--dry-run` to `jev annotate` previews the same request bodies without credentials or network access; a preview has no answer to filter.

The standard table contract is independent row states: each row builds its own request body, and distinct canonical bodies create independent logical evaluations. Identical final bodies can share an in-flight evaluation or cached outcome even when their original rows differ in unselected fields. Retries and cache eviction can increase HTTP attempts, so row count is not an exact request-count guarantee.

The lack of a dedicated batch endpoint does not make client-side packing impossible: a client can explicitly assemble several rows into one structured state and name questions for each row. However, [all questions in a request share that state](https://docs.typesafe.ai/concepts/state), so neighboring rows become part of each decision's context. This change introduces neither automatic packing nor a packing flag, and never rewrites questions to reference synthetic row indices. A future packing mode requires separate validation of decision quality and cross-row influence, request/context limits, partial errors, cache equivalence, and shared usage attribution. Users can still intentionally construct a joint state for `jev ask` with ordinary Nu commands; that is one-state evaluation, not transparent table batching.

Alternative: automatically mapping `ask` over lists would make the same Nu value mean both a shared state and a batch, contradicting the API.

### 6. Bounded scheduling and ordering

Use one invocation-local supervisor and a dedicated synchronous input producer. For `N = jobs`, define an admission window `W = 2N`; both bridge channels have capacity at most `N`. A row acquires an admission permit before input is read. It retains that permit while queued, awaiting or sharing a request, buffered for ordering, or waiting in the output queue. Release it when its output is yielded or cancellation discards it. Incoming read-ahead within Nushell's own protocol is outside these plugin-local counters.

```text
Nu iterator
    |
    v
admit row within W --> bounded input --> async supervisor
                                         |
                                         v
                                   at most N evaluations
                                         |
                                         v
                                   ordered outcomes
                                   or ready outcomes
                                         |
                                         v
                                   bounded output --> Nu iterator
```

The supervisor manages at most `N` unique outstanding evaluations, including their retries, with `FuturesUnordered`. Sequence numbers associate outcomes with original rows. Ordered mode advances through every row outcome, including failed rows returned by `keep` or `record`; unordered mode releases completed outcomes immediately. Duplicate waiters consume row permits even though they share one HTTP future. A completed HTTP request does not by itself release row admission credit.

The synchronous producer builds and serializes each request before passing it to the async supervisor. A reference-counted encoded body is shared by the canonical key and all HTTP retry attempts, avoiding repeated JSON serialization on Tokio workers. The supervisor retains typed questions for response validation. A full output-channel send also waits for invocation cancellation, so backpressure cannot hide an interrupt while the receiver is still alive.

This bounds queued and outstanding work by `O(jobs)` and prevents an ever-growing reorder buffer behind one slow row. Full utilization can fall while ordered output waits for an early row; that is the intended ordering/backpressure trade-off. The standard [`buffered`](https://docs.rs/futures-util/latest/futures_util/stream/trait.StreamExt.html#method.buffered) adaptor is a valid simpler implementation only if it also preserves these admission and failure guarantees.

Alternative: limiting only active HTTP futures allows completed outcomes and duplicate waiters to accumulate without bound.

### 7. Single-flight, bounded invocation cache, and metadata

Use mandatory single-flight deduplication and a bounded LRU of successful results within each annotation invocation. Check the completed cache first and refresh a hit's recency. On a miss, check the in-flight lookup: join an existing evaluation or register one before dispatching HTTP. All retry attempts belong to that shared logical evaluation. Each original row still has its own output outcome and consumes a row admission permit; there is no unbounded waiter list.

The logical key covers the canonical complete request body and service root: final state (including explicit context), questions, requested model, and any other response-affecting request parameters. Object keys are sorted recursively; arrays retain order, and scalar representations and omitted-versus-null fields are not silently conflated. Output destinations, ordering, jobs, deadlines, and retry budgets do not change request equivalence. Credentials are immutable within the invocation and are never part of a stored or logged key. Static request components can be shared rather than copied into every entry, but hash lookup must still compare canonical keys rather than treating a digest collision as equality.

On success, insert an eligible outcome into the LRU, evicting least-recently-used entries until both `max_entries` and `max_approx_bytes` are satisfied. Entry weight accounts for canonical key bytes, the serialized successful response, request identity, and a documented fixed per-entry overhead estimate. Account for the complete key even when static components share storage. This is deterministic approximate accounting, not a promise about process RSS. If one entry alone exceeds the byte limit, return it to current rows without caching it or evicting other entries to make room. The in-flight guarantee still applies to such an oversized request.

Complete the cache insertion and in-flight removal as one supervisor transition before awaiting output delivery. A failed evaluation supplies its error to current waiters and is removed without a cache entry. A later duplicate may start a fresh evaluation after a failure, eviction, or oversized-result bypass. Eviction only drops the cache's reference: it must not cancel active evaluations or invalidate shared outcomes retained by admitted rows. Successful outcomes are not guaranteed to survive for the whole invocation.

Store canonical keys and shared outcomes, not original source rows or Nu engine handles. Destroy both lookup structures when the stream completes, terminates with an error, or is dropped. No result is shared across invocations, including for `jev-latest`; invocation scope is the selected protection against stale moving aliases, so no cross-invocation TTL or global/persistent cache is introduced.

Generate a unique local `request_id` for each new logical evaluation, retained across retries, in-flight sharing, and successful cache hits. A fresh evaluation after eviction or bypass receives a new identity even for an identical body. With `--meta`, output `{model, usage, request_id}`. Reused rows preserve the same usage and identity; documentation sums reported usage once per distinct request identity, including repeated evaluations after eviction. `ask` keeps the API envelope unchanged. The identity is local provenance, not a server idempotency key or a billing receipt.

Alternative: retaining every success indefinitely would require memory proportional to all distinct requests. A process-global or disk cache additionally risks stale model aliases and retains inputs beyond the invocation's lifetime.

### 8. Cancellation, row failures, and stream ownership

Create a cancellation token per invocation. Register an engine interrupt handler and move its guard into the output stream's lifetime owner; [the guard keeps the handler registered only while it is alive](https://docs.rs/nu-plugin/latest/nu_plugin/struct.EngineInterface.html#method.register_signal_handler). Also attach engine signals to the Nu stream. Snapshot the signal state so an already-interrupted invocation does not begin work.

The stream iterator owns the output receiver and a drop guard. Dropping it cancels the invocation independently of the next attempted channel send. The supervisor also observes receiver closure while HTTP futures are pending. Cancellation stops admission, drops pending HTTP/retry futures, closes channels, wakes blocked producers/consumers, and disposes of invocation state. Neither a blocked channel send nor retry sleep can prevent teardown. A blocking input iterator already inside `next()` must be released by the supported Nu stream-drop/signal mechanism; no attempt is made to forcibly terminate a Rust thread. Verify this path with the real plugin protocol.

Call-level argument, configuration, credential, and question validation errors return before stream production. Per-row state, path, collision, and HTTP errors use `--on-error`:

- `fail` (default): cancel upon the first observed terminal error, without waiting for its ordered slot; emit one terminal Nu error value and no subsequent successful values. Previously delivered rows cannot be rolled back.
- `keep`: yield the original row without additions.
- `record`: preserve the row and add `jev_error: {kind, message, status}`; use null status for non-HTTP errors. If an existing `jev_error` prevents insertion, emit a terminal error rather than overwrite data.

Non-record rows can be returned unchanged by `keep`; `record` cannot attach a field and therefore terminates with an error. Existing upstream Nu errors remain terminal and are not converted into successful kept rows. For table dry runs, `fail` is terminal and `keep`/`record` yield their respective failed-row forms among the request bodies; this mixed output is documented. The supervisor's terminal path must wake a consumer waiting behind a slow ordered row.

Alternative: relying solely on checking `engine.signals()` between rows cannot cancel an HTTP request or backoff sleep already in progress.

### 9. Transport and deadlines

Use reusable [reqwest clients](https://docs.rs/reqwest/0.12.28/reqwest/struct.Client.html) and authenticated `POST /v1/systemone` for `ask` and `annotate`. Enable reqwest's `http2`, `system-proxy`, and `socks` features alongside the existing JSON and Rustls features. For HTTPS, negotiate HTTP/2 through ALPN when the service and route support it, with ordinary HTTP/1.1 fallback; do not require HTTP/2 or use prior-knowledge mode. Do not follow authenticated redirects automatically; the configured service root determines the destination.

The default `auto` policy uses reqwest's standard proxy discovery, including applicable process `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`, and OS proxy settings. `direct` disables all proxies. An explicit `http://` or `socks5h://` proxy URL is authoritative: it replaces automatic discovery and is not silently bypassed by global `NO_PROXY`. The `socks5h://` form resolves destination names at the proxy. If an explicit proxy is malformed, unreachable, or rejects the request, fail the invocation or evaluation as appropriate rather than silently routing directly or through another proxy. An explicit proxy is selected by `$env.config.plugins.jev.proxy` or caller-scoped `NU_PLUGIN_JEV_PROXY`, with plugin config taking precedence. A dry run validates the selected policy but never opens a proxy connection or reveals its URL.

The two proxy sources intentionally have different update lifetimes: reqwest's automatic process/OS discovery is retained for `auto` and refreshed by restarting the plugin, while Jev-specific policy is fetched from the caller for every new command invocation. Do not reinterpret ordinary Nu proxy-variable edits as dynamic updates to an already-created automatic client. Document both lifetimes, the explicit-proxy `NO_PROXY` exception, and the absence of silent fallback without presenting unfinished controls as available in the current binary.

Cache clients by effective proxy policy, retaining the default automatic client and bounding the number of additional cached clients. Every client keeps its connection pool across rows and compatible invocations; never build one per row. An in-flight request may retain a client after its cache entry is evicted. Do not put proxy URLs or credentials into tracing fields or raw error messages. This client cache is distinct from the invocation-local successful-result LRU; response results do not persist across invocations.

`timeout` is a total deadline for one logical evaluation, including retry delays and attempts, starting when an evaluation is dispatched. It is not a deadline for an entire table. Retry `429`, `502`, `503`, `504`, and documented `529` up to `retries` additional attempts. Follow the [official SDK's retry-header precedence](https://github.com/typesafe-ai/typesafe-sdk-js/blob/main/src/retry.ts): prefer a finite, nonnegative, representable `retry-after-ms` delay over a valid `Retry-After` delta-seconds or HTTP-date delay; an invalid millisecond header falls back to `Retry-After`. With neither usable, start exponential backoff at 500 ms, double its base for each retry up to 5 seconds, then subtract 0–25% jitter. This makes the first unguided wait 375–500 ms. Do not copy the SDK's separate 60-second retry-guidance cutoff: if a valid advised delay exceeds the remaining evaluation deadline, return a timeout rather than retry early or substitute a shorter delay. Waiting is asynchronous and cancellable. Keep the existing status allowlist, default retry count, and total-deadline meaning; do not broaden retries to ambiguous transport failures or add per-attempt deadlines.

Do not retry other 4xx responses, response decoding/contract failures, timeout, or ambiguous transport failures automatically. Disable reqwest's separate automatic protocol-NACK retries with `reqwest::retry::never()` so `retries = 0` means exactly one wire attempt and the configured attempt budget remains authoritative. Keep error kinds stable (`validation`, `state`, `field_collision`, `http`, `transport`, `timeout`, `response`) and preserve HTTP status where available. Read the [server request ID header](https://github.com/typesafe-ai/typesafe-sdk-js/blob/main/src/api-promise.ts) from each HTTP response, but accept it for per-attempt tracing only when it is nonempty, does not contain the caller's API key, and is at most 128 ASCII bytes drawn from letters, digits, `.`, `_`, `:`, and `-`; otherwise omit it. Emit the accepted value at debug level with the attempt number under the existing local logical-request span, including failed attempts. The server ID is not a local `request_id`, idempotency key, or billing receipt, and does not enter the API envelope, `--meta`, or `jev_error`. Redact credentials, including proxy URL userinfo, from diagnostics and avoid dumping request headers or unbounded/raw error bodies.

Alternative: unlimited retries or per-attempt timeout alone can make a cancelled or overloaded pipeline remain active indefinitely. Sequential `block_on` per table row would defeat concurrency; `jev ask` can block its synchronous command thread on one cancellable future.

### 10. Repository-owned usage skill

Keep `skills/jev-nushell/SKILL.md` in the repository rather than among environment-owned OpenSpec workflow skills. It gives agents concise guidance for using the implemented plugin: structured state, multi-question requests, row annotation, dry runs, native Nu composition, and data-disclosure boundaries. The README and actual command surface remain the source of truth; the skill must not claim pending OpenSpec work is implemented. Update the skill alongside README/help whenever user-visible behavior changes, including completion of the proxy transport extension. The repository's `AGENTS.md` records this maintenance rule for later changes.

## Risks / Trade-offs

- Cache eviction or oversized entries can cause repeated evaluations and additional reported usage --> document bounded reuse rather than an invocation-wide once-only guarantee, and distinguish fresh request identities from cache hits.
- Approximate cache accounting is not an absolute process-memory cap --> bound entries and key/result bytes separately, document the accounting estimate, and retain active rows/outcomes only within the scheduling window.
- One array state or one record can be very large --> document single-state materialization and per-row memory costs; keep large-table examples on `annotate` rather than implying a batch endpoint.
- Packing independent rows into a shared state can change decision quality and complicate partial failures, deduplication, and usage attribution --> preserve isolated row-state semantics and evaluate packing separately rather than selecting a default batch size from another integration.
- Ordered output can stall behind an early request --> bound the admission window and offer `--unordered` explicitly.
- Dropping a client future cannot guarantee that the server stopped or that no usage was incurred --> promise prompt local teardown and bounded read-ahead, not exactly ten remote requests after `first 10`.
- Schema and prose can change independently --> keep representative contract fixtures, record source/version assumptions, and test raw commands separately from constructor limits.
- Nu test support alone does not reproduce every protocol lifetime --> add real Nu subprocess coverage for downstream truncation, interrupts, and blocked producer/consumer teardown.
- Reused successful usage does not report any unseen server work during failed attempts --> present request identities as provenance for reported usage, not an invoice total.
- Ambient proxy settings captured by a long-lived reqwest client may become stale during a Nu session --> document that standard proxy environment and OS setting changes require a plugin restart, while Jev-specific `proxy`/`NU_PLUGIN_JEV_PROXY` are resolved per invocation.
- A proxy URL may contain credentials and transport errors may echo it --> redact the entire configured URL and userinfo from diagnostics and returned errors, and test the failure paths.

## Migration Plan

No existing data or separately released plugin is migrated. Within this open change, the interim environment names and implicit TOML paths are replaced without aliases; users of the working binary must rename those variables and move those files, or select an old-named local file explicitly with `--config`. Implement the layers in dependency order, validate each stage with formatting, clippy, and tests, then register the binary in an isolated Nu `0.116.x` session. Verify offline examples and mock-server pipelines before documenting an optional live smoke request using the user's own environment key.

Distribution documentation uses `cargo install --path . --locked`, `plugin add ~/.cargo/bin/nu_plugin_jev`, and `plugin use jev`, followed by command help. No task publishes a crate, creates a remote repository, or sends private input to the live API. Rollback is removal of the registration/binary; no external cache or persisted plugin state needs migration.

## License

The project is licensed under MIT, recorded in Cargo metadata and the root `LICENSE` file. Dependencies retain their own licenses and distribution obligations.
