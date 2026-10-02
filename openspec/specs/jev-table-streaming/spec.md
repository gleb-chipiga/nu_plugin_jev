# jev-table-streaming Specification

## Purpose

Annotate Nu record streams with independent Jev evaluations for native downstream processing, with bounded outstanding work, explicit ordering and failure policies, cancellation, single-flight deduplication, and bounded invocation-local result caching.

## Requirements

### Requirement: Annotation preserves rows and adds named answers

`jev annotate <questions>` SHALL incrementally evaluate each input record as an independent state against the complete questions map and emit the original record with `answers` under `jev`. `--into <name>` SHALL select a nonempty literal top-level output field. Successful annotation SHALL preserve every original field, including fields not sent when state selection is used. A single input record SHALL be treated as a one-row input; an empty table SHALL produce an empty output stream.

#### Scenario: Multiple questions for one row

- **WHEN** a row is annotated using spam, kind, and urgency questions
- **THEN** its logical evaluation contains all three questions together
- **AND** its added field contains the three answers without the surrounding model/usage envelope

#### Scenario: Custom annotation field

- **WHEN** annotation is invoked with `--into ai`
- **THEN** answers are accessible through paths such as `ai.spam.noul`
- **AND** the original fields remain unchanged

### Requirement: Output destinations do not overwrite input data

Annotation and metadata destination names SHALL be nonempty and distinct. An existing destination field in a row SHALL cause a row field-collision error before that row is submitted. Error-record insertion SHALL NOT overwrite an existing `jev_error`; inability to attach an error record SHALL terminate with an error.

#### Scenario: Annotation field collision

- **WHEN** an input row already has the configured annotation field
- **THEN** that row is not sent to the service and follows the selected row error policy
- **AND** its existing field is preserved

#### Scenario: Identical annotation and metadata destinations

- **WHEN** `--into ai --meta ai` is supplied
- **THEN** invocation fails before consuming input rows

### Requirement: Cell-path state selection and static context

`--state <cell-path>` SHALL select an existing value from each record using ordinary Nu field/index path semantics. With neither `--state` nor `--fields`, the whole record SHALL be the outbound input. Missing paths SHALL cause row errors rather than silently supplying null. `--context` SHALL wrap each selected or projected input according to the shared value contract. The plugin SHALL NOT execute a closure to construct state.

#### Scenario: Nested state with preserved row

- **WHEN** `--state payload.request` selects a nested string from a row
- **THEN** only that string is the input part of its request state
- **AND** the complete original row is returned on successful annotation

#### Scenario: Unselected unsupported value

- **WHEN** a row has an unrelated binary field but `--state message` selects a valid string
- **THEN** the unrelated field is preserved without being converted or sent

#### Scenario: Missing selected field

- **WHEN** the selected cell path does not exist in a row
- **THEN** that row produces a state error before HTTP dispatch

### Requirement: Explicit row-preserving field projection

`--fields <list<string>>` on `jev annotate` SHALL construct an outbound record containing exactly the listed top-level fields with their original names and values before JSON conversion. Names SHALL be literal, including names containing dots, rather than cell paths. The list SHALL be nonempty and contain distinct strings. `--fields` and `--state` SHALL be mutually exclusive. Invalid lists or conflicting flags SHALL fail before input consumption or HTTP dispatch, regardless of row error policy or preview mode. A missing projected field SHALL cause a per-row state error under the selected row error policy; it SHALL NOT be skipped or substituted with null. Projection SHALL NOT remove or modify any fields of the original output row or bypass output-destination collision checks. Complex state construction SHALL remain the responsibility of ordinary Nu pipelines.

#### Scenario: Selected columns with a complete source row

- **WHEN** a row with `id`, `message`, `sender`, and an unrelated binary field is annotated with `--fields [message sender]`
- **THEN** its request state is an object containing only `message` and `sender`
- **AND** the successful output retains `id`, the binary field, and every other original field alongside the answers

#### Scenario: Projection before context wrapping

- **WHEN** `--fields [message sender] --context $policy` is supplied
- **THEN** the request state is exactly `{input: {message: <row.message>, sender: <row.sender>}, context: <converted policy>}`
- **AND** unrelated row fields are absent from the request

#### Scenario: Literal field name containing a dot

- **WHEN** a row contains both a top-level `document.text` field and a nested `document.text` path, and `--fields ["document.text"]` is supplied
- **THEN** the outbound record contains the top-level field under the unchanged name `document.text`
- **AND** the nested path is not selected implicitly

#### Scenario: Invalid field-selection arguments

- **WHEN** `--fields` is an empty list, contains a non-string or duplicate name, or is supplied together with `--state`
- **THEN** invocation fails without reading input rows or submitting requests, even with `--on-error keep` or `--dry-run`

#### Scenario: Missing projected field follows row policy

- **WHEN** a row lacks `sender` and `--fields [message sender] --on-error record` is supplied
- **THEN** no request is submitted for that row
- **AND** the complete original row is returned with a state-category `jev_error` and no annotation or metadata

#### Scenario: Excluded annotation destination still collides

- **WHEN** a row already contains `ai` and annotation is invoked with `--fields [message] --into ai`
- **THEN** the existing `ai` field causes a field-collision error before dispatch even though it is excluded from the projected state
- **AND** the existing value is not overwritten

### Requirement: Typed annotations compose with native Nu processing

Annotation SHALL preserve typed answers for ordinary Nu cell-path access. The plugin SHALL NOT apply an implicit probability threshold or perform semantic filtering internally. Documentation SHALL show native `where` and `sort-by` over answer fields and native `reject` when callers want to remove annotations. Failed rows returned by `keep` or `record` SHALL remain available to downstream Nu commands without fabricating answer fields; documentation SHALL show that callers handle these rows explicitly when applying a decision predicate.

#### Scenario: Caller chooses an inclusive threshold

- **WHEN** annotated rows receive probabilities `0.97`, `0.98`, and `0.99` and downstream uses `where jev.spam.noul >= 0.98`
- **THEN** native Nu filtering retains the rows with `0.98` and `0.99`
- **AND** native `reject jev` can remove the annotation from those rows

#### Scenario: Native processing preserves projected source rows

- **WHEN** rows are annotated with `--fields [message sender] --into ai` and then processed with native `where` and `sort-by` over `ai` answers
- **THEN** only the projected fields were transmitted for each row
- **AND** every original field remains available to the downstream pipeline

### Requirement: Independent row states without automatic packing

`jev annotate` SHALL build a separate request body for each valid row using that row's selected or projected input and explicit context. It SHALL NOT automatically combine multiple independent rows into a shared array/object state, include neighboring rows, or rewrite questions with synthetic row references. Distinct canonical request bodies SHALL create independent logical evaluations; equivalent bodies SHALL remain eligible for duplicate sharing regardless of differences in unselected source fields. Each evaluation SHALL contain all named questions for that row. Documentation SHALL distinguish an array used as one intentional joint state from independent row processing, and SHALL NOT present the absence of a batch endpoint as proof that client-side packing is impossible. Packing independent rows SHALL remain outside this change pending separate validation of quality, cross-row influence, context limits, partial failures, caching, and usage attribution. Duplicate sharing and HTTP retries SHALL NOT be described as a remote batch endpoint.

#### Scenario: Three distinct row states

- **WHEN** three records yielding distinct canonical request bodies are annotated without retryable failures
- **THEN** the service receives three independent System One requests
- **AND** no request's state is the complete table

#### Scenario: Neighboring rows do not enter the projected state

- **WHEN** several rows are processed with `--fields [message]` and no additional context
- **THEN** each request's state contains only that row's `message` field, and the named questions remain unchanged
- **AND** no request adds a `rows` collection or rewrites the questions to refer to row indices

### Requirement: Bounded outstanding work and input read-ahead

`--jobs N` SHALL limit outstanding unique logical evaluations, including their retries, to at most `N`. Plugin-controlled admission SHALL retain no more than `2N` row outcomes that have not yet been yielded or discarded on termination. This bound SHALL include queued rows, duplicate waiters, completed outcomes awaiting order, and queued output. Table input SHALL NOT be fully collected before results are returned. The separately bounded completed cache and upstream Nu protocol buffering SHALL be distinguished from this work bound.

#### Scenario: Slow first row in ordered mode

- **WHEN** the first evaluation is stalled while subsequent evaluations finish on a long input
- **THEN** outstanding row outcomes and input admission remain within the stated bound
- **AND** completed later rows do not permit an unbounded reorder backlog

#### Scenario: Many duplicates await one request

- **WHEN** a long input repeats a state whose shared evaluation is stalled
- **THEN** duplicate waiters consume the same bounded row admission budget
- **AND** the plugin does not read the whole input into an unbounded waiter list

### Requirement: Ordered default and opt-in unordered output

Results SHALL preserve input order by default, including failed rows returned by `keep` or `record`. With `--unordered`, completed row outcomes SHALL be available without waiting for earlier unfinished rows; ordering among simultaneously ready outcomes SHALL not be promised.

#### Scenario: Completion order differs from input order

- **WHEN** row two completes before row one in default mode
- **THEN** successful output places row one before row two

#### Scenario: Unordered early completion

- **WHEN** row one is stalled and row two completes with `--unordered`
- **THEN** row two can be consumed while row one remains unfinished

### Requirement: Interrupts cancel the invocation

An engine interrupt SHALL stop further input admission, cancel pending local HTTP/retry work, release retained invocation state, and close the output stream promptly. Interrupt handling SHALL remain active after the command returns its stream and SHALL NOT be treated as an ordinary row error by `keep` or `record`.

#### Scenario: Interrupt after stream creation

- **WHEN** the engine signals interruption while row evaluations or retry delays are pending after stream creation
- **THEN** the invocation stops without waiting for those request timeouts or retry delays to finish

#### Scenario: Interrupt during output backpressure

- **WHEN** the engine signals interruption while an outcome waits to enter a full output channel and the receiver has not been dropped
- **THEN** the invocation stops without waiting for the receiver to consume another row
- **AND** pending local evaluations are cancelled

### Requirement: Downstream truncation stops background work

When downstream stops consuming and drops the output stream, the invocation SHALL stop admitting input and cancel outstanding local work promptly. Detection SHALL NOT depend on the next completed request or a subsequent failed output send. Cancellation SHALL unblock producers/consumers under plugin control. The plugin SHALL NOT promise that requests already accepted by the remote service were undone.

#### Scenario: First ten results from a long input

- **WHEN** `jev annotate <questions> | first 10` closes consumption on a long source
- **THEN** the plugin stops local work for the remaining source rather than continuing to evaluate thousands of rows
- **AND** any read-ahead or already-submitted evaluations remain bounded by the invocation's work limits

#### Scenario: Downstream closes while HTTP is stalled

- **WHEN** downstream drops the output while all pending HTTP requests are waiting
- **THEN** cancellation occurs without needing an HTTP response to trigger another channel send

### Requirement: Terminal failure policy

`--on-error fail` SHALL be the default. After retry policy is exhausted, the first observed terminal row failure SHALL stop input admission and cancel remaining evaluations without waiting for its ordered slot. The stream SHALL emit a terminal Nu error value and no successful values after that error. Rows already delivered SHALL not be rolled back. Invalid invocation arguments, configuration, credentials, or question maps SHALL fail before stream processing regardless of row error mode; existing upstream Nu errors SHALL remain terminal.

#### Scenario: Later row fails while first row is stalled

- **WHEN** a later evaluation fails terminally before an earlier unfinished row
- **THEN** the failure cancels pending work promptly and terminates the stream
- **AND** it is not hidden indefinitely behind the earlier row

### Requirement: Keep and record row failure policies

With `--on-error keep`, a failed row SHALL be returned unchanged. With `--on-error record`, a failed record SHALL be preserved with `jev_error: {kind, message, status}`, where non-HTTP status is null. Failed rows SHALL receive no new annotation or metadata. Non-record rows SHALL be errors; `keep` SHALL return them unchanged, while `record` SHALL terminate because it cannot attach a field.

#### Scenario: Annotation keeps a failed row

- **WHEN** an HTTP error occurs with `--on-error keep`
- **THEN** the original row is returned without new decision or metadata fields

#### Scenario: Annotation records a failed judgment

- **WHEN** a row cannot be evaluated by `jev annotate` with `--on-error record`
- **THEN** the row is included with `jev_error`
- **AND** the error record identifies the category, message, and HTTP status or null

### Requirement: Canonical request equivalence

Deduplication keys SHALL cover the complete canonical request body and service root, including final state, questions, requested model, and all other response-affecting request parameters. Context SHALL participate through the final state. Object field order SHALL not change equivalence; array order, scalar representations, and omitted-versus-null fields SHALL remain distinct. Key comparisons SHALL NOT rely solely on hash equality. Credentials SHALL NOT be stored in keys, and outcomes SHALL NOT be shared across credential-isolated invocations.

#### Scenario: Object order and array order

- **WHEN** two state objects differ only in field insertion order
- **THEN** they share their evaluation
- **AND** states with differently ordered arrays do not share an evaluation solely because they contain the same elements

#### Scenario: Different request parameters

- **WHEN** requests have the same input but differ in questions, explicit context, requested model, another response-affecting field, or service root
- **THEN** they are not treated as equivalent requests

#### Scenario: Source rows differ only in excluded fields

- **WHEN** rows have different identifiers or other excluded fields but identical `message` and `sender` values, and use `--fields [message sender]` within the same invocation
- **THEN** their equivalent request bodies are eligible for the same in-flight evaluation or cached result
- **AND** each output preserves its own identifier and complete original record

### Requirement: Mandatory in-flight single-flight deduplication

Identical requests admitted while one logical evaluation is in flight SHALL join that evaluation rather than dispatch a second independent evaluation. Its retries SHALL remain part of the same shared operation. Single-flight SHALL apply independently of completed-cache capacity or entry eligibility. Every original row SHALL still have its own output outcome within the bounded admission window. A completed failure SHALL be delivered to its current waiters and removed without being retained in the completed cache.

#### Scenario: Concurrent duplicate during retries

- **WHEN** a duplicate request is admitted while the first evaluation awaits a response or retry delay
- **THEN** it joins that logical evaluation without initiating its own HTTP operation
- **AND** each admitted row receives the shared outcome under its own row handling

#### Scenario: Concurrent oversized duplicates

- **WHEN** identical requests overlap in flight but their eventual successful cache entry is larger than the byte limit
- **THEN** they still share one logical evaluation
- **AND** cache ineligibility does not turn concurrent duplicates into independent operations

#### Scenario: Later duplicate after failure

- **WHEN** a shared evaluation fails in keep mode and its state appears later after that evaluation completes
- **THEN** the later row can initiate a fresh evaluation rather than receiving a permanently cached failure

### Requirement: Bounded LRU of successful outcomes

`jev annotate` SHALL check the completed cache before the in-flight lookup, refresh recency on a cache hit, and cache only successful outcomes. Cache eligibility SHALL NOT depend on subsequent native Nu filtering. Retained cache entries SHALL satisfy both configured `max_entries` and `max_approx_bytes`. Approximate entry weights SHALL deterministically account for canonical key bytes, the serialized successful response, request identity, and a documented fixed per-entry overhead estimate. Insertion SHALL evict least-recently-used entries as necessary; later duplicates of evicted entries MAY initiate new logical evaluations. An entry exceeding the byte limit by itself SHALL be returned to current rows without caching it or evicting other entries for its insertion. Cache eviction SHALL NOT cancel active evaluations or invalidate outcomes held by admitted rows. Documentation SHALL distinguish these bounds from a strict process-memory ceiling and SHALL NOT promise once-only evaluation for an entire invocation.

#### Scenario: Entry count triggers eviction

- **WHEN** the cache holds its maximum entry count and an eligible new success fits the byte budget
- **THEN** insertion evicts the least-recently-used entry
- **AND** both cache limits remain satisfied

#### Scenario: Byte budget triggers eviction

- **WHEN** inserting an eligible success would exceed the approximate byte limit while staying below the entry-count limit
- **THEN** least-recently-used entries are evicted until the byte limit is satisfied
- **AND** key bytes are included rather than accounting for responses alone

#### Scenario: Cache hit changes recency

- **WHEN** A and B are cached in that order, A is reused, and C requires eviction with a two-entry limit
- **THEN** B is evicted and A remains reusable

#### Scenario: Duplicate after eviction

- **WHEN** a successful request's entry has been evicted and an identical request appears with no equivalent operation in flight
- **THEN** a new logical evaluation is dispatched normally
- **AND** earlier rows keep their already-associated successful outcomes

#### Scenario: Oversized success bypasses caching

- **WHEN** one successful entry alone exceeds `max_approx_bytes`
- **THEN** current rows still receive that success and existing cached entries are not evicted for it
- **AND** a later non-overlapping duplicate may submit a fresh request

### Requirement: Invocation-local cache lifetime

In-flight state and completed results SHALL belong exclusively to one table invocation and SHALL be released on completion, terminal failure, or output drop. Results SHALL NOT be reused across invocations, including for a moving alias such as `jev-latest`. No process-global or persistent cache SHALL be used.

#### Scenario: Alias reused by another command invocation

- **WHEN** a later table invocation submits a request identical to one previously completed with `jev-latest`
- **THEN** it performs a fresh evaluation instead of reusing the previous invocation's result

### Requirement: Optional metadata identifies shared usage

`--meta <name>` on annotation SHALL add a separate record containing the returned `model`, `usage`, and a unique local `request_id` for the logical evaluation. Duplicate rows sharing an in-flight operation or cached outcome SHALL preserve the same identity and usage. Retries SHALL retain that identity. A new evaluation, including one after eviction or cache bypass, SHALL receive a new identity. Documentation SHALL instruct users to count reported usage once per distinct identity and SHALL distinguish the identity from a server idempotency key or billing receipt.

#### Scenario: Duplicate rows share metadata provenance

- **WHEN** two rows reuse one evaluation with metadata enabled
- **THEN** their metadata contains identical request identity, model, and usage
- **AND** grouping by request identity counts that reported usage once

#### Scenario: Re-evaluation after eviction has new provenance

- **WHEN** an identical request is successfully evaluated again after its completed-cache entry was evicted
- **THEN** its metadata has a new request identity
- **AND** reported usage from both logical evaluations is counted rather than collapsing them by state

### Requirement: Streaming request previews

`--dry-run` on `jev annotate` SHALL emit one exact request-body record per valid input row in input order without collecting the whole table, requiring credentials, making HTTP calls, or collapsing duplicates. Previewed state SHALL respect cell paths, explicit field projection, and context. It SHALL not pretend that an API response exists. Row validation failures SHALL follow `fail`, `keep`, or `record`; documentation SHALL explain that keep/record previews can therefore contain original failed-row forms among request bodies.

#### Scenario: Duplicate rows in preview

- **WHEN** two identical valid rows are processed with `--dry-run`
- **THEN** two corresponding request bodies are emitted and no request is submitted

#### Scenario: Named questions in annotation preview

- **WHEN** a valid `jev annotate` invocation with Noul, Choice, and Score questions is run with `--dry-run`
- **THEN** each request body contains its selected state and all supplied questions under their original names
- **AND** no decisions or synthetic annotation fields are fabricated

#### Scenario: Projected preview matches the live request

- **WHEN** annotation is run first with `--fields [message sender] --context $policy --dry-run` and then against a mock service with identical inputs and settings
- **THEN** the preview's projected and wrapped state is structurally identical to the captured live request
- **AND** excluded source fields never appear in either body
