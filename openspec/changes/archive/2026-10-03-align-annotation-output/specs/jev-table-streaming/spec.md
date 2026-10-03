# Spec Delta

## MODIFIED Requirements

### Requirement: Annotation preserves rows and adds named answers

`jev annotate <questions>` SHALL incrementally evaluate each input record as an independent state against the complete questions map and emit the original record with `answers` under the top-level `answers` field by default. `--into <name>` SHALL select a nonempty literal top-level answer field instead. Each successful live row SHALL also have the separate fixed, always-present `jev_meta` record from `add-request-metrics`, plus optional fixed `jev_metrics` when `--metrics` is selected; `--into` SHALL change neither destination. Successful annotation SHALL preserve every original field, including fields not sent when state selection is used. A single input record SHALL be treated as a one-row input; an empty table SHALL produce an empty output stream.

#### Scenario: Multiple questions for one row

- **WHEN** a row is annotated using spam, kind, and urgency questions without `--into`
- **THEN** its logical evaluation contains all three questions together
- **AND** `row.answers` contains the three named answers without the surrounding model/usage envelope
- **AND** `row.jev_meta` contains the selected service root, returned model, and usage

#### Scenario: Custom annotation field

- **WHEN** annotation is invoked with `--into ai`
- **THEN** answers are accessible through paths such as `ai.spam.noul` instead of `answers.spam.noul`
- **AND** the original fields and independent `jev_meta` destination remain unchanged

#### Scenario: Existing default answer field

- **WHEN** a source row already has `answers` and annotation is invoked without `--into`
- **THEN** the stream terminates with a field-collision error as soon as that row is read, before its request is constructed or submitted, regardless of `--on-error keep|record`
- **AND** the row is not passed through with its existing `answers` field or a new `jev_error`

### Requirement: Typed annotations compose with native Nu processing

Annotation SHALL preserve typed answers for ordinary Nu cell-path access. The plugin SHALL NOT apply an implicit probability threshold or perform semantic filtering internally. Documentation SHALL show native `where` and `sort-by` over answer fields and native `reject` when callers want to remove annotations. Failed rows returned by `keep` or `record` SHALL remain available to downstream Nu commands without fabricating answer fields; documentation SHALL show that callers handle these rows explicitly when applying a decision predicate.

#### Scenario: Caller chooses an inclusive threshold

- **WHEN** annotated rows receive probabilities `0.97`, `0.98`, and `0.99` and downstream uses `where answers.spam.noul >= 0.98`
- **THEN** native Nu filtering retains the rows with `0.98` and `0.99`
- **AND** native `reject answers` can remove the annotation from those rows

#### Scenario: Native processing preserves projected source rows

- **WHEN** rows are annotated with `--fields [message sender] --into ai` and then processed with native `where` and `sort-by` over `ai` answers
- **THEN** only the projected fields were transmitted for each row
- **AND** every original field remains available to the downstream pipeline
