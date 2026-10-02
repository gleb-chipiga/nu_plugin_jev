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

In `auto` mode, HTTP requests SHALL use reqwest's standard process environment and OS proxy discovery, including applicable `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY` behavior. In `direct` mode, they SHALL bypass all proxies. An explicit `http://` or `socks5h://` proxy SHALL override automatic discovery and SHALL NOT be bypassed by a global `NO_PROXY`; `socks5h://` SHALL resolve destination hostnames through the proxy. A failure of an explicitly selected proxy SHALL surface as an error rather than silently retrying via a system proxy or a direct connection. Proxy-policy changes between invocations SHALL select reusable clients by effective policy; a client SHALL NOT be built per row, and the retained client cache SHALL be bounded.

#### Scenario: Automatic proxy bypass

- **WHEN** `auto` mode is selected and ordinary proxy settings exclude the destination through `NO_PROXY`
- **THEN** the request follows standard automatic bypass behavior

#### Scenario: Explicit proxy overrides ordinary bypass

- **WHEN** an explicit Jev proxy is selected while global `NO_PROXY` matches the destination
- **THEN** the request uses the explicit proxy rather than silently connecting directly

#### Scenario: Explicit proxy is unavailable

- **WHEN** the selected explicit proxy cannot be reached
- **THEN** the evaluation fails with a redacted transport error and does not connect directly

#### Scenario: Reuse within a table

- **WHEN** multiple rows in one invocation share the same effective proxy policy
- **THEN** they reuse a pooled client rather than constructing a client for each row

### Requirement: Typed response contract validation

An evaluation response SHALL include model, named answers, and usage. Answer names and variants SHALL match the submitted questions. Noul SHALL preserve `noul`; Choice SHALL preserve `choice`, `confidence`, and `probabilities`; Score SHALL preserve `score`, `confidence`, `legend`, and `probabilities`. Required fields, representable numeric values, probability/confidence domains, and membership of returned options/levels SHALL be validated. Token counts SHALL be nonnegative. Invalid responses SHALL cause a response error rather than a fabricated decision.

#### Scenario: Resolved model differs from requested alias

- **WHEN** `jev-latest` is requested and a concrete model name is returned
- **THEN** the output preserves the returned model name

#### Scenario: Wrong answer variant

- **WHEN** a named Noul question receives a Choice answer or an answer is missing
- **THEN** the evaluation fails with a response-contract error

#### Scenario: Invalid probability

- **WHEN** a probability lies outside zero to one
- **THEN** the plugin reports a response error without exposing it as a valid decision

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

A valid finite, nonnegative, representable `retry-after-ms` value SHALL determine the retry delay in milliseconds and take precedence over `Retry-After`. If it is absent or invalid, a valid `Retry-After` value in delta seconds or HTTP-date form SHALL determine the delay. Without usable guidance, retries SHALL use capped exponential backoff: a 500 ms initial base doubled for each subsequent retry, capped at 5 seconds, with 0–25% downward jitter applied to each base. Waiting SHALL be cancellable. If the required delay cannot fit within the remaining deadline, the request SHALL time out rather than retry before the advised time.

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

Errors SHALL identify validation/state/field-collision, HTTP, transport, timeout, or response failure categories as applicable. HTTP failures SHALL retain their status, and non-HTTP failures SHALL have no HTTP status. When an HTTP response provides a usable `x-typesafe-request-id`, non-blocking per-attempt diagnostics SHALL correlate that bounded, sanitized server identifier with the local logical `request_id` and attempt number. The server identifier SHALL NOT replace the local identity or change the Nu response and row-error schemas. Diagnostics SHALL be bounded and credential-redacted rather than dumping raw headers or unrestricted server bodies.

#### Scenario: Server response identity is available

- **WHEN** an HTTP attempt receives a response carrying a valid `x-typesafe-request-id` and debug tracing is enabled
- **THEN** its diagnostics include the sanitized server identifier, local logical request identity, and attempt number without logging other response headers

#### Scenario: Server response identity is unusable

- **WHEN** the header is missing, malformed, or exceeds the diagnostic length limit
- **THEN** the evaluation and its output remain unchanged and no untrusted header value is logged

#### Scenario: Row HTTP failure information

- **WHEN** a row evaluation ultimately fails with HTTP `503`
- **THEN** its error classification is `http` and its status is `503`

#### Scenario: Timeout information

- **WHEN** a request expires without an HTTP response
- **THEN** its classification is `timeout` and it does not invent a status code
