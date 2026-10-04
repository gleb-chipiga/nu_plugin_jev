# Spec Delta

## MODIFIED Requirements

### Requirement: Interrupts cancel the invocation

An engine interrupt SHALL promptly stop plugin-controlled input admission,
HTTP/retry work, and output. The plugin SHALL submit no row after an already
entered upstream `next()` returns. An external iterator blocked inside `next()`
cannot be forcibly released by the plugin. Interrupt handling SHALL remain
active after the command returns its stream and SHALL NOT become a `keep` or
`record` row error.

#### Scenario: Interrupt after stream creation

- **WHEN** the engine signals interruption while row evaluations or retry delays are pending after stream creation
- **THEN** the invocation stops without waiting for those request timeouts or retry delays to finish

#### Scenario: Interrupt during output backpressure

- **WHEN** the engine signals interruption while an outcome waits to enter a full output channel and the receiver has not been dropped
- **THEN** the invocation stops without waiting for the receiver to consume another row
- **AND** pending local evaluations are cancelled

#### Scenario: Upstream call is already blocked

- **WHEN** interruption occurs while an external iterator is blocked inside `next()`
- **THEN** local HTTP and output work stop promptly without waiting for that call
- **AND** the producer sends no new row or request when `next()` eventually returns

### Requirement: Downstream truncation stops background work

When downstream drops the output stream, the invocation SHALL promptly stop
plugin-controlled admission and outstanding local work. Detection SHALL NOT
depend on another HTTP completion or failed send. Cancellation SHALL unblock
plugin-controlled producers and consumers; it cannot forcibly end an external
iterator already blocked in `next()`. Remote requests already accepted SHALL
not be claimed as undone.

#### Scenario: First ten results from a long input

- **WHEN** `jev annotate <questions> | first 10` closes consumption on a long source
- **THEN** the plugin stops local work for the remaining source rather than continuing to evaluate thousands of rows
- **AND** any read-ahead or already-submitted evaluations remain bounded by the invocation's work limits

#### Scenario: Downstream closes while HTTP is stalled

- **WHEN** downstream drops the output while all pending HTTP requests are waiting
- **THEN** cancellation occurs without needing an HTTP response to trigger another channel send

#### Scenario: Downstream closes during an upstream call

- **WHEN** downstream drops output while an external iterator remains in `next()`
- **THEN** plugin-controlled work stops promptly
- **AND** the producer exits without dispatching the returned row once that call finishes

### Requirement: Mandatory in-flight single-flight deduplication

For deduplication, a request SHALL count as admitted when its prepared row
enters the bounded scheduler input; reserving read-ahead credit alone does not
identify a request. Identical requests admitted before completion handling
SHALL join one evaluation and its retries regardless of cache eligibility.
Each row SHALL retain its bounded outcome. A completed failure SHALL reach
current waiters and SHALL NOT be cached.

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

#### Scenario: Queued duplicate at completion boundary

- **WHEN** a duplicate is already accepted into scheduler input as its first evaluation finishes, and the success is too large to cache or the evaluation fails
- **THEN** it receives the first evaluation's outcome without a second HTTP request
