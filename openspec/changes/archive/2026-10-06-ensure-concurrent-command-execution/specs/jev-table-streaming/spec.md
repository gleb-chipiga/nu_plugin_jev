# Spec Delta

## MODIFIED Requirements

### Requirement: Bounded outstanding work and input read-ahead

`--jobs N` SHALL bound outstanding unique evaluations, including retries and
shared-slot waits and completed tasks not yet joined, by `N`. Admission SHALL
retain at most `2N` row outcomes
not yet yielded or discarded, counting queued rows, duplicate waiters,
ordered completed outcomes, and queued output. Input SHALL stream rather
than be fully collected. The completed cache and upstream Nu protocol
buffering SHALL be separate from this work bound.

#### Scenario: Slow first row in ordered mode

- **WHEN** the first evaluation is stalled while subsequent evaluations finish on a long input
- **THEN** outstanding row outcomes and input admission remain within the stated bound
- **AND** completed later rows do not permit an unbounded reorder backlog

#### Scenario: Many duplicates await one request

- **WHEN** a long input repeats a state whose shared evaluation is stalled
- **THEN** duplicate waiters consume the same bounded row admission budget
- **AND** the plugin does not read the whole input into an unbounded waiter list

#### Scenario: Shared network capacity is occupied

- **WHEN** a long annotation input has evaluations waiting behind other invocations' HTTP attempts
- **THEN** these waiting evaluations count toward its `--jobs` bound
- **AND** its retained row outcomes and read-ahead remain within the `2N` admission bound

### Requirement: Invocation-local cache lifetime

In-flight state and completed results SHALL belong exclusively to one table
invocation and SHALL be released on completion, terminal failure, or output
drop. Results SHALL NOT be reused across invocations, including for a moving
alias such as `jev-latest`. No process-global or persistent cache SHALL be used.

#### Scenario: Alias reused by another command invocation

- **WHEN** a later table invocation submits a request identical to one previously completed with `jev-latest`
- **THEN** it performs a fresh evaluation instead of reusing the previous invocation's result

#### Scenario: Identical requests in concurrent invocations

- **WHEN** two concurrent annotations request identical states, questions, and model using the same API root
- **THEN** each invocation performs its own evaluation rather than joining the other's request or completed cache
- **AND** duplicate rows within either invocation still share only that invocation's evaluation

## ADDED Requirements

### Requirement: Independent table work and process attempt limits

The invocation-local `--jobs` limit SHALL remain independent of the shared
HTTP attempt limit, with its unchanged default of `16`. A jobs value larger
than the process limit SHALL remain valid without increasing the shared
attempt capacity.

#### Scenario: Default jobs is not raised to the process default

- **WHEN** an annotation runs without a jobs override and the process attempt limit defaults to 128
- **THEN** it admits at most 16 outstanding unique evaluations rather than 128

#### Scenario: Explicit jobs can exceed shared capacity

- **WHEN** `jev annotate --jobs 8` runs in a process with an attempt limit of two
- **THEN** it retains its logical work bound of eight and row admission bound of sixteen
- **AND** no more than two HTTP attempts from all invocations occupy shared slots simultaneously

### Requirement: Active evaluations progress under output backpressure

Dispatched evaluations in an active annotation SHALL continue HTTP, retries, and deadline
handling independently of a full, open output channel. Their completion or
deadline SHALL release shared attempt capacity without requiring downstream
to resume reading or drop that output.

#### Scenario: Completed bodies unblock a neighbor

- **WHEN** an annotation fills its open output and occupies all process slots, then the server releases its remaining response bodies
- **THEN** a neighboring invocation can acquire capacity and finish without any further consumption of the annotation output

#### Scenario: Deadline unblocks a neighbor

- **WHEN** an annotation fills its open output and occupies all process slots with unfinished attempts whose deadlines expire
- **THEN** the attempts release capacity and a neighboring invocation can finish while the annotation output remains unread and open

#### Scenario: Retries continue with a stalled consumer

- **WHEN** an annotation output is full and an already-dispatched evaluation receives a retryable response
- **THEN** its retry delay, subsequent attempt, and original deadline continue to be serviced without output consumption

#### Scenario: Joining results preserves row admission

- **WHEN** a slow first row delays ordered output while subsequent task results are joined
- **THEN** their rows still consume admission credit and input read-ahead remains at most twice jobs

### Requirement: Evaluation tasks follow stream lifetime

Annotation SHALL cancel and clean up its remaining evaluation tasks on
interrupt, terminal failure, or output drop. Returning a stream from `run`
SHALL NOT end its protected lifetime. Waiting for a full output SHALL remain
cancellable and SHALL NOT detach tasks or retain process permits after cleanup.

#### Scenario: Cancel with full open output

- **WHEN** local cancellation occurs with full open annotation output and active or queued evaluation tasks
- **THEN** the supervisor terminates, its remaining tasks are stopped, and their process slots are available to other invocations

#### Scenario: Normal completion cleans up

- **WHEN** all annotation rows have been consumed and the returned stream ends normally
- **THEN** no evaluation task or invocation-local routing state remains active

### Requirement: Terminal cleanup precedes error delivery

After observing a terminal row failure, annotation SHALL stop admission and
new dispatch, cancel and join remaining evaluation tasks, and release local
cache and routing state before waiting for output capacity to deliver the
original error. This cleanup SHALL NOT depend on downstream consumption or
the return of an external synchronous input call.

#### Scenario: Observed failure with full open output

- **WHEN** output is full and open and a terminal failure is observed with siblings in an HTTP attempt, shared-slot wait, or retry delay
- **THEN** those siblings are cleaned up and their process slots become available without consuming any output
- **AND** a neighboring invocation can finish while the terminal error still awaits delivery

#### Scenario: Original cause survives worker cleanup

- **WHEN** downstream resumes reading after terminal cleanup waited for output capacity
- **THEN** it receives the saved original failure rather than a cancellation error
- **AND** no success from discarded work follows that error

#### Scenario: Output closes before the error is read

- **WHEN** downstream drops the output after cleanup but before consuming the terminal error
- **THEN** error delivery stops without restarting work or retaining an HTTP permit

#### Scenario: Interrupt while error delivery waits

- **WHEN** Nu interrupts after terminal cleanup while error delivery waits on a full output
- **THEN** that delivery wait ends without waiting for downstream consumption

### Requirement: Synchronous input does not gate local termination

Local termination SHALL NOT wait for an external synchronous `input.next()`.
A producer blocked in that call SHALL hold no HTTP permit or shared mutex
and SHALL discard a returned row after local termination, without submitting
another request.

#### Scenario: Failed annotation has a blocked source

- **WHEN** an annotation observes a terminal failure while its producer is blocked in an external input call
- **THEN** HTTP cleanup and error delivery do not wait for that call
- **AND** the producer's thread and source-related resources may remain until the call returns, after which the returned row is discarded
