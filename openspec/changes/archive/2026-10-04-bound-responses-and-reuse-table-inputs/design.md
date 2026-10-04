# Design

## Context

See [proposal.md](proposal.md) for motivation and the [transport delta](specs/jev-http-transport/spec.md) for the new external contract. `jev annotate` already uses a dedicated synchronous producer, bounded admission, and a Tokio HTTP supervisor. The producer prepares each JSON body off Tokio workers; a typed request is retained for answer validation. The same serialized body is shared with retries and the deduplication key.

The current `reqwest::Response::bytes()` call collects a successful body before decoding. The row builder converts the same Nu context and clones the complete questions map for each row. Neither issue requires changing the Nu command surface or the existing row-order/backpressure design.

## Goals / Non-Goals

**Goals:**

- Bound successful body collection for both API endpoints without changing retry, timeout, cancellation, or redaction behavior.
- Reduce repeated local row-preparation work without changing request bytes, dry-run output, cache keys, or per-row error behavior.
- Report measured preparation cost separately from network/API latency.

**Non-goals:**

- No new flags, configuration keys, request-body size limit, persistent cache, or byte-level bound on all admitted Nu rows.
- No assumption that local preparation savings predict end-to-end API throughput.

## Decisions

### Read successful bodies incrementally

Use `Response::content_length()` as an early rejection check and `Response::chunk()` to enforce the 16 MiB limit on observed bytes even when no trustworthy length is declared. `chunk()` is available with the current reqwest features, so no streaming feature is needed. Return a redacted `response` error and drop the response as soon as the bound is exceeded; do not retry a successful-but-oversized response. Preserve a single-chunk `Bytes` without copying it; aggregate only multi-chunk responses before typed JSON decoding. The existing total deadline and cancellation encompass body reading.

Alternative considered: cap only `Content-Length` or inspect `body.len()` after `bytes()`. Neither bounds chunked or misleading responses during collection. A configurable cap adds public settings and testing surface without a demonstrated need; 16 MiB is a deliberately generous fixed limit for this change.

### Share table questions and preconvert static context

Keep `SystemOneRequest` typed and serializable, but hold its immutable question map in an `Arc`. Enable Serde's `rc` support so the wire format remains the same while each row clones only the `Arc`, not the full map. Convert valid static `--context` from Nu to JSON once when constructing the table row builder; clone that JSON value into each row's state because each independent request must still contain the context. If static context conversion fails, retain the existing per-row conversion path so input-state conversion errors still take precedence and `--on-error` behaves as before. `jev ask` keeps its single-request construction path.

Alternative considered: pre-serialize and splice static JSON fragments into request bodies. That would complicate exact-body validation, dry-run, cache equivalence, and error handling for modest extra savings. Sharing ownership and conversion is narrower.

### Measure the local effect

Use a repeatable, ignored local benchmark comparing legacy per-row construction with shared-data construction on identical small and large question/context fixtures. Measure both request construction and construction plus JSON body preparation over 1000 rows, reporting medians across repeated runs. Verify that both paths produce identical body bytes before timing. Keep network latency out of this comparison.

Paired local result from `cargo test --all-features --bin nu_plugin_jev commands::annotate::tests::benchmark_row_preparation --locked -- --exact --ignored --nocapture` in the debug test profile, with five repeated 1000-row runs per fixture (milliseconds, medians):

| Fixture | Legacy construction | Reused construction | Legacy prepared body | Reused prepared body |
| --- | ---: | ---: | ---: | ---: |
| 3 questions, 1 KiB context | 8.131 | 2.615 | 60.535 | 53.797 |
| 24 questions, 8 KiB context | 41.926 | 3.250 | 384.999 | 346.714 |

The prepared-body reduction is about 11% for the small fixture and 10% for the large one. The benchmark verifies identical prepared bytes before timing; it is not an end-to-end throughput measurement.

## Risks / Trade-offs

- [A very large individual HTTP chunk may already be allocated by the transport before the application inspects it] → Reject immediately on receipt; the accumulated body remains capped, but do not claim a strict process-RSS ceiling.
- [16 MiB rejects previously accepted oversized successes] → Document the compatibility change and keep the error classified as `response`, without partial answers or payload logging.
- [Static conversion can shift which row error is reported first] → Fall back to the old per-row path for invalid context and test mixed invalid input/context cases.
- [Sharing a typed question map can accidentally change serialization or cache keys] → Compare exact prepared bytes and dry-run output; retain existing streaming and integration tests.
- [Microbenchmarks are noisy] → Use paired runs and medians, report workload and build mode, and do not claim an API throughput improvement.

## Migration Plan

No data migration is needed. Release notes, README, and the repository usage skill will state the response limit. If valid real-world responses above 16 MiB are required, revisit the contract through a later OpenSpec change rather than silently lifting the bound.
