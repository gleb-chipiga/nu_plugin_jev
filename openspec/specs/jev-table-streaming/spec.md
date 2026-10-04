# jev-table-streaming Specification

## Purpose

Annotate Nu record streams with independent Jev evaluations for native downstream processing, with bounded outstanding work, explicit ordering and failure policies, cancellation, single-flight deduplication, and bounded invocation-local result caching.

## Requirements

### Requirement: Annotation preserves rows and adds named answers

`jev annotate <questions>` SHALL incrementally evaluate each record independently with all named questions and emit its original fields plus answers under top-level `answers` by default. `--into <name>` SHALL choose another nonempty literal answer field. Each successful live row SHALL also receive fixed `jev_meta` and optional `jev_metrics` with `--metrics`; `--into` SHALL not move them. Unsent source fields SHALL remain. A single record SHALL be one row; an empty table SHALL yield no rows.

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

### Requirement: Output destinations do not overwrite input data

`--into` SHALL select a nonempty literal answer field distinct from fixed `jev_meta` and, when enabled, `jev_metrics`. Invalid destinations SHALL fail before input consumption. With `--on-error record`, the answer field SHALL also differ from `jev_error`. The old `--meta` switch SHALL be rejected.

#### Scenario: Identical annotation and metadata destinations

- **WHEN** `--into jev_meta` is supplied
- **THEN** invocation fails before consuming input rows because the answer and metadata destinations coincide

#### Scenario: Removed metadata switch

- **WHEN** the formerly supported `--meta ai` is supplied
- **THEN** invocation rejects that switch before consuming input rows; successful live rows instead receive fixed `jev_meta`

#### Scenario: Identical answer and metrics destinations

- **WHEN** `--into jev_metrics --metrics` is supplied
- **THEN** invocation fails before consuming input rows because the answer and enabled metrics destinations coincide

#### Scenario: Disabled metrics destination is not reserved for metrics

- **WHEN** `--into jev_metrics` is supplied without `--metrics`
- **THEN** the name is accepted as the answer destination if it does not collide with a source field

#### Scenario: Error destination remains reserved

- **WHEN** `--on-error record` is supplied with `--into jev_error`
- **THEN** invocation fails before consuming input rows

### Requirement: Enabled output destinations never overwrite rows

An existing enabled destination in a row SHALL cause terminal collision on reading, before request construction or dispatch, even when excluded from state. Collision SHALL stop admission, cancel outstanding work, and end the stream regardless of row-error policy or ordering. The row SHALL NOT pass through unchanged or with `jev_error`. Delivered rows cannot be rolled back; the plugin SHALL NOT collect the stream to search ahead.

#### Scenario: Annotation field collision

- **WHEN** an input row already has the configured annotation field
- **THEN** the stream terminates with a field-collision error before that row is sent to the service, even with `--on-error keep` or `record`
- **AND** its existing field is not overwritten or emitted as if it contained a new Jev answer

#### Scenario: Metrics field collision

- **WHEN** a row already contains the enabled `jev_metrics` destination, even if `--fields` excludes that source field
- **THEN** the stream terminates with a field-collision error before that row is sent to the service, without overwriting its field

#### Scenario: Meta field collision

- **WHEN** a row already contains the fixed `jev_meta` destination, even if `--fields` excludes it
- **THEN** the stream terminates with a field-collision error before that row is sent to the service, even without `--metrics`

#### Scenario: Later collision is not held behind an earlier request

- **WHEN** an earlier row's evaluation is stalled and a later row has an enabled output destination in its source record
- **THEN** the collision stops admission, cancels outstanding local work, and terminates without waiting for the earlier row's ordered result
- **AND** no request is submitted for the conflicting row

### Requirement: Error records do not overwrite source fields

Error-record insertion SHALL NOT overwrite an existing `jev_error`; if the field cannot be attached, the invocation SHALL terminate with an error.

#### Scenario: Existing error destination

- **WHEN** a failed row already contains `jev_error` under `--on-error record`
- **THEN** the invocation terminates rather than replacing that source field

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

`jev annotate --fields <list<string>>` SHALL build outbound state from exactly those top-level fields with original names and values before JSON conversion. Names, including dotted ones, SHALL be literal, not cell paths. The list SHALL be nonempty with distinct strings and incompatible with `--state`. Invalid arguments SHALL fail before input or HTTP, regardless of row policy or preview mode. Projection SHALL preserve the output row; complex state construction SHALL remain in Nu pipelines.

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

### Requirement: Projected rows retain state and collision semantics

A missing projected field SHALL cause a per-row state error under the selected row policy, never be skipped or replaced with null. Projection SHALL NOT bypass output-destination collision checks, even for fields excluded from outbound state.

#### Scenario: Missing projected field follows row policy

- **WHEN** a row lacks `sender` and `--fields [message sender] --on-error record` is supplied
- **THEN** no request is submitted for that row
- **AND** the complete original row is returned with a state-category `jev_error` and no annotation or metadata

#### Scenario: Excluded annotation destination still collides

- **WHEN** a row already contains `ai` and annotation is invoked with `--fields [message] --into ai`
- **THEN** the existing `ai` field causes a field-collision error before dispatch even though it is excluded from the projected state
- **AND** the existing value is not overwritten

### Requirement: Typed annotations compose with native Nu processing

Annotation SHALL preserve typed answers for native Nu cell paths without implicit probability thresholds or internal semantic filtering. Documentation SHALL show `where`, `sort-by`, and `reject` over answer fields. Failed `keep`/`record` rows SHALL remain available downstream without fabricated answers; documentation SHALL show callers handling them explicitly before applying decision predicates.

#### Scenario: Caller chooses an inclusive threshold

- **WHEN** annotated rows receive probabilities `0.97`, `0.98`, and `0.99` and downstream uses `where answers.spam.noul >= 0.98`
- **THEN** native Nu filtering retains the rows with `0.98` and `0.99`
- **AND** native `reject answers` can remove the annotation from those rows

#### Scenario: Native processing preserves projected source rows

- **WHEN** rows are annotated with `--fields [message sender] --into ai` and then processed with native `where` and `sort-by` over `ai` answers
- **THEN** only the projected fields were transmitted for each row
- **AND** every original field remains available to the downstream pipeline

### Requirement: Independent row states without automatic packing

`jev annotate` SHALL build one request body per valid row from its selected/projected input and explicit context, with all named questions. It SHALL NOT pack independent rows into a shared state, include neighbors, or add synthetic row references. Distinct canonical bodies SHALL cause independent evaluations; equivalent bodies MAY share one despite different unselected source fields.

#### Scenario: Three distinct row states

- **WHEN** three records yielding distinct canonical request bodies are annotated without retryable failures
- **THEN** the service receives three independent System One requests
- **AND** no request's state is the complete table

#### Scenario: Neighboring rows do not enter the projected state

- **WHEN** several rows are processed with `--fields [message]` and no additional context
- **THEN** each request's state contains only that row's `message` field, and the named questions remain unchanged
- **AND** no request adds a `rows` collection or rewrites the questions to refer to row indices

### Requirement: Explain independent rows and intentional joint state

Documentation SHALL distinguish one intentional array state from independent row processing. It SHALL NOT claim that lack of a batch endpoint makes client-side packing impossible or call duplicate sharing/retries a remote batch endpoint. Packing SHALL remain outside this change pending validation of quality, cross-row influence, context limits, partial failures, caching, and usage attribution.

#### Scenario: Describe request sharing without batch claims

- **WHEN** documentation explains annotation, in-flight sharing, or retries
- **THEN** it identifies independent row evaluations and does not describe them as a remote batch endpoint

### Requirement: Bounded outstanding work and input read-ahead

`--jobs N` SHALL bound outstanding unique evaluations, including retries, by `N`. Admission SHALL retain at most `2N` row outcomes not yet yielded or discarded, counting queued rows, duplicate waiters, ordered completed outcomes, and queued output. Input SHALL stream rather than be fully collected. The completed cache and upstream Nu protocol buffering SHALL be separate from this work bound.

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

`--on-error fail` SHALL be the default. After retries, the first observed terminal row failure SHALL stop admission and cancel remaining work without waiting for its ordered slot. The stream SHALL emit a terminal Nu error, with no later successes; delivered rows SHALL not roll back. Invalid arguments, config, credentials, or questions SHALL fail before stream processing in every row mode. Upstream Nu errors SHALL remain terminal.

#### Scenario: Later row fails while first row is stalled

- **WHEN** a later evaluation fails terminally before an earlier unfinished row
- **THEN** the failure cancels pending work promptly and terminates the stream
- **AND** it is not hidden indefinitely behind the earlier row

### Requirement: Keep and record row failure policies

`--on-error keep` SHALL return an ordinary failed row unchanged. `record` SHALL preserve a failed record with `jev_error: {kind, message, status}`, using null status for non-HTTP errors. Neither mode SHALL fabricate annotation or metadata. Non-record rows SHALL be errors: `keep` returns them unchanged, while `record` terminates because it cannot attach a field. Output collisions or existing `jev_error` SHALL terminate immediately, not pass through as success.

#### Scenario: Annotation keeps a failed row

- **WHEN** an HTTP error occurs with `--on-error keep`
- **THEN** the original row is returned without new decision or metadata fields

#### Scenario: Annotation records a failed judgment

- **WHEN** a row cannot be evaluated for a non-collision reason with `--on-error record`
- **THEN** the row is included with `jev_error`
- **AND** the error record identifies the category, message, and HTTP status or null

#### Scenario: Keep and record do not suppress destination collisions

- **WHEN** a source row already contains an enabled output destination and `--on-error keep` or `record` is selected
- **THEN** the stream terminates at that row without returning it as an unchanged or error-bearing record

### Requirement: Canonical request equivalence

Deduplication keys SHALL include canonical full request body and service root: final state (including context), questions, requested model, and other response-affecting parameters. Object field order SHALL not matter; array order, scalar representation, and omitted versus null SHALL. Comparisons SHALL NOT rely only on hashes. Keys SHALL omit credentials, and outcomes SHALL NOT cross credential-isolated invocations.

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

Identical requests admitted during one in-flight evaluation SHALL join it, including its retries, rather than dispatch again. Single-flight SHALL apply regardless of completed-cache capacity or eligibility. Each original row SHALL retain its own outcome within bounded admission. A completed failure SHALL reach current waiters and be removed, never cached as a completed result.

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

`jev annotate` SHALL check completed cache before in-flight work, refresh recency on hits, and cache only successes regardless of later native Nu filtering. Entries SHALL satisfy both `max_entries` and `max_approx_bytes`. Weight SHALL deterministically include canonical key bytes, serialized successful response, request identity, and documented fixed per-entry overhead.

#### Scenario: Cache hit changes recency

- **WHEN** A and B are cached in that order, A is reused, and C requires eviction with a two-entry limit
- **THEN** B is evicted and A remains reusable

### Requirement: Completed-cache eviction permits re-evaluation

Insertion SHALL evict least-recently-used entries to meet both limits. A duplicate after eviction MAY start another evaluation. Eviction SHALL NOT cancel active work or invalidate outcomes already held by admitted rows. Documentation SHALL distinguish cache bounds from a strict process-memory ceiling and SHALL NOT promise once-only evaluation throughout an invocation.

#### Scenario: Entry count triggers eviction

- **WHEN** the cache holds its maximum entry count and an eligible new success fits the byte budget
- **THEN** insertion evicts the least-recently-used entry
- **AND** both cache limits remain satisfied

#### Scenario: Byte budget triggers eviction

- **WHEN** inserting an eligible success would exceed the approximate byte limit while staying below the entry-count limit
- **THEN** least-recently-used entries are evicted until the byte limit is satisfied
- **AND** key bytes are included rather than accounting for responses alone

#### Scenario: Duplicate after eviction

- **WHEN** a successful request's entry has been evicted and an identical request appears with no equivalent operation in flight
- **THEN** a new logical evaluation is dispatched normally
- **AND** earlier rows keep their already-associated successful outcomes

### Requirement: Oversized cache entries bypass insertion

A successful entry exceeding the byte limit alone SHALL reach current rows without being cached or evicting existing entries for that insertion.

#### Scenario: Oversized success bypasses caching

- **WHEN** one successful entry alone exceeds `max_approx_bytes`
- **THEN** current rows still receive that success and existing cached entries are not evicted for it
- **AND** a later non-overlapping duplicate may submit a fresh request

### Requirement: Invocation-local cache lifetime

In-flight state and completed results SHALL belong exclusively to one table invocation and SHALL be released on completion, terminal failure, or output drop. Results SHALL NOT be reused across invocations, including for a moving alias such as `jev-latest`. No process-global or persistent cache SHALL be used.

#### Scenario: Alias reused by another command invocation

- **WHEN** a later table invocation submits a request identical to one previously completed with `jev-latest`
- **THEN** it performs a fresh evaluation instead of reusing the previous invocation's result

### Requirement: Always-present `jev_meta` and optional `jev_metrics` on annotations

Every successful live annotation SHALL preserve the source record and add answers plus fixed `jev_meta: {base_url, model, usage}`. `base_url` SHALL be the selected validated root; `model` and `usage` SHALL come from the API response. The old `--meta` flag SHALL be rejected.

#### Scenario: Default metadata without metrics

- **WHEN** a row is successfully annotated without `--metrics`
- **THEN** the original row and answers remain intact beside `jev_meta: {base_url, model, usage}`
- **AND** no `jev_metrics` field is added

#### Scenario: Metadata destination is fixed

- **WHEN** a row is successfully annotated with a custom answer `--into ai`
- **THEN** its selected root, returned model, and usage remain under `jev_meta` beside `ai`

### Requirement: Optional fixed annotation measurements

With `--metrics`, successful rows SHALL add fixed `jev_metrics: {request_id, request_bytes, response_bytes, elapsed, attempt_elapsed, attempts, http_version}` without repeating `base_url`, `model`, or `usage`. The flag SHALL fail before input consumption with `--dry-run` or available `--estimate-tokens`.

#### Scenario: Default metrics destination

- **WHEN** a row is successfully annotated with `--metrics`
- **THEN** `jev_meta` contains the selected root, returned model, and usage
- **AND** `jev_metrics` contains local request identity and HTTP measurements without `base_url`, `model`, or `usage`

#### Scenario: Metrics destination is fixed

- **WHEN** a row is successfully annotated with `--metrics --into ai`
- **THEN** local request identity and HTTP measurements remain under `jev_metrics`, while provenance remains under `jev_meta` and answers appear under `ai`

#### Scenario: Metrics destination collides with source data

- **WHEN** a row already has `jev_metrics` and `--metrics` enables that destination
- **THEN** the stream terminates immediately with a field-collision error without overwriting source data or sending that row

#### Scenario: Failed row has no success metrics

- **WHEN** an ordinary row failure is handled under `--on-error keep` or `record`
- **THEN** that row receives neither new answers nor fabricated `jev_meta` or `jev_metrics` fields

#### Scenario: Metrics reject offline modes

- **WHEN** `--metrics` is combined with `--dry-run` or an available `--estimate-tokens` switch
- **THEN** invocation fails before consuming input rows or sending HTTP

### Requirement: Shared evaluation identity and accounting

`request_id` SHALL identify one local logical evaluation, not a server idempotency key or billing receipt. In-flight/cache sharing SHALL reuse provenance, measurements, and ID; retries SHALL retain it, re-evaluation after eviction SHALL get a new ID. Documentation SHALL tell users to enable `--metrics` and count usage and body sizes once per distinct ID for shared evaluations.

#### Scenario: Duplicate rows share one evaluation's metrics

- **WHEN** two rows join one in-flight evaluation or reuse its completed cache entry with `--metrics`
- **THEN** their `jev_meta` records contain the same model and usage, and their `jev_metrics` records contain the same `request_id` and HTTP measurements
- **AND** both durations refer to that one shared evaluation, not the later rows' time waiting or cache lookup
- **AND** documentation tells callers to count usage and body sizes once per distinct `request_id`, not once per row

#### Scenario: Re-evaluation after eviction has new identity

- **WHEN** an identical request is evaluated again after its completed-cache entry was evicted
- **THEN** its `jev_metrics.request_id` differs from the evicted evaluation's identity
- **AND** usage and body sizes from both logical evaluations remain countable

### Requirement: Streaming request previews

`jev annotate --dry-run` SHALL stream one `{request: <exact body>, request_bytes: <integer>}` per valid row in input order, without collecting the table or collapsing duplicates. Bytes SHALL equal compact UTF-8 JSON body length, excluding wrapper and transport overhead. State SHALL respect cell paths, field projection, and context. Downstream truncation SHALL leave read-ahead bounded.

#### Scenario: Duplicate rows in preview

- **WHEN** two identical valid rows are processed with `--dry-run`
- **THEN** two corresponding preview wrappers are emitted in input order and no request is submitted

#### Scenario: Named questions in annotation preview

- **WHEN** a valid `jev annotate` invocation with Noul, Choice, and Score questions is run with `--dry-run`
- **THEN** each nested request body contains its selected state and all supplied questions under their original names
- **AND** no decisions or synthetic annotation fields are fabricated

#### Scenario: Projected preview matches the live request

- **WHEN** annotation is run first with `--fields [message sender] --context $policy --dry-run` and then against a mock service with identical inputs and settings
- **THEN** each preview's `request` is structurally identical to the corresponding captured live request
- **AND** each `request_bytes` equals that request's captured JSON body byte length
- **AND** excluded source fields never appear in either body

#### Scenario: Downstream truncates preview

- **WHEN** a long input is processed with `jev annotate --dry-run | first 10`
- **THEN** only a bounded prefix is consumed and wrapped
- **AND** no API request is sent

### Requirement: Preview safety and row-error policy

Dry-run previews SHALL need no credentials or HTTP and SHALL contain no fabricated API response, `jev_meta`, or `jev_metrics`. `--metrics` SHALL fail before input consumption. Ordinary row failures SHALL follow `fail`, `keep`, or `record`; collisions SHALL terminate before a conflicting preview. Documentation SHALL explain mixed preview wrappers and unchanged/error-bearing rows under keep/record. Behavior SHALL not depend on the `token-estimation` feature.

#### Scenario: Failed row keeps its existing preview policy

- **WHEN** one row lacks a projected field and `--on-error record` is selected
- **THEN** that row receives `jev_error` without a fabricated request or byte count
- **AND** other valid rows continue producing preview wrappers

#### Scenario: Preview collision is terminal

- **WHEN** a row already has the configured answer destination and `jev annotate --dry-run --on-error keep` is used
- **THEN** preview processing terminates at that row without returning it unchanged or emitting a request wrapper for it

#### Scenario: Preview does not fabricate live fields

- **WHEN** a valid row is processed with `jev annotate --dry-run`
- **THEN** its preview wrapper contains `request` and `request_bytes`, but no `jev_meta` or `jev_metrics`
