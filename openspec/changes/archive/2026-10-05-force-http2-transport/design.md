# Design

## Context

`JevClient::for_policy` is the single reqwest builder for the startup automatic
proxy client and bounded alternate proxy-client cache. Reqwest's `http2` feature
and an `h2` test dependency are already present. Most local mocks currently
speak HTTP/1.1; one transport test already runs an HTTP/2 TLS server.

## Goals / Non-Goals

**Goals:** Apply the same transport policy to every pooled client: HTTP/2-only
in default builds and existing negotiation with the Cargo feature disabled.
Preserve retry and proxy contracts, and test real wire behavior.

**Non-goals:** Add a runtime flag, environment variable, TOML key, or new
request/response field. Change the remote TypeSafe API contract.

## Decisions

- Add a default-enabled `http2-prior-knowledge` Cargo feature and call
  `ClientBuilder::http2_prior_knowledge()` in the shared builder only when it
  is enabled. This covers `ask`, `annotate`, and `models` in every proxy-policy
  client without a runtime opt-out or per-request version override. With the
  feature disabled, retain reqwest's previous HTTP version negotiation.
- Use a ready-made local HTTP/2 server for successful network tests (h2c for
  `http://` endpoints, TLS/H2 where TLS matters), including streamed bodies
  and retry responses. Keep an HTTP/1.1-only negative compatibility test in
  the default build and a positive compatibility test without the feature.
  Keep transport-free unit tests free of listeners and test-only policy bypasses.
  Keep the existing low-level TLS/ALPN and REFUSED_STREAM fixtures only for
  protocol conditions a normal application handler cannot produce.
- In default builds, treat an incompatible route as a transport failure. The
  existing policy does not retry ambiguous transport failures; it still
  retries only the specified HTTP statuses after a response is received.
  Successful `http_version` reports the actual protocol in either build.
- Document the default-build compatibility constraint and compile-time
  alternative in README. The short Nushell usage skill should not carry
  transport-build details.

## Risks / Trade-offs

- HTTP/1.1-only custom endpoints and some proxy routes will stop working in
  default builds. -> Document the build choice and verify both modes.
- HTTP/2 can multiplex requests on one connection, changing assumptions in
  test fixtures that count one accepted socket per request. -> Count streams
  and model concurrency at stream level, not socket level.
- Raw HTTP/1.1 malformed-wire fixtures cannot stand in for HTTP/2 responses.
  -> Preserve the intent (bounded body, invalid JSON, retry timing) with
  HTTP/2 frames and headers through a maintained server implementation.

## Migration Plan

Apply the Cargo feature and builder change, then convert affected mocks in the
same commit. Run full Rust checks in default mode, focused feature-off checks,
and real Nushell integration tests. HTTP/1.1-only routes require either a
feature-off build or an HTTP/2-capable route.
