# Spec Delta

## ADDED Requirements

### Requirement: Measure successful logical HTTP operations and service root

For every successful logical System One evaluation or model-list lookup, independently of the `--metrics` output flag, the returned metadata record's `base_url` SHALL identify the validated, normalized service root selected for that invocation. Its fixed path is `meta.base_url` on `ask` and `models`, or `jev_meta.base_url` on `annotate`. The root SHALL NOT be derived from or attributed to the TypeSafe response body, substituted with an appended `/v1/...` endpoint, or confused with the proxy URL. `request_bytes` SHALL be the compact UTF-8 JSON request-body length for one attempt, and SHALL be zero for the bodyless `GET /v1/models`. `response_bytes` SHALL be the byte length of the final successful response body consumed for JSON decoding.

`elapsed` SHALL measure from immediately before the first explicit HTTP attempt through successful response decoding and contract validation, including retry waits and later attempts but excluding pre-dispatch preparation and table queue time. `attempt_elapsed` SHALL measure from immediately before the final successful HTTP attempt through that same decoding and validation, excluding previous attempts and retry waits. Both durations SHALL use the same completion instant; with one successful attempt they SHALL be equal. When returned to Nu, both SHALL be nonnegative Nu durations. `attempts` SHALL count explicit HTTP attempts in that logical operation. `http_version` SHALL identify the HTTP protocol version of the final successful response as a string. These values SHALL NOT claim to measure headers, TLS/proxy framing, complete network traffic, cumulative retry bodies, or server-side processing alone. A `metrics` record on `ask`/`models`, or `jev_metrics` on `annotate`, SHALL be returned only with a validated success and `--metrics`; it SHALL contain no `base_url`, `model`, `usage`, request state, questions, answer content, credentials, proxy details, or inferred new-connection status. Validated evaluation `model` and `usage` SHALL instead appear in always-present `meta` on `ask` or `jev_meta` on `annotate`; annotation's optional `jev_metrics` SHALL additionally include a local `request_id` for sharing and aggregation.

#### Scenario: Selected service root survives retries

- **WHEN** a validated evaluation or model-list lookup succeeds after retrying against a non-default service root
- **THEN** its returned metadata record's `base_url` (`meta.base_url` on `ask`/`models`, `jev_meta.base_url` on `annotate`) equals that selected root with or without `--metrics`, and completion tracing records the same root
- **AND** it contains neither the proxy URL nor the full request endpoint

#### Scenario: Bodyless model lookup

- **WHEN** `GET /v1/models` returns a valid model list after one or more attempts
- **THEN** `request_bytes` is exactly zero, `response_bytes` measures only the final successful response body, and `attempts` counts explicit GET attempts
- **AND** the same deadline, retry-delay, and cancellation boundaries apply as for System One

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

#### Scenario: Invalid or failed evaluation

- **WHEN** the final response is malformed, fails answer validation, or the evaluation ends in an HTTP/transport/timeout error
- **THEN** no successful metrics record is returned or cached

#### Scenario: Protocol overhead is outside body metrics

- **WHEN** the request uses HTTP/2, a proxy, or multibyte JSON content
- **THEN** byte fields are based on the serialized request and consumed final response bodies, not headers, frame overhead, or character count
