# Spec Delta

## MODIFIED Requirements

### Requirement: Ask evaluates one state against all named questions

`<state> | jev ask <questions>` SHALL submit one System One evaluation containing the selected model, one state, and the full named questions map. On validated live success it SHALL return `{answers, meta: {base_url, model, usage}}`, preserving answer names and typed answer fields. `base_url` SHALL be the selected validated service root, while `model` and `usage` SHALL come from the validated API response. The legacy top-level `model` and `usage` fields SHALL move into fixed `meta`. No API response provenance SHALL be fabricated for dry-run or offline token estimation.

#### Scenario: Mixed answers in one response

- **WHEN** one state is evaluated against Noul, Choice, and Score questions
- **THEN** one evaluation contains all questions
- **AND** the Nu response contains named typed answers and `meta` containing the selected service root, returned model, and input/output token usage

### Requirement: Exact single-state dry runs

`--dry-run` on `jev ask` SHALL return `{request: <exact JSON-shaped Nu request body>, request_bytes: <integer>}`. The nested `request` SHALL contain the `model`, final `state`, and `questions` that the live evaluation would send. `request_bytes` SHALL equal the compact UTF-8 JSON serialization length of that body, excluding the preview wrapper and HTTP transport overhead. The preview SHALL perform normal request validation but SHALL NOT require an API key, a supported token-estimation model, or HTTP access. `--context` SHALL be reflected in `request.state`. The same output contract SHALL hold when the plugin is built without the `token-estimation` Cargo feature.

#### Scenario: Preview matches submitted body

- **WHEN** `jev ask` is first run with `--dry-run` and then live against a mock service using the same values and settings
- **THEN** `preview.request` is structurally identical to the captured JSON request body
- **AND** `preview.request_bytes` equals the captured body's byte length
- **AND** the preview contains no credentials

#### Scenario: Existing dry-run field access migrates through request

- **WHEN** a caller previously read `model`, `state`, or `questions` directly from a dry-run result
- **THEN** the corresponding request-body fields are available under `request`
- **AND** no measurement metadata is inserted into the nested body

#### Scenario: Byte preview without token support

- **WHEN** the plugin lacks the `token-estimation` feature or the requested model lacks token-counting rules
- **THEN** a valid `jev ask --dry-run` still returns its request and serialized byte size without an API call

#### Scenario: Multibyte and escaped state

- **WHEN** the state contains multibyte UTF-8 or JSON-escaped characters
- **THEN** `request_bytes` counts serialized JSON bytes, not Nu display characters or Unicode scalar values

## ADDED Requirements

### Requirement: Optional live metrics for one state

`jev ask --metrics` SHALL perform the ordinary live evaluation and, on validated success, add `metrics: {request_bytes, response_bytes, elapsed, attempt_elapsed, attempts, http_version}` beside `answers` and fixed `meta`. `metrics` SHALL contain HTTP measurements only: no `base_url`, `model`, or `usage`. Without `--metrics`, the same `answers` and `meta` records SHALL remain and no HTTP measurements SHALL be added. `--metrics` SHALL require a live call and SHALL fail with a labeled argument error when combined with `--dry-run` or, when available, `--estimate-tokens`.

#### Scenario: Successful ask with metrics

- **WHEN** a valid state is evaluated with `jev ask --metrics`
- **THEN** the complete typed API answer remains accessible under `answers`
- **AND** `meta` contains the selected root, returned model, and usage while `metrics` contains nonnegative body-byte counts, total and successful-attempt Nu durations, positive explicit-attempt count, and final response HTTP version

#### Scenario: Ordinary ask retains provenance

- **WHEN** the same state is evaluated without `--metrics`
- **THEN** its response contains `answers` and `meta: {base_url, model, usage}` without `metrics`

#### Scenario: Metrics are live-only

- **WHEN** `--metrics` is combined with `--dry-run` or an available `--estimate-tokens` switch
- **THEN** invocation fails before sending HTTP
