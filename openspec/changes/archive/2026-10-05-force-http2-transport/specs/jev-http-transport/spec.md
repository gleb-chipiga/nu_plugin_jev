# Spec Delta

## ADDED Requirements

### Requirement: Compatibility build protocol negotiation

Without the `http2-prior-knowledge` Cargo feature, live requests SHALL retain
the former protocol negotiation: HTTP/2 through HTTPS ALPN when available,
HTTP/1.1 otherwise, and HTTP/1.1 for cleartext HTTP endpoints.

#### Scenario: HTTPS and cleartext compatibility

- **WHEN** the plugin is built without `http2-prior-knowledge`
- **THEN** HTTPS requests negotiate HTTP/2 when offered and fall back to HTTP/1.1 otherwise
- **AND** HTTP requests retain the previous HTTP/1.1 behavior

### Requirement: No runtime transport override

The selected build's HTTP version policy SHALL NOT be changed by command
flags, environment values, or TOML settings.

#### Scenario: Runtime settings do not change protocol policy

- **WHEN** a default build receives command flags, environment values, or TOML settings
- **THEN** none of them disables HTTP/2 prior knowledge for a live request

### Requirement: Transport selection preserves application contracts

HTTP version policy SHALL NOT alter JSON request bodies, bearer
authentication, response contracts, or the application retry budget.

#### Scenario: Same application contract across builds

- **WHEN** either Cargo feature mode sends a live evaluation or model-list request
- **THEN** its body, authentication, response validation, and retry budget follow the same contract

## MODIFIED Requirements

### Requirement: HTTP version negotiation

With the default-enabled `http2-prior-knowledge` Cargo feature, every live
`jev ask`, `jev annotate`, and `jev models` request SHALL use HTTP/2 over HTTP
or HTTPS without HTTP/1.1 fallback. A route unable to serve HTTP/2 SHALL fail
as a transport error.

#### Scenario: HTTP/2-capable endpoint

- **WHEN** an endpoint and selected route support HTTP/2
- **THEN** evaluation and model-list requests use HTTP/2 and return their ordinary typed results

#### Scenario: HTTP/1.1-only endpoint

- **WHEN** a configured service supports only HTTP/1.1
- **THEN** the live request fails as a transport error without retrying through HTTP/1.1

#### Scenario: Cleartext HTTP/2 endpoint

- **WHEN** a configured `http://` service supports prior-knowledge HTTP/2
- **THEN** a live request succeeds without an HTTP/1.1 upgrade or fallback

### Requirement: Bounded successful response bodies

Each successful System One or model-list response SHALL be limited to 16 MiB
(16,777,216 body bytes), independently of retries and rows. An oversized
declared `Content-Length` SHALL be rejected before body reading; a response
without a usable length SHALL be limited while reading. Oversize SHALL produce
a nonretryable response error without JSON decoding, logging body contents, or
returning partial data.

#### Scenario: Declared oversized body

- **WHEN** a successful response declares a body larger than 16 MiB
- **THEN** the plugin rejects it without reading or decoding the body

#### Scenario: Chunked oversized body

- **WHEN** a successful HTTP/2 response arrives in DATA chunks without a usable length declaration and exceeds 16 MiB while being read
- **THEN** the plugin stops reading and returns a response error without retrying it

#### Scenario: Body at the limit

- **WHEN** a successful body has exactly 16 MiB of data and satisfies the endpoint's JSON contract
- **THEN** the size limit alone does not reject it

### Requirement: Measure successful HTTP attempts and durations

`attempts` SHALL count explicit attempts and `http_version` SHALL name the final successful response protocol. `elapsed` SHALL run from before the first attempt through decoding and contract validation, including retries and waits but excluding preparation and table queueing. `attempt_elapsed` SHALL use the same completion instant, starting before the final attempt. Both SHALL be nonnegative Nu durations, equal on one-attempt success, and SHALL NOT claim to measure server processing alone.

#### Scenario: One successful attempt

- **WHEN** one HTTP attempt receives a valid JSON response
- **THEN** `attempts` is one, `request_bytes` equals the submitted JSON body's length, and `response_bytes` equals the final response body's consumed byte length
- **AND** `elapsed` and `attempt_elapsed` are equal and include the attempt and response validation

#### Scenario: Retry before success

- **WHEN** a retryable response is followed by a valid response after a delay
- **THEN** `attempts` counts both explicit attempts and `elapsed` includes the retry wait
- **AND** `attempt_elapsed` excludes the earlier attempt and retry wait while including final response decoding and validation
- **AND** `request_bytes` remains one body's length and `response_bytes` counts only the final successful response body

#### Scenario: Final response protocol version

- **WHEN** a valid response arrives after any retryable responses
- **THEN** `http_version` names the final successful response protocol (HTTP/2 in default builds)
- **AND** it does not claim whether a new connection was established for the attempt
