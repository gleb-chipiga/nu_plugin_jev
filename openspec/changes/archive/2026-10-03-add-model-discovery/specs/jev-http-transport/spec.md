# Spec Delta

## ADDED Requirements

### Requirement: Authenticated model-list endpoint and response

Model discovery SHALL use `GET /v1/models` under the configured HTTP(S) service root with the calling user's bearer API key and no request body. A trailing slash on the root SHALL NOT change the resolved path. The response SHALL have a `models` array; each entry SHALL contain string `name`, `description`, and `release_date` values. Unknown response fields SHALL NOT cause a response error by themselves, but missing or wrong-typed required fields SHALL cause a response error rather than a partial or fabricated list. The release-date value SHALL remain an opaque string rather than being rejected for not matching a client-side date parser.

#### Scenario: Authenticated local listing

- **WHEN** a local mock root ending in `/` returns a valid model list
- **THEN** exactly one logical GET reaches `/v1/models` with the caller's bearer credential and no request body
- **AND** the typed entries are available for conversion to Nu rows

#### Scenario: Invalid model-list response

- **WHEN** the response omits `models`, makes it a non-array value, or omits a required string field from one entry
- **THEN** discovery fails with a response-contract error and returns no partial list

### Requirement: Model discovery shares live transport safeguards

`jev models` SHALL resolve caller-scoped credentials, service root, proxy, timeout, and retries using the same precedence and validation as other live HTTP commands. A missing or invalid selected key SHALL fail before dispatch. The selected proxy policy, no-redirect rule, bounded retryable status set and retry-delay guidance, total deadline, cancellation, and redacted error classification SHALL apply to the model-list GET. Nonretryable 4xx responses, malformed JSON, and ambiguous transport failures SHALL not be retried; the HTTP status of a terminal response SHALL remain available without exposing request headers, keys, proxy credentials, or unbounded response bodies.

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
