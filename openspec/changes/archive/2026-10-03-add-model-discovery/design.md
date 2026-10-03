# Design

## Context

See [proposal.md](proposal.md) for motivation and the [model-discovery delta](specs/jev-model-discovery/spec.md) for the command contract. The current plugin registers six commands and deliberately has no `jev models`. Its reusable `JevClientPool` already handles automatic, direct, and explicit proxy policies, while per-invocation configuration and credentials are resolved before async work. The HTTP client currently implements only `POST /v1/systemone`. TypeSafe's public OpenAPI 0.2.0 defines an authenticated `GET /v1/models` returning `ModelMetadataList { models: ModelMetadata[] }`; each entry has three required string fields.

## Goals / Non-Goals

**Goals:**

- Make model discovery a single, cancellable live lookup that is easy to compose with native Nu table commands.
- Reuse existing authenticated transport and caller-scoped settings without making an evaluation model a prerequisite for discovering models.
- Preserve the small command surface and avoid processing accidental pipeline input.

**Non-Goals:**

- Cache model lists, persist metadata, auto-select a default, or validate every evaluation model with a preliminary GET.
- Add model-specific filter/sort commands, a `--raw` response-envelope mode, or a generic API-request command.
- Infer model capabilities, price, alias resolution, or release-date semantics not present in the endpoint response.

## Decisions

### Add a narrow configuration scope

Add a model-list scope to the existing caller-side configuration capture. It reads the applicable `--base-url`, `--timeout`, and `--config` flags; corresponding environment, Nu plugin-config, and TOML settings; retry/proxy settings; and the caller API key. Resolve those into a small transport-only configuration type, reusing validation helpers shared with evaluation configuration. Do not make the existing evaluation `model` optional merely to run discovery, and do not validate unused `model`, `jobs`, or cache values for this command. File syntax, unknown keys, and the implicit local-file prohibition on `base_url`/`proxy` remain enforced. TOML files are selected and read afresh for each `jev models` invocation, as they are for the existing live commands; a reused plugin process does not retain their parsed contents.

### Keep the command source-like and non-materializing

Implement `jev models` as `PluginCommand`, not `SimplePluginCommand`, so `PipelineData::ListStream` can be rejected before collection. The command takes no positional state or questions and returns one Nu list of ordinary records. Use `--base-url`, `--timeout`, and `--config` as long-only flags, matching existing transport/config behavior. The synchronous plugin entrypoint resolves files/credentials, checks signals, selects a pooled client, and bridges one async operation through the existing explicit Tokio runtime. Network I/O and retry waits stay async; cancellation closes the outstanding request promptly.

### Reuse the HTTP pool and validate a typed envelope

Add a `models` client method using the existing pooled `reqwest::Client`; do not build a client per lookup. Resolve `/v1/models` below the same validated service root as `/v1/systemone`, send an authenticated bodyless GET, and keep redirects and reqwest's independent hidden retries disabled. Share the bounded status-retry, retry-guidance, deadline, cancellation, and error-classification machinery with POST without changing POST behavior. One command means one logical GET; configured retries can produce additional wire attempts.

Deserialize into concrete `ModelMetadataList` and `ModelMetadata` types. Reject missing or wrong-typed required fields, but tolerate unknown extra JSON fields for forward compatibility. Keep `release_date` as the returned string rather than parsing it into a Nu date: the published schema specifies a string, and date parsing would introduce an unnecessary compatibility gate. Build Nu records directly from typed entries, preserving server order. An empty list remains an empty list. The HTTP connection pool may persist across calls, but response data is never cached by the plugin. The later `add-request-metrics` change wraps this bare list as `{models, meta: {base_url}}` on every successful lookup and adds separate optional `metrics`; this core listing change does not implement that wrapper.

### Keep discovery separate from evaluation

Register the new command and update root/help documentation, but do not modify the `jev ask` or `jev annotate` request paths to consult it. The list is informational and can change independently of a user's chosen model. Native `where`, `select`, and `sort-by` operate on the returned records; no additional Jev command or client-side filter is needed.

## Risks / Trade-offs

- **Discovery blocked by unrelated bad settings** -> Resolve only transport settings and credentials; test that an invalid selected evaluation model or table jobs value does not block `jev models`.
- **Authenticated GET sent to an untrusted endpoint** -> Preserve the existing implicit-local-file `base_url`/`proxy` restriction, explicit selection rule, URL validation, no-redirect policy, and redacted errors.
- **Retry behavior diverges from evaluations** -> Share policy code and test status, `Retry-After`, deadline, cancellation, and no-hidden-retry behavior for GET and POST.
- **Release-date text varies** -> Preserve it as a string and test both documented date-like text and other service-supplied strings.
- **Accidental input is materialized or ignored** -> Reject nonempty `PipelineData` before iterating it, with a real-Nu stream test.
- **Users mistake a listing for a stable allowlist** -> Do not cache or enforce it in evaluation; document that aliases and availability can change.

## Migration Plan

No existing command changes shape. Add typed API types and shared transport support, then the new command and scoped configuration, followed by mock-HTTP, plugin, and real-Nu tests. Update README, root/help text, changelog, and `skills/jev-nushell/SKILL.md` in the same implementation. Rollback removes the command and its client path without altering stored user data or the evaluation protocol.
