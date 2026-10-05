# Spec Delta

## MODIFIED Requirements

### Requirement: Per-evaluation total deadline

Timeout SHALL bound one dispatched logical evaluation, including shared-slot
waiting, HTTP attempts, retry waits, body decoding, and response-contract
validation. It SHALL NOT impose one deadline on a whole table invocation.
Cancellation SHALL interrupt awaiting requests and waits without waiting for
timeout expiry.

#### Scenario: Multiple attempts share one deadline

- **WHEN** retry waits and HTTP attempts together exhaust the configured timeout
- **THEN** the evaluation terminates even if no individual attempt consumed the full timeout

#### Scenario: Long table invocation

- **WHEN** each row finishes within its evaluation deadline but the whole table takes longer than that duration
- **THEN** the table does not fail solely because of total invocation elapsed time

#### Scenario: Validation exceeds the remaining deadline

- **WHEN** a complete JSON response arrives before the deadline but contract validation finishes after it
- **THEN** the evaluation reports a timeout without returning that response or success metrics

#### Scenario: Deadline expires before the first slot

- **WHEN** another invocation occupies the shared capacity until a waiting evaluation's deadline expires
- **THEN** the waiting evaluation reports a timeout without sending an HTTP request or returning success metrics
- **AND** it does not acquire a slot later after its deadline

#### Scenario: A retry waits for capacity

- **WHEN** a retry waits for shared capacity after its backoff ends
- **THEN** the wait uses the original evaluation deadline rather than starting a new timeout
- **AND** this deadline behavior applies to both System One and model-list operations

## ADDED Requirements

### Requirement: Aggregate process HTTP attempt budget

All HTTP attempts in one plugin process SHALL share one concurrent attempt
limit, including retries from `jev ask`, `jev annotate`, and `jev models`,
regardless of API root, authorization, or proxy policy. This limit SHALL
count attempts rather than connections or whole command invocations.

#### Scenario: Several callers reach the common limit

- **WHEN** concurrent commands offer more independent requests than the configured limit and the server holds active responses
- **THEN** active attempts reach but do not exceed that limit
- **AND** remaining attempts begin as occupied slots become available

#### Scenario: Different clients do not get separate budgets

- **WHEN** concurrent calls select different API roots, synthetic keys, and proxy policies
- **THEN** their combined active HTTP attempts remain within the same process limit

### Requirement: Attempt-scoped capacity ownership

An actual HTTP attempt SHALL occupy exactly one shared slot from send
admission until its response body is consumed or the attempt terminates.
The slot SHALL be released before response decoding or validation, retry
waiting, or downstream output delivery, including on failure and cancellation.

#### Scenario: Response headers do not release capacity

- **WHEN** a successful response's headers arrive but its body remains incomplete
- **THEN** the attempt continues to occupy its slot until body consumption completes or the attempt terminates

#### Scenario: Retry backoff releases capacity

- **WHEN** a retryable response is abandoned and its invocation enters a retry delay
- **THEN** another invocation can use the released slot during that delay
- **AND** the retry must acquire one slot again before sending

#### Scenario: Response work does not hold network capacity

- **WHEN** a response body has been received but decoding or contract validation is still in progress
- **THEN** another invocation can acquire the released slot before that response work completes

#### Scenario: Every termination path releases its slot

- **WHEN** an admitted attempt succeeds, fails with an HTTP or transport error, times out, is cancelled, or rejects an oversized response body
- **THEN** its slot becomes available to another invocation without restarting the plugin

### Requirement: Cancellable asynchronous capacity waiting

Waiting for shared attempt capacity SHALL be asynchronous and interruptible
by local cancellation. Cancelling a waiter SHALL remove only that wait;
it SHALL NOT disable the shared budget or prevent other invocations from
progressing.

#### Scenario: Cancelled waiter never sends later

- **WHEN** an annotation output is dropped while its evaluation waits for shared capacity
- **THEN** that evaluation does not send a request when capacity subsequently becomes available
- **AND** a healthy invocation can still acquire the capacity and complete

#### Scenario: Waiting does not block unrelated runtime progress

- **WHEN** all attempt slots are occupied and additional evaluations await capacity
- **THEN** active response handling, timers, cancellation, and unrelated ready work continue to progress

### Requirement: Non-attempt work does not consume capacity

Offline commands, request previews, invocation-local cache hits, and waiters
sharing an existing in-flight evaluation SHALL NOT occupy additional HTTP
attempt slots. A new independent HTTP attempt SHALL require its own slot.

#### Scenario: Previews work with a full budget

- **WHEN** live requests occupy all shared slots and another invocation constructs questions or runs `--dry-run`
- **THEN** its offline result can complete without awaiting a network slot

#### Scenario: Duplicate rows share one admitted attempt

- **WHEN** duplicate annotation rows join the same invocation-local in-flight evaluation
- **THEN** only that evaluation's actual HTTP attempt occupies a slot
- **AND** later completed-cache hits occupy no HTTP slots

### Requirement: Slot waits preserve HTTP measurement boundaries

HTTP measurements SHALL start only after initial slot admission. `elapsed`
SHALL include later slot waits between attempts; `attempt_elapsed` SHALL
exclude the wait preceding the final successful attempt. `attempts` SHALL
count actual sends, not slot waits. The existing broader trace `duration_ms`
SHALL continue to include initial slot waiting.

#### Scenario: One attempt waits before admission

- **WHEN** a successful single-attempt operation first waits for shared capacity
- **THEN** `elapsed` and `attempt_elapsed` remain equal and exclude that initial wait
- **AND** `attempts` is one and the operation trace duration includes the wait

#### Scenario: Retry waits before reacquisition

- **WHEN** a retryable attempt is followed by backoff, a shared-slot wait, and a successful attempt
- **THEN** `elapsed` includes the earlier attempt, backoff, slot reacquisition wait, and final decoding and validation
- **AND** `attempt_elapsed` starts after the final slot is acquired and ends at the same validation completion instant
- **AND** `attempts` counts only the two actual sends and existing byte and protocol metrics retain their meanings
