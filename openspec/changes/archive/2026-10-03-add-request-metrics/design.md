# Design

## Context

See [proposal.md](proposal.md) for the motivation. `jev ask` and `jev annotate` already construct a typed `SystemOneRequest`; the live client serializes it once into `PreparedRequest.body`, while dry runs currently project the body into Nu values. `jev models` uses the same client policy for a bodyless GET and returns a bare model list. Its discovery change must be synced or archived before this change's MODIFIED model-list delta is archived. The client can retry one logical operation and validates decoded responses. Annotation can share a completed or in-flight evaluation across rows; its optional metadata currently carries `model`, `usage`, and `request_id`.

## Goals / Non-Goals

**Goals:**

- Measure dry-run request bodies without credentials, HTTP calls, or token-estimation support.
- Expose the selected service root and returned response provenance in an always-present live metadata record (`meta` on `ask` and `models`, `jev_meta` on `annotate`); keep per-logical-operation body sizes, total and final-successful-attempt elapsed times, attempt count, and HTTP version in opt-in `metrics` or `jev_metrics`, respectively.
- Put the same measurements and service-root provenance in successful evaluation and model-list tracing events, independently of the `--metrics` output flag.
- Remove annotation's optional `--meta` switch and use fixed `jev_meta` and, when `--metrics` is selected, `jev_metrics` destinations; keep `meta` and `metrics` fixed on `ask` and `models`.

**Non-Goals:**

- Measure on-the-wire bytes, headers, compression, proxy/TLS framing, server-only latency, or token usage independently of the API's returned `usage`.
- Infer whether the final attempt established a new connection or reused one.
- Report cumulative bytes from failed attempts, failed evaluations, or rows served without a new HTTP evaluation.
- Add a new command, endpoint, persistent telemetry store, or tokenizer feature dependency.

## Decisions

### Return request previews with a separate measured wrapper

Successful `--dry-run` output becomes `{request, request_bytes}`. `request` is the exact JSON-shaped body; inserting byte metadata inside it would make the preview look like a different API request. A feature-independent helper serializes the typed request with the same compact `serde_json` settings as live HTTP and counts UTF-8 bytes. Prefer a counting `Write` sink, verified against `PreparedRequest.body.len()` and a captured mock-service body, to avoid an extra full-size allocation per preview. If exactness cannot be preserved, reuse the live encoder instead. The helper maps serialization or Nu-integer overflow failures to content-free labeled errors.

The existing lazy annotation preview stream remains in place. It yields one wrapper per valid row, including duplicates; ordinary invalid rows retain the existing `--on-error` policy and never receive a fabricated size. Output-destination collisions instead terminate the invocation, including in preview mode. No runtime or HTTP worker is started for previews. This output-shape change is intentional and documented as a migration from `get state` to `get request.state`.

### Capture a typed measurement at the HTTP client boundary

The client returns a typed successful response plus an internal measurement record for every successful live evaluation or model lookup, regardless of `--metrics`, so tracing and optional Nu output use the same measurement. Retain the invocation's validated, normalized `base_url` as provenance independently of the HTTP measurement fields. It is the selected service root before appending `/v1/systemone` or `/v1/models`, not a proxy URL, final redirected URL, or a value claimed to come from the TypeSafe response body. Redirects remain disabled. `request_bytes` comes from the actual encoded body length for **one** HTTP attempt; it is exactly zero for the bodyless models GET. `attempts` counts explicit send attempts. Capture one monotonic start immediately before each explicit HTTP send and retain the first and final successful attempt starts. After response decoding and contract validation, use one completion instant to compute `elapsed` from the first start and `attempt_elapsed` from the final start. The first duration includes retries and waits; the second excludes earlier attempts and inter-attempt delays. Both exclude pre-dispatch preparation and table queue time, and both include final response body consumption and validation. On a single-attempt success, both durations are equal because they use the same start and end instants.

Read the final successful response into a byte buffer once, take its length, then decode the existing typed System One or model-list response from that buffer. Retry/error bodies are neither read for accounting nor included in `response_bytes`. This avoids changing retry behavior or claiming complete transfer measurements. Retain the existing timeout and cancellation envelope around response reads. If decoding work could block a Tokio worker for large bodies, move that work to the project's blocking executor without changing the measurement boundary.

Capture `response.version()` from that final successful HTTP response before consuming its body and expose a stable HTTP-version string such as `HTTP/1.1` or `HTTP/2`. A version reported by an earlier retry response, a proxy tunnel handshake, or an assumption based on client features must not replace it. Reqwest does not provide a reliable per-response connection-new/reused flag through this path, so no such field is included or inferred from remote address or timing.

The alternative of summing every attempted request body and every response body was rejected: the client currently discards retry responses, transport failures cannot prove which bytes reached the server, and reading error bodies solely for accounting would alter resource use. The fields describe application payloads, not transport counters.

### Trace measured success once per logical HTTP operation

Extend the existing plugin-owned `evaluation completed` and `model listing completed` events at `info` with numeric `request_bytes`, `response_bytes`, `elapsed_ns`, `attempt_elapsed_ns`, and `attempts`, plus string `http_version` and `base_url`. The two nanosecond fields are the total HTTP-boundary and final-successful-attempt measurements respectively; the existing `duration_ms` remains the broader operation-span duration and must not be relabeled as either HTTP measurement. Use the same internal success provenance and measurement records that power returned `meta`/`metrics` on `ask` and `models`, or `jev_meta`/`jev_metrics` on `annotate`, with checked conversions to tracing's integer fields. Preserve the `request_id` in each enclosing span; evaluation completion keeps its model and usage fields, while listing completion keeps its model count and does not fabricate token usage.

Emit one completion event per actual successful HTTP operation, whether or not `--metrics` was requested. Duplicate annotation rows joining an in-flight request or reusing a completed cache entry do not generate another completion event. A failed, cancelled, dry-run, or token-estimation operation has no successful measurement event; existing failure and attempt diagnostics remain, without fabricated success fields. Apart from the explicitly requested, validated `base_url`, no request state, question body, full endpoint URL, proxy URL, credentials, or response content is added. The planned NUON formatter can represent the numeric fields and `base_url` string without changing this measurement contract.

### Keep response provenance separate from opt-in HTTP measurements

On every validated live success, `jev ask` returns `{answers, meta: {base_url, model, usage}}`; `jev annotate` appends answers and fixed `jev_meta: {base_url, model, usage}` to each source row; and `jev models` returns `{models: <ordinary list>, meta: {base_url}}`, including for an empty list. `base_url` is plugin-selected provenance, while `model` and `usage` are validated System One response fields. No individual model row gains `base_url`. All metadata destinations are fixed. The old annotation `--meta <name>` flag is removed without a replacement destination override. Native model-table processing now uses `jev models | get models`; this default-output change is intentional.

`--metrics` adds a separate `metrics` sibling field on `ask` and `models`, or `jev_metrics` on `annotate`, only for successful live operations. It contains `{request_bytes, response_bytes, elapsed, attempt_elapsed, attempts, http_version}`; annotation additionally includes the local `request_id` so shared or cached rows can be counted once per logical evaluation. Metrics destinations are fixed. Neither `base_url`, `model`, nor `usage` is duplicated inside a metrics record. Both durations are Nu durations; byte counts and attempts are checked Nu integers. The completed annotation cache entry retains response provenance, measurement, and request identity together; duplicate rows reuse all three. Without `--metrics`, token usage is still visible in `jev_meta` on annotation rows, but callers need the opted-in `jev_metrics.request_id` to de-duplicate usage across shared rows.

Validate the answer destination against fixed `jev_meta`, enabled `jev_metrics`, and `jev_error` when `--on-error record` is selected before consuming annotation input. `--metrics` is live-only and is rejected with `--dry-run` or `--estimate-tokens` before input consumption. Once an annotation row is read, check all enabled destinations against its complete source record before request construction or HTTP dispatch, even when projection excludes those fields. A collision is terminal independently of `--on-error`: stop admission, cancel outstanding local work, and deliver the error without waiting for an earlier ordered result. Already delivered rows cannot be rolled back, and a streaming input cannot be pre-scanned without losing bounded processing.

The client-side shared response/cache entry retains its measurement alongside the typed answer and `request_id`. Joining an in-flight evaluation or reusing a completed cache entry therefore produces the same `jev_meta`, optional `jev_metrics`, and identity. No new HTTP request is implied by each annotated row. Errors are never cached as success and ordinary `keep`/`record` error rows receive no success fields.

On evaluation commands, `--metrics` is accepted only for live operations; reject its combination with `--dry-run` or the separately planned `--estimate-tokens` before sending HTTP and, for annotation, before consuming input. `jev models` accepts `--metrics` but no output-destination override or offline flag. Neither preview nor offline estimation pretends to have an API response or completed HTTP measurements. The byte helper and live result formatting must compile without the token-estimation feature.

## Risks / Trade-offs

- **Dry-run field paths change** → Update tests, command help, README, usage skill, and changelog with `get request` migration examples.
- **Metrics are mistaken for wire traffic or billing** → Document JSON-body scope, retry exclusions, and `attempts`; distinguish from API `usage` tokens.
- **Two durations are confused** → Name the final-attempt field explicitly, capture both from one completion instant, and document that only `elapsed` includes retry waits.
- **Tracing fields are mistaken for one event per row** → Emit only on actual evaluation completion and explain that cache/in-flight sharing reuses one request identity.
- **A request fails mid-send** → Expose no success metrics rather than claiming the encoded body was fully transmitted.
- **A response is malformed or fails answer validation** → Return the existing error and do not expose or cache a partial metrics record.
- **Large response buffering and decoding use memory/CPU** → Reuse one response buffer, preserve cancellation, and keep potentially blocking decode work off Tokio workers.
- **Duplicate rows appear to multiply network volume** → Preserve one `request_id` and identical metrics; instruct users to aggregate once per distinct identity.
- **An answer, `jev_meta`, or enabled `jev_metrics` destination collides with source data** → Terminate at the first conflicting row before HTTP, even with `--on-error keep|record`; the fixed plugin-prefixed destinations and configurable answer destination avoid silently passing through an unannotated row.
- **Output migrations break existing paths** → Document `ask` moving `model`/`usage` under `meta`, annotation's old `--meta <name>` being removed without a destination override, and `models` changing from a bare list to `{models, meta}`.
- **Service-root provenance reveals internal host/path information** → Report only the validated caller-selected root in always-present `meta` on `ask`/`models` or `jev_meta` on `annotate`, and successful `info` tracing, never proxy credentials or a full request URL; document that roots should contain no secrets.

## Migration Plan

First add the shared byte-measurement helper and HTTP success measurement for POST and GET, then enrich both tracing completion events and wire the always-present `meta` on `ask`/`models` or `jev_meta` on `annotate`, plus optional `metrics` or `jev_metrics`, respectively. Remove annotation's `--meta <name>` without a replacement destination override; keep `ask` and `models` metadata and metrics destinations fixed as `meta` and `metrics`. Update mock HTTP, retry, duplicate, collision, cancellation, tracing, and offline preview tests; update README, command help, usage skill, and changelog with the exact result shapes and metric scope. Run the repository's formatting, Clippy, nextest, and strict OpenSpec validation gates when implementing. Existing dry-run consumers move body paths through `request`; existing `ask` users move `model`/`usage` paths through `meta`, and model-list users select `get models` for native table processing. Successful `info` diagnostics gain measurement and root fields. A rollback can restore the old output shapes and flag with corresponding documentation and tests; no stored-data migration is required.
