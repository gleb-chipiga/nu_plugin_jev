# jev-system-one-commands Specification

## Purpose

Provide one-state System One evaluations and exact request previews through `jev ask`, preserving typed answers for ordinary Nu field access and downstream processing.

## Requirements

### Requirement: Ask evaluates one state against all named questions

`<state> | jev ask <questions>` SHALL submit one System One evaluation containing the selected model, one state, and the full named questions map. It SHALL return the API envelope with `model`, `answers`, and `usage`, preserving answer names and typed answer fields.

#### Scenario: Mixed answers in one response

- **WHEN** one state is evaluated against Noul, Choice, and Score questions
- **THEN** one evaluation contains all questions
- **AND** the Nu response contains named typed answers, the model used, and input/output token usage

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

`--dry-run` on `jev ask` SHALL return the exact JSON-shaped Nu request body that the live evaluation would send: `model`, final `state`, and `questions`. It SHALL perform normal request validation but SHALL NOT require an API key or send HTTP. `--context` SHALL be reflected in the preview.

#### Scenario: Preview matches submitted body

- **WHEN** `jev ask` is first run with `--dry-run` and then live against a mock service using the same values and settings
- **THEN** the preview is structurally identical to the captured JSON request body
- **AND** the preview contains no credentials

### Requirement: Single-state failures are visible

`jev ask` SHALL return a labeled Nu error for invalid input, invalid configuration/questions, missing credentials, exhausted HTTP failures, deadline expiry, or invalid responses. It SHALL NOT substitute a default probability, option, or score.

#### Scenario: Invalid response for ask

- **WHEN** a Choice evaluation returns a missing or wrong-typed answer
- **THEN** `jev ask` returns an error rather than fabricating an answer
