# jev-system-one-commands Specification

## Purpose

Provide one-state System One evaluations and exact request previews through `jev ask`, preserving typed answers for ordinary Nu field access and downstream processing.

## Requirements

### Requirement: Ask evaluates one state against all named questions

`<state> | jev ask <questions>` SHALL submit one System One evaluation with the selected model, one state, and every named question. Validated live success SHALL return `{answers, meta: {base_url, model, usage}}`, preserving typed answer names and fields. `base_url` SHALL be the selected validated root; `model` and `usage` SHALL come from the API response, not legacy top-level fields. Dry runs and offline token estimates SHALL NOT fabricate API response provenance.

#### Scenario: Mixed answers in one response

- **WHEN** one state is evaluated against Noul, Choice, and Score questions
- **THEN** one evaluation contains all questions
- **AND** the Nu response contains named typed answers and `meta` containing the selected service root, returned model, and input/output token usage

### Requirement: Lists and finite streams are one array state

A list or finite input stream passed to `jev ask` SHALL be one array state and SHALL NOT be interpreted as independent row requests. Documentation SHALL explain that forming one array state consumes the finite stream and requires memory proportional to that state.

#### Scenario: Thread-level judgment

- **WHEN** a three-message list is piped to `jev ask` with a named thread-level Noul question
- **THEN** one request is made with the three-message array as its state

#### Scenario: Finite stream passed to ask

- **WHEN** a finite stream of records is piped to `jev ask`
- **THEN** it is collected into one JSON array for one evaluation
- **AND** no hidden table batch behavior is introduced

### Requirement: Typed answers support native field access

`jev ask` SHALL retain complete tagged answer records under `answers` without imposing a probability threshold, rounding expected scores, or recomputing confidence. Native Nu field access SHALL expose the returned Noul probability, chosen option, fractional Score, and detailed fields without a separate plugin projection command or `--details` flag. Documentation SHALL show field access such as `get answers.spam.noul`, `get answers.kind.choice`, and `get answers.urgency.score`.

#### Scenario: Native Noul projection

- **WHEN** a question named `spam` receives `{type: noul, noul: 0.973}`
- **THEN** the returned envelope preserves that answer record
- **AND** native `get answers.spam.noul` produces float `0.973`

#### Scenario: Complete Choice answer

- **WHEN** a question named `kind` receives a valid Choice answer
- **THEN** `answers.kind` includes `type`, `choice`, `confidence`, and `probabilities`
- **AND** confidence is not replaced by the largest probability

#### Scenario: Fractional Score result

- **WHEN** a question named `urgency` receives expected score `2.4`
- **THEN** native `get answers.urgency.score` returns numeric `2.4` rather than a rounded level
- **AND** `answers.urgency` also preserves type, confidence, legend, and probabilities

### Requirement: Exact single-state dry runs

`jev ask --dry-run` SHALL return `{request: <exact JSON-shaped Nu body>, request_bytes: <integer>}`. `request` SHALL contain the live body's model, final state (including `--context`), and questions. `request_bytes` SHALL equal compact UTF-8 JSON bytes of that body, excluding the wrapper and transport overhead.

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

### Requirement: Dry-run validation without live dependencies

Dry runs SHALL validate the request normally but SHALL NOT require an API key, supported token-estimation model, or HTTP access. The same preview contract SHALL hold without the `token-estimation` Cargo feature.

#### Scenario: Offline request preview

- **WHEN** a valid dry run is performed without a key and with the service unavailable
- **THEN** the exact request and byte length are returned without an HTTP call

### Requirement: Optional live metrics for one state

`jev ask --metrics` SHALL run normally and, on validated success, add `metrics: {request_bytes, response_bytes, elapsed, attempt_elapsed, attempts, http_version}` beside `answers` and fixed `meta`. Metrics SHALL exclude `base_url`, `model`, and `usage`. Without the flag, `answers` and `meta` SHALL remain without measurements. Combining `--metrics` with `--dry-run` or available `--estimate-tokens` SHALL fail with a labeled argument error before HTTP.

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

### Requirement: Single-state failures are visible

`jev ask` SHALL return a labeled Nu error for invalid input, invalid configuration/questions, missing credentials, exhausted HTTP failures, deadline expiry, or invalid responses. It SHALL NOT substitute a default probability, option, or score.

#### Scenario: Invalid response for ask

- **WHEN** a Choice evaluation returns a missing or wrong-typed answer
- **THEN** `jev ask` returns an error rather than fabricating an answer
