# jev-http-transport Specification

## Purpose

Define reliable authenticated communication with TypeSafe's System One endpoint, including response contracts, bounded retries, deadlines, and actionable errors.

## Requirements

### Requirement: Configured endpoints and authentication

Evaluations SHALL use `POST /v1/systemone` under the configured absolute HTTP(S) service root. Evaluation bodies SHALL contain JSON `state`, `model`, and `questions`. Live requests SHALL use `Authorization: Bearer <caller API key>`. A trailing slash on the service root SHALL NOT alter endpoint resolution. Service roots containing credentials, a query, or a fragment SHALL be rejected. Authenticated requests SHALL NOT automatically follow redirects to a different destination.

#### Scenario: Local service root

- **WHEN** a local HTTP mock root with a trailing slash is configured
- **THEN** evaluation requests reach its `/v1/systemone` endpoint with the expected JSON body and bearer header

#### Scenario: Redirected authenticated request

- **WHEN** the service responds with an HTTP redirect
- **THEN** the plugin does not forward the authenticated request automatically to the redirect target

### Requirement: HTTP version negotiation

HTTPS evaluations SHALL offer HTTP/2 through ALPN when supported by the endpoint and selected route. They SHALL fall back to HTTP/1.1 when HTTP/2 is unavailable, without requiring HTTP/2 prior knowledge. HTTP version choice SHALL NOT change the request body, authentication, response contract, or application retry budget.

#### Scenario: HTTP/2-capable endpoint

- **WHEN** the HTTPS endpoint and route negotiate HTTP/2
- **THEN** the evaluation uses HTTP/2 and returns the ordinary typed response

#### Scenario: HTTP/1.1-only endpoint

- **WHEN** a local service supports only HTTP/1.1
- **THEN** the evaluation succeeds with the same request and response contract

### Requirement: Proxy routing without silent fallback

In `auto` mode, requests SHALL use reqwest's process/OS proxy discovery, including applicable `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY`; `direct` SHALL bypass proxies. An explicit `http://` or `socks5h://` proxy SHALL override discovery and global `NO_PROXY`; `socks5h://` SHALL resolve destination hostnames through the proxy. Explicit proxy failure SHALL return an error, never fall back to a system proxy or direct connection.

#### Scenario: Automatic proxy bypass

- **WHEN** `auto` mode is selected and ordinary proxy settings exclude the destination through `NO_PROXY`
- **THEN** the request follows standard automatic bypass behavior

#### Scenario: Explicit proxy overrides ordinary bypass

- **WHEN** an explicit Jev proxy is selected while global `NO_PROXY` matches the destination
- **THEN** the request uses the explicit proxy rather than silently connecting directly

#### Scenario: Explicit proxy is unavailable

- **WHEN** the selected explicit proxy cannot be reached
- **THEN** the evaluation fails with a redacted transport error and does not connect directly

### Requirement: Reusable clients follow the effective proxy policy

Proxy-policy changes between invocations SHALL select clients by effective policy. A client SHALL NOT be built per table row, and the retained client cache SHALL be bounded.

#### Scenario: Reuse within a table

- **WHEN** multiple rows in one invocation share the same effective proxy policy
- **THEN** they reuse a pooled client rather than constructing a client for each row

### Requirement: Typed response contract validation

Responses SHALL contain model, named answers, and usage. Names and variants SHALL match questions. Noul SHALL retain `noul`; Choice SHALL retain `choice`, `confidence`, and `probabilities`; Score SHALL retain `score`, `confidence`, `legend`, and `probabilities`. The plugin SHALL validate required fields, representable numbers, probability/confidence ranges, returned option/level membership, and nonnegative token counts. Invalid responses SHALL fail rather than fabricate decisions.

#### Scenario: Resolved model differs from requested alias

- **WHEN** `jev-latest` is requested and a concrete model name is returned
- **THEN** the output preserves the returned model name

#### Scenario: Wrong answer variant

- **WHEN** a named Noul question receives a Choice answer or an answer is missing
- **THEN** the evaluation fails with a response-contract error

#### Scenario: Invalid probability

- **WHEN** a probability lies outside zero to one
- **THEN** the plugin reports a response error without exposing it as a valid decision

### Requirement: Bounded successful response bodies

Each successful System One or model-list response SHALL be limited to 16 MiB (16,777,216 body bytes), independently of retries and rows. An oversized declared `Content-Length` SHALL be rejected before body reading; a response without a usable length SHALL be limited while reading. Oversize SHALL produce a nonretryable response error without JSON decoding, logging body contents, or returning partial data.

#### Scenario: Declared oversized body

- **WHEN** a successful response declares a body larger than 16 MiB
- **THEN** the plugin rejects it without reading or decoding the body

#### Scenario: Chunked oversized body

- **WHEN** a successful chunked response has no usable length declaration and exceeds 16 MiB while being read
- **THEN** the plugin stops reading and returns a response error without retrying it

#### Scenario: Body at the limit

- **WHEN** a successful body has exactly 16 MiB of data and satisfies the endpoint's JSON contract
- **THEN** the size limit alone does not reject it

### Requirement: Bounded retry policy

HTTP statuses `429`, `502`, `503`, `504`, and `529` SHALL be retried up to the configured number of additional attempts. Other 4xx errors, response decoding failures, expired deadlines, and ambiguous transport failures SHALL NOT be retried automatically. Reqwest's independent automatic protocol-NACK retries SHALL be disabled. An evaluation with retries set to zero SHALL make one wire attempt only. Retry attempts SHALL use the same logical request body, credentials, and proxy policy.

#### Scenario: Retryable overload followed by success

- **WHEN** the service returns `529` and then a valid response with retries enabled
- **THEN** the plugin waits before a second attempt and returns the successful response

#### Scenario: Nonretryable validation failure

- **WHEN** the service returns `422`
- **THEN** only one attempt is made and the HTTP error is returned

#### Scenario: Retry budget exhausted

- **WHEN** retries is three and every attempt returns `503`
- **THEN** no more than four attempts occur and the terminal error retains HTTP status `503`

#### Scenario: No hidden protocol retry

- **WHEN** retries is zero and the transport receives a protocol NACK
- **THEN** the plugin does not initiate a second wire attempt through the HTTP client

### Requirement: Retry delay respects service guidance

Retry delay SHALL follow finite, nonnegative, representable `retry-after-ms` first, then valid `Retry-After` delta seconds or HTTP date. Without valid guidance, it SHALL use a 500 ms initial base, double per retry up to 5 s, and apply 0–25% downward jitter. Waiting SHALL be cancellable. If the delay exceeds the remaining deadline, the request SHALL time out without an early retry.

#### Scenario: Millisecond guidance takes precedence

- **WHEN** a retryable response contains valid `retry-after-ms: 250` and `Retry-After: 2` with sufficient deadline remaining
- **THEN** the next attempt is delayed by at least 250 ms, without waiting for the two-second fallback

#### Scenario: Invalid millisecond guidance falls back

- **WHEN** `retry-after-ms` is invalid and `Retry-After: 2` is valid
- **THEN** the next attempt is not made before the two-second delay elapses

#### Scenario: Unguided retry uses bounded jitter

- **WHEN** a retryable response supplies no usable retry-delay header
- **THEN** the first retry waits between 375 and 500 ms, and later bases double up to the five-second cap before downward jitter

#### Scenario: Delta-seconds retry guidance

- **WHEN** a retryable response contains `Retry-After: 2` and sufficient deadline remains
- **THEN** another attempt is not made before the advised delay elapses

#### Scenario: Retry guidance exceeds deadline

- **WHEN** the advised retry delay exceeds the evaluation's remaining deadline
- **THEN** the evaluation ends with a timeout without an early retry

### Requirement: Per-evaluation total deadline

Timeout SHALL bound one dispatched logical evaluation, including all of its attempts and retry waits. It SHALL NOT impose a single deadline on an entire table invocation. Cancellation SHALL interrupt requests and waits without waiting for timeout expiry.

#### Scenario: Multiple attempts share one deadline

- **WHEN** retry waits and HTTP attempts together exhaust the configured timeout
- **THEN** the evaluation terminates even if no individual attempt consumed the full timeout

#### Scenario: Long table invocation

- **WHEN** each row finishes within its evaluation deadline but the whole table takes longer than that duration
- **THEN** the table does not fail solely because of total invocation elapsed time

### Requirement: Stable actionable error information

Errors SHALL identify validation, state, field collision, HTTP, transport, timeout, or response failure. HTTP failures SHALL retain status; non-HTTP failures SHALL have none.

#### Scenario: Row HTTP failure information

- **WHEN** a row evaluation ultimately fails with HTTP `503`
- **THEN** its error classification is `http` and its status is `503`

#### Scenario: Timeout information

- **WHEN** a request expires without an HTTP response
- **THEN** its classification is `timeout` and it does not invent a status code

### Requirement: Correlated bounded server response diagnostics

For a usable `x-typesafe-request-id`, non-blocking per-attempt diagnostics SHALL correlate its bounded, sanitized value with the local logical `request_id` and attempt number. The server ID SHALL NOT replace local identity or change Nu response or row-error schemas. Diagnostics SHALL be bounded and credential-redacted, without raw header dumps or unrestricted response bodies.

#### Scenario: Server response identity is available

- **WHEN** an HTTP attempt receives a response carrying a valid `x-typesafe-request-id` and debug tracing is enabled
- **THEN** its diagnostics include the sanitized server identifier, local logical request identity, and attempt number without logging other response headers

#### Scenario: Server response identity is unusable

- **WHEN** the header is missing, malformed, or exceeds the diagnostic length limit
- **THEN** the evaluation and its output remain unchanged and no untrusted header value is logged

### Requirement: Authenticated model-list endpoint and response

Model discovery SHALL send a bodyless `GET /v1/models` under the configured HTTP(S) root with the caller's bearer key; a trailing root slash SHALL NOT change the path. The response SHALL contain a `models` array of entries with string `name`, `description`, and `release_date`. Unknown fields SHALL be tolerated, but missing or wrong-typed required fields SHALL fail without a partial or fabricated list. Release dates SHALL remain opaque strings, without client-side date validation.

#### Scenario: Authenticated local listing

- **WHEN** a local mock root ending in `/` returns a valid model list
- **THEN** exactly one logical GET reaches `/v1/models` with the caller's bearer credential and no request body
- **AND** the typed entries are available for conversion to Nu rows

#### Scenario: Invalid model-list response

- **WHEN** the response omits `models`, makes it a non-array value, or omits a required string field from one entry
- **THEN** discovery fails with a response-contract error and returns no partial list

### Requirement: Model discovery shares live transport safeguards

`jev models` SHALL use live-command precedence and validation for caller key, root, proxy, timeout, and retries; missing/invalid keys SHALL fail before dispatch. GET SHALL share proxy policy, no-redirect rule, retryable statuses and delays, total deadline, cancellation, and redacted errors. Nonretryable 4xx, malformed JSON, and ambiguous transport failures SHALL not retry. Terminal HTTP status SHALL remain available; headers, keys, proxy credentials, and unbounded bodies SHALL not leak.

#### Scenario: Retryable status then success

- **WHEN** the list endpoint returns `503` with valid retry guidance and then a valid list within the configured deadline
- **THEN** discovery follows the existing bounded delay/retry policy and returns that list

#### Scenario: Authentication and redirect failures

- **WHEN** the endpoint returns `401` or redirects an authenticated request
- **THEN** the plugin neither retries the `401` nor forwards the bearer credential to a redirect target
- **AND** the caller receives an actionable, credential-redacted error

#### Scenario: Cancellation during listing

- **WHEN** the caller interrupts an in-progress models request or retry wait
- **THEN** pending network work stops without waiting for the full timeout

### Requirement: Return selected service root as metadata

For every validated System One evaluation or model-list lookup, metadata SHALL contain the invocation's selected, validated, normalized service root at `meta.base_url` (`ask`, `models`) or `jev_meta.base_url` (`annotate`), even without `--metrics`. The root SHALL NOT come from the response body, include an appended `/v1/...` endpoint, or be confused with the proxy URL.

#### Scenario: Selected service root survives retries

- **WHEN** a validated evaluation or model-list lookup succeeds after retrying against a non-default service root
- **THEN** its returned metadata record's `base_url` (`meta.base_url` on `ask`/`models`, `jev_meta.base_url` on `annotate`) equals that selected root with or without `--metrics`, and completion tracing records the same root
- **AND** it contains neither the proxy URL nor the full request endpoint

### Requirement: Measure successful HTTP body bytes

On a successful logical operation, `request_bytes` SHALL equal one attempt's compact UTF-8 JSON request body, or zero for bodyless `GET /v1/models`. `response_bytes` SHALL equal the final successful response body's consumed byte length for JSON decoding. Neither field SHALL include headers, TLS/proxy framing, HTTP frames, cumulative retry bodies, or character counts; they SHALL NOT claim to measure complete network traffic.

#### Scenario: Bodyless model lookup

- **WHEN** `GET /v1/models` returns a valid model list after one or more attempts
- **THEN** `request_bytes` is exactly zero, `response_bytes` measures only the final successful response body, and `attempts` counts explicit GET attempts
- **AND** the same deadline, retry-delay, and cancellation boundaries apply as for System One

#### Scenario: Protocol overhead is outside body metrics

- **WHEN** the request uses HTTP/2, a proxy, or multibyte JSON content
- **THEN** byte fields are based on the serialized request and consumed final response bodies, not headers, frame overhead, or character count

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

- **WHEN** a valid response arrives over HTTP/1.1 or HTTP/2 after any retryable responses
- **THEN** `http_version` names the protocol version of that final successful response
- **AND** it does not claim whether a new connection was established for the attempt

### Requirement: Return requested metrics only for validated success

With `--metrics`, validated `ask`/`models` SHALL return `metrics` and `annotate` SHALL return `jev_metrics`; otherwise neither SHALL appear. Metrics SHALL omit `base_url`, `model`, `usage`, state, questions, answers, credentials, proxy details, and inferred connection status. Evaluation `model` and `usage` SHALL always be in `meta`/`jev_meta`; annotation metrics SHALL include local `request_id` for sharing and aggregation.

#### Scenario: Successful measurement placement

- **WHEN** a validated evaluation succeeds with `--metrics`
- **THEN** its metrics contain `request_bytes`, `response_bytes`, `elapsed`, `attempt_elapsed`, `attempts`, and `http_version`
- **AND** evaluation `model` and `usage` remain in always-present metadata rather than metrics

#### Scenario: Invalid or failed evaluation

- **WHEN** the final response is malformed, fails answer validation, or the evaluation ends in an HTTP/transport/timeout error
- **THEN** no successful metrics record is returned or cached
