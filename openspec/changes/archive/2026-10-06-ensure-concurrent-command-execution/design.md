# Design

## Context

See `proposal.md` for motivation and the three spec deltas for behavior.

`JevPlugin` already owns a shared multi-thread Tokio runtime and a client pool.
The pinned `nu-plugin 0.116.0` can dispatch concurrent command handlers;
blocking a synchronous handler on async work does not require serializing
other handlers. Live overlap through actual Nu is not currently covered.

`JevClient::send` is the common attempt loop for System One and models.
The pool shares an automatic-proxy client and caches alternate-policy clients
under a short-lived mutex, without locking across HTTP work. Operation
deadlines already cover retries, decoding, and contract validation.

Table calls have their own scheduler, bounded queues, cancellation, dedup,
and completed LRU. Their active logical evaluations are bounded by `--jobs`,
while row admission is bounded separately by twice that value. Invocation
settings are resolved on the synchronous caller side and frozen for that call.

## Goals / Non-Goals

**Goals:** Add network admission at the common attempt boundary and prove
native command overlap while preserving the runtime and bounded table bridge.
Keep process policy separate from request configuration, and make permit
cleanup automatic when existing cancellation or deadline paths drop work.

**Non-Goals:** Cross-invocation caches, command scheduling, provider quota
tracking, rate limiting, dynamic resizing, per-client limits, or CPU limiting.
The budget does not bound the number of callers, waiters, or remote operations
already accepted by the service. Response schemas and protocol modes stay as
implemented; no command, flag, or dependency is added.

## Decisions

### 1. Resolve the process budget once at startup

Resolve the limit before constructing shared client state or serving commands:
startup `NU_PLUGIN_JEV_MAX_IN_FLIGHT`, local NUON `max_in_flight`, user NUON
`max_in_flight`, then 128. Select the startup local file using process
`NU_PLUGIN_JEV_CONFIG`, relative to startup Nu `PWD`, or `.nu_plugin_jev.nuon`
in that directory. Nu deliberately spawns plugins beside their executable,
so use absolute startup process `PWD`, falling back to OS cwd only when it is
absent or relative. Use the startup platform user config directory, honoring
absolute `XDG_CONFIG_HOME`. An explicit selected file must exist; implicit files are
optional. Reuse the bounded, data-only NUON loader and its field safeguards.

Accept a positive native integer in NUON or a positive base-10 environment
string, bounded by `tokio::sync::Semaphore::MAX_PERMITS`. Validate only the
selected value without falling back after an error. A higher-priority value
avoids loading lower-priority files just for this setting. Startup errors
identify the setting or file layer without exposing values or parser details.
Parsed file credentials are not retained in shared process state.

Keep this process setting outside `InvocationConfig` and request precedence.
The NUON loader recognizes `max_in_flight`, but later invocation file reads
do not apply it. Caller environment, Nu plugin config, `--config`, file edits,
key, API root, proxy, or jobs changes cannot resize the budget. Command flags
and Nu plugin config are not startup sources. Keep `--jobs` at 16 by default;
128 is the independently chosen process ceiling, not a measured optimal jobs
value or a provider guarantee.

Tests should inject capacities or parse supplied values without mutating
process environment shared by parallel tests. Startup subprocess tests can
set their child environment explicitly.

**Alternatives:** Per-invocation resolution makes a shared limit dependent
on call order; accepting the first invocation's value has the same flaw.
Reading NUON on each command would incorrectly turn process policy into a
caller override. Dynamic semaphore resizing adds lifecycle complexity
unrelated to this change.

### 2. Share one semaphore through every reusable client

Create one `Arc<tokio::sync::Semaphore>` for the plugin instance and propagate
it through `JevClientPool` to every `JevClient`, including clients created
after alternate-policy cache eviction. Cloned clients share the same budget.
Use the common `send` loop so both GET models and POST evaluations, including
every retry, receive identical admission behavior.

Retain existing connection pooling, short client-cache locks, and parallel
handlers. Never hold a mutex around the command or HTTP future, reserve a
command's whole jobs allocation, or close the semaphore to cancel one caller.

**Alternatives:** A budget per client, endpoint, key, or invocation would
multiply capacity when settings change. A dedicated command dispatcher is
unnecessary because Nu already supplies concurrent handlers. No additional
HTTP wrapper or client library is needed.

### 3. Scope one permit to sending and body acquisition

Each iteration asynchronously acquires one permit inside the existing
cancellation/deadline-wrapped operation. It then timestamps and records the
actual send, submits the request, and holds the permit through bounded body
acquisition. Scope the permit so it is dropped before decoding or contract
validation, including existing offloaded response work.

For a rejected response, inspect only the existing status and retry guidance,
then drop the response and permit before awaiting backoff. A retry reacquires
capacity. Error, body-size rejection, timeout, and local cancellation all
release an acquired permit through RAII. No permit exists for input
preparation, dry run, constructors, completed-cache hits, duplicate waiters,
or downstream output/backpressure.

The existing deadline wrapper can cancel capacity acquisition by dropping
its future; reuse it rather than creating a second timeout system. Slot wait
time counts against the dispatched logical operation's deadline, not one
deadline for the whole annotation. No blocking acquisition belongs on a
Tokio worker. Engine interrupt remains plugin-wide; local output drop or
timeout must not be implemented by triggering that shared interrupt.

**Alternatives:** Holding a slot across retries wastes capacity during
backoff. Releasing at response headers undercounts still-active bodies.
Holding until decode or row delivery limits non-network work and can make
slow consumers occupy the entire network budget. Ignoring admission waits
in the deadline could leave callers queued indefinitely.

### 4. Preserve measurement meanings

Capture `first_started` after the first slot acquisition, immediately before
the actual send, and final `attempt_started` after the final acquisition.
Keep their common completion instant after decoding and contract validation.
Thus initial admission waiting is outside both HTTP durations; retry delays
and later slot waits are inside `elapsed`, not `attempt_elapsed`. Single-send
success still has equal durations. Count only actual sends as attempts.

The existing broader operation trace `duration_ms` includes initial slot
waiting. Preserve trace/returned nanosecond equality, body-byte accounting,
protocol reporting, token fields, and metadata. Do not add queue-duration
fields or change command response formats.

**Alternative:** Starting HTTP measurements before initial admission would
change their send-based meaning and conflate local contention with network
time. Reporting only the last network phase would omit existing decoding and
validation timing.

### 5. Keep table admission and caching invocation-local

A logical evaluation waiting for a network slot remains in the table's local
task set and consumes its existing jobs position. Do not derive the input
window or queue capacity from the process limit, or clamp a valid jobs value
to it. Existing `2N` admission credits cover queued and completed rows even
when other callers occupy all network capacity.

Sharing the semaphore does not share dedup groups or LRU entries. Two
invocations may issue identical requests independently; within one invocation
duplicate waiters still need only their evaluation's one HTTP attempt slot.

**Alternative:** Process-wide dedup changes cancellation ownership and cache
isolation, requiring a separate design. It is expressly outside this change.

### 6. Decouple dispatched HTTP work from output backpressure

Use one invocation-local `JoinSet` for complete logical evaluations, including
the existing deadline wrapper, admission waits, retries, response decoding,
and validation. Unlike raw futures inside `FuturesUnordered`, spawned tasks
keep running when the supervisor awaits a full output channel. Keep permit
release inside the unchanged HTTP client; task results must not carry network
permits into output or reorder buffers.

Keep at most `jobs` tasks in the set, counting completed tasks whose results
have not yet been joined. Independently retain admission credit for each row
until its outcome is yielded or discarded, including duplicate waiters,
pending groups, completed task results, reorder storage, and output queues.
Joining a result does not return row credit. Neither bound depends on shared
HTTP capacity, and a slow consumer cannot expand the `2 * jobs` row window.

The supervisor owns the task set for the returned stream's lifetime. Output
drop or interrupt must wake a backpressured send, cancel local work, abort
and join remaining tasks, and release invocation-local state. Natural end or
terminal failure also cleans up tasks. Dropping the set is an abort safeguard;
bare detached `JoinHandle`s are not acceptable. No Tokio worker blocks while
waiting for input, output capacity, HTTP, or task cleanup.

Test a full but open output with every process permit occupied by the same
annotation. Server-released bodies and, separately, existing deadlines must
free permits so a neighbor succeeds without consuming or dropping that
output. Verify both work bounds, ordered and unordered output, retry progress,
and cancellation cleanup, using fixture gates rather than arbitrary sleeps.

**Alternative:** Continuing to poll raw futures during output admission would
require another bounded event-routing layer. Raising the process limit only
hides the cross-invocation stall and cannot fix it.

### 7. Register interrupt handling before checking signal state

Use a shared Nu-side helper for live ask, models, and annotation. It creates
local cancellation, registers an Interrupt-only handler, then checks current
engine signals before work starts. The post-registration check observes a
signal arriving before or during registration; later signals reach the
handler. Reset never revives cancelled work.

Retain the registration guard through the whole protected operation. An
annotation transfers it into its returned output stream rather than dropping
it when `run` returns. Test interrupt boundaries with controlled registration
and check hooks, including guard lifetime and Reset, without changing shared
engine interrupt semantics or introducing per-command signal state.

### 8. Prove behavior with gated real Nu calls

Reuse `tests/support/h2_fixture.rs` and its Axum server, extending controlled
response gates or body streaming only where needed. Preserve default HTTP/2
prior-knowledge coverage and fallback-build protocol coverage. No bespoke
protocol server, external API, real credentials, or timing benchmark is needed.

The principal test launches one actual Nu process with one plugin instance:
two distinct single-row annotations, each with `--jobs 1`, execute through
`par-each --threads 2` and fully consume their results within their branches.
Use process capacity two or higher. The server releases responses only after
receiving both requests; a watchdog fails clearly if commands serialize.
Add live concurrent ask calls and overlapping mixed command families.

Use smaller injected budgets for deterministic saturation checks. Verify
both reaching the limit and never exceeding it, including different client
policies and roots. Gate body completion to prove headers do not free a slot;
gate response work separately to prove decoding does. Exercise retry backoff,
timeout before admission, cancellation of an active or queued operation, and
error cleanup, always showing that a healthy neighbor can finish afterward.

Verify caller data and authorization with synthetic keys. Catch expected
errors inside their Nu branches so global pipeline teardown cannot fake a
local-isolation result. Output-drop tests should use the existing stream
adapter when that gives deterministic local cancellation. Compare event order
and observed overlap, not narrow sleep-based latency thresholds.

**Alternatives:** Direct client tests alone cannot detect handler serialization
or caller-environment mixing in Nu. Separate plugin subprocesses prove neither
shared capacity nor same-instance overlap. Counting TCP connections is wrong
for HTTP/2 multiplexing, and remote handlers surviving client cancellation
must not be mistaken for still-owned local attempt slots.

### 9. Clean up observed terminal failures before delivering their error

Keep the first observed terminal outcome separate from ordinary output. Once
observed, stop row admission and new dispatch, close the local input channel,
abort and join the invocation's remaining evaluation tasks, and release its
cache and routing state before awaiting output capacity for that saved error.
Do not drain queued input before handling an already observed terminal result.
Apply the same path to unconditional destination collisions.

Reuse ownership boundaries instead of adding another cancellation token:
closing the invocation-local admission semaphore stops its producer, closing
input prevents later sends, and `JoinSet::shutdown` cancels and joins HTTP
tasks. Never close the process-wide HTTP semaphore. Keep stream-lifetime
cancellation live for the saved error's send; external interruption or output
closure can still end that wait without replacing the original cause with a
worker-cancellation error. The stream's handler guard remains retained.

The existing contract says first **observed** failure, not first completed
task. A failure may remain unjoined while the supervisor awaits an earlier
successful send. This correction changes cleanup after observation, without
adding a new result-polling layer or a stronger discovery guarantee.

Gate a terminal response until output is full and siblings occupy an HTTP
slot, a shared-slot wait, and retry backoff. Before consuming any output,
verify cleanup and a healthy neighbor's progress. Then drain the output and
verify the saved original error; separately drop output or interrupt while
error delivery waits. Use cleanup and retry-deadline boundaries to prove no
additional retry starts, not a global ordering claim about remote arrivals.

An external `input.next()` is not joinable on this cleanup path. Its producer
owns no HTTP permit or shared mutex and checks local admission closure after
the call returns, discarding the row. It may retain its OS thread, the source,
one local row credit, and request-building data until return; repeated stuck
sources can accumulate those resources. Do not move this limitation to Tokio
workers or claim an overall process-memory ceiling.

## Risks / Trade-offs

- **Local contention can exhaust a deadline before sending.** Keep the
  existing operation deadline and document that concurrent callers share a
  startup budget; HTTP durations are not the whole invocation latency.
- **A large number of parallel callers can still allocate waiters.** Keep
  table-local bounds; this is not a process-wide memory or command-count cap.
- **Local cancellation cannot undo service-side work already accepted.**
  Test local permit reclamation, not a guarantee of remote task cancellation.
- **Response work can overlap new HTTP attempts after release.** Preserve
  existing bounded response sizes and offloading; do not add a CPU limiter.
- **A restarted or second plugin process has a separate budget.** Document
  process scope rather than claiming a provider-account quota.
- **Gated tests can hang when behavior regresses.** Apply bounded server waits
  and child-process watchdogs, and always clean up fixture resources.
- **A synchronous source can remain blocked indefinitely.** HTTP cleanup and
  error delivery must not join its producer; retained source threads and their
  input-related resources are a residual risk, not a proved memory leak.

## Migration Plan

No response, existing configuration-file, or command syntax migration is required.
Existing users retain `--jobs 16`; aggregate traffic now shares 128 attempt
slots by default. Users can set `max_in_flight` in local or user NUON, or
override it with startup `NU_PLUGIN_JEV_MAX_IN_FLIGHT`. Restart the plugin to
apply changes. Document startup file selection separately from dynamic caller
configuration; a later command's directory or `--config` does not resize it.

Update README, help where applicable, the unreleased changelog, and the usage
skill during implementation. The skill needs only actionable guidance on
native parallel calls and the two limits, not semaphore or cancellation
mechanics. Reverting this implementation restores the previous absence of an
aggregate cap without changing persisted data or response schemas.
