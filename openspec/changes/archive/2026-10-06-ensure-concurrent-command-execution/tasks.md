# Tasks

## 1. Startup process budget

- [x] 1.1 Add startup-only parsing of `NU_PLUGIN_JEV_MAX_IN_FLIGHT` with
  default 128 and supported positive capacity validation. Verify absent,
  valid, empty, zero, negative, fractional, nonnumeric, overflow, and
  platform-supported non-Unicode cases with unit and startup subprocess
  tests; invalid diagnostics name the setting without panic or raw-value echo.
- [x] 1.2 Construct one shared semaphore before client-pool initialization and
  propagate it to every client without changing invocation configuration or
  dependencies. Verify that automatic and alternate-policy clients, clones,
  and clients recreated after pool eviction share the same budget.
- [x] 1.3 Document startup environment/local NUON/user NUON precedence,
  startup file selection, process scope, default 128,
  and restart requirement briefly in README and the usage skill. Verify these
  instructions do not imply per-command or Nu config overrides and
  retain the existing `--jobs` default of 16.
- [x] 1.4 Reuse the bounded NUON loader to resolve `max_in_flight` before
  command servicing. Test partial files, startup local-file selection,
  environment/local/user/default precedence, native integer validation,
  missing explicit files, redacted malformed files, and selected-value errors
  without lower-layer fallback. Keep this setting out of invocation config.
- [x] 1.5 Align existing file-key and offline-command contracts with startup
  validation; test that later malformed NUON does not affect root guidance
  or question constructors in an already running process.

## 2. HTTP attempt admission

- [x] 2.1 Acquire one slot asynchronously for each actual send in the common
  System One/models attempt loop and release it before decoding, validation,
  backoff, or output delivery. Use the existing Axum fixture to verify that
  incomplete bodies hold slots, response work does not, and another caller
  progresses during retry backoff while retries reacquire capacity.
- [x] 2.2 Include slot acquisition in existing local cancellation and total
  deadlines without closing the shared semaphore or adding another timeout.
  Verify pre-admission timeout sends nothing, cancelled waiters never send
  later, retry acquisition retains the original deadline, and healthy callers
  continue; cover both evaluations and model-list operations.
- [x] 2.3 Cover permit reclamation after success, HTTP failure, transport
  failure, oversized body rejection, active-attempt timeout, and cancellation.
  For each case, verify a subsequent independent request can use the full
  configured capacity without a plugin restart.
- [x] 2.4 Preserve measurement boundaries by timestamping only admitted sends.
  Verify initial waits are outside equal single-attempt HTTP durations, later
  slot waits are inside `elapsed` but outside final `attempt_elapsed`, only
  sends increment `attempts`, and trace/returned nanoseconds, bytes, protocol,
  metadata, and token fields remain consistent without schema additions.
- [x] 2.5 Clarify in README that the operation deadline includes capacity
  waiting while HTTP durations begin at sending, and update actionable usage
  guidance on timeout interpretation. Verify the descriptions match gated
  measurement tests and add no semaphore mechanics to the usage skill.

## 3. Table bounds and local cancellation

- [x] 3.1 Preserve table-local logical work and admission budgets while HTTP
  capacity is occupied by other callers. Verify jobs 8 with process capacity
  2 remains valid, waiting evaluations count toward jobs, ordered/unordered
  output stays bounded by the existing `2N` window, and default jobs stays 16.
- [x] 3.2 Verify duplicate rows within one invocation use one admitted attempt,
  completed-cache hits require no network slot, and identical concurrent
  invocations evaluate independently. Keep the existing local LRU lifetime
  and bounds unchanged; do not add cross-invocation state.
- [x] 3.3 Extend stream-adapter tests for output drop while waiting for a slot
  and during an admitted attempt. Verify no later rows or queued requests
  escape local cancellation, and a neighboring invocation still completes.
- [x] 3.4 Check README, help, and usage-skill table guidance against the
  independent jobs/process-limit tests; correct any conflicting wording
  without adding implementation details or a new flag.
- [x] 3.5 Replace supervisor-polled HTTP futures with an invocation-local
  JoinSet of full deadline-wrapped evaluations. Count completed unjoined
  tasks toward jobs, retain row admission credits until consumption/discard,
  and cancel, abort, and join remaining tasks on stream termination. Keep
  HTTP-client permit release, response formats, defaults, and caches unchanged.
- [x] 3.6 Add gated full-but-open output regressions with every process slot
  occupied. Verify a neighbor progresses without consuming or dropping that
  output after body completion and deadline expiry, retry progress continues,
  cancellation cleans up tasks, and jobs plus the 2N row window remain bounded.
- [x] 3.7 Preserve an observed terminal outcome, stop local admission, and
  abort and join sibling evaluations before waiting to deliver its original
  error. Keep stream cancellation independent of worker cleanup, discard
  queued work, and retain the first-observed failure contract without a new
  result-discovery guarantee or HTTP-client change.
- [x] 3.8 Add gated terminal-failure regressions with full open output and
  siblings in an admitted attempt, capacity wait, and retry backoff. Verify
  cleanup and healthy-neighbor progress before output consumption, original
  error delivery afterward, and interruption/output-drop during delivery.
- [x] 3.9 Verify terminal cleanup does not wait for external input.next();
  after release the producer discards the row without another request. State
  its possible indefinite thread/source retention precisely in the technical
  contract and keep implementation mechanics out of the usage skill.

## 4. Native Nu concurrency and isolation

- [x] 4.1 Add the principal same-process Nu test using `par-each --threads 2`,
  distinct one-row `annotate --jobs 1` calls, capacity at least two, and fully
  consumed branch results. Gate server replies on both arrivals and use a
  protective watchdog so accidental serialization fails without hanging.
- [x] 4.2 Add live same-process concurrent ask calls and mixed
  ask/annotate/models calls using the existing HTTP fixture. Verify request
  overlap, correct per-caller results, and aggregate saturation at, but never
  above, a small shared limit across client policies and API roots.
- [x] 4.3 Verify concurrent caller configuration isolation with synthetic keys,
  models, states, questions, and context. Change caller environment/config
  and edit a startup-selected NUON file after startup; verify the process
  budget remains fixed, including with different caller directories and
  command `--config` files, rather than depending on the first invocation.
- [x] 4.4 Catch expected errors inside their Nu branches and verify local
  failures or timeouts do not stop healthy branches. While live traffic fills
  the budget, verify question constructors and dry runs still complete without
  network admission or unintended plugin-wide interruption.
- [x] 4.5 Add a concise native Nu parallel-call example and explain the two
  independent limits in README and the usage skill; record the implemented
  behavior in the unreleased changelog. Verify examples use existing commands,
  consume branch results, and introduce neither `--parallel` nor shared caches.
- [x] 4.6 Share a register-then-check interrupt helper across ask, models,
  and annotation. Preserve the guard for the protected operation or stream
  lifetime and verify interrupt-before/during/after registration, Reset,
  failure cleanup, and guard drop through deterministic synchronization.

## 5. Integration validation

- [x] 5.1 Run `cargo fmt`, `cargo clippy --all-targets --all-features`, and
  `cargo nextest run --all-features --all-targets --locked`; fix all findings.
  Retain the existing Cargo home and target directory, waiting for any lock
  rather than starting a separate build directory. Isolate native test children
  from real caller settings, config files, and credentials.
- [x] 5.2 Run `cargo nextest run --no-default-features --all-targets --locked`
  and repeat the gated concurrency/isolation tests to check scheduling
  robustness. Verify default HTTP/2 and existing fallback protocol coverage
  without external services or real API keys.
- [x] 5.3 Run `openspec validate --all --strict --no-interactive` and repository
  hooks for changed files. Verify no findings, including INFO notices, for
  affected specs and confirm every scenario is covered before completing tasks.
