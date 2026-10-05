//! Schedules independent row requests with bounded admission and invocation-local reuse.
//! Independent tasks keep HTTP progressing under output stalls; row credit bounds read-ahead.
//! Terminal cleanup precedes error delivery and does not wait for a blocked external source.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    future::pending,
    sync::Arc,
    thread,
};

use futures::FutureExt;
use nu_protocol::{HandlerGuard, Value};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::{Id, JoinError, JoinSet},
};

use crate::{
    api::{
        cancel::{CancelHandle, CancelSignal},
        client::{JevClient, MeasuredSuccess, PreparedRequest},
        types::{SystemOneRequest, SystemOneResponse},
    },
    config::{ApiKey, InvocationConfig},
    error::JevError,
};

use super::cache::{CompletedCache, RequestKey, SharedResponse, next_request_id};

/// Builds one outbound request from a source row on the dedicated input thread.
pub(crate) type RowBuilder = Box<dyn Fn(&Value) -> Result<SystemOneRequest, JevError> + Send>;

/// Captures all immutable evaluation settings before table input is consumed.
pub(crate) struct StreamSetup {
    /// Shared HTTP connection pool and process attempt budget for this invocation.
    pub(crate) client: JevClient,
    /// Validated caller-scoped model, transport, and scheduling settings.
    pub(crate) config: InvocationConfig,
    /// Caller credential retained only for this live invocation.
    pub(crate) key: ApiKey,
    /// Emits whichever completed row is ready without sequence buffering.
    pub(crate) unordered: bool,
    /// Stops on the first row failure instead of returning error outcomes.
    pub(crate) fail_fast: bool,
    /// Reports task admission and output stalls for deterministic scheduler tests.
    #[cfg(test)]
    pub(crate) observer: Option<mpsc::UnboundedSender<SupervisorEvent>>,
}

/// Exposes controlled scheduler boundaries without changing production execution.
#[cfg(test)]
pub(crate) enum SupervisorEvent {
    /// Counts tasks retained by the set, including unjoined completions.
    TaskCount(usize),
    /// Marks a ready outcome about to wait for a full, open output channel.
    OutputBlocked,
    /// Confirms local admission is closed and evaluation tasks and routing are cleaned up.
    WorkStopped,
    /// Confirms evaluation cleanup and output sender release at supervisor termination.
    Stopped,
}

/// Owns a source row, its result, and admission credit until consumed or discarded.
/// Credit bounds this bridge, not upstream Nu protocol buffers or completed-cache storage.
pub(crate) struct RowOutcome {
    /// Input sequence number, independent of completion order.
    pub(crate) sequence: u64,
    /// Original row preserved for annotation or error policies.
    pub(crate) source: Value,
    /// Shared successful result or classified row failure.
    pub(crate) result: Result<Arc<SharedResponse>, Arc<JevError>>,
    /// Local row credit, not HTTP capacity; retained through reordering and output buffering.
    _permit: OwnedSemaphorePermit,
}

/// Receives bounded outcomes and cancels the invocation when dropped early.
pub(crate) struct ScheduledOutput {
    receiver: mpsc::Receiver<RowOutcome>,
    cancel: CancelHandle,
    /// Retains interrupt protection after the command returns its lazy stream.
    _handler: Option<HandlerGuard>,
    /// Keeps the async bridge alive until the returned stream is consumed or dropped.
    _runtime: Arc<tokio::runtime::Runtime>,
}

impl Iterator for ScheduledOutput {
    type Item = RowOutcome;

    /// Waits on the plugin protocol thread, never on a Tokio runtime worker.
    fn next(&mut self) -> Option<Self::Item> {
        self.receiver.blocking_recv()
    }
}

impl Drop for ScheduledOutput {
    /// Wakes the supervisor to cancel local work without interrupting neighboring invocations.
    fn drop(&mut self) {
        self.cancel.cancel();
        self.receiver.close();
    }
}

/// Carries one admitted row from the synchronous producer to the supervisor.
struct InputRow {
    sequence: u64,
    source: Value,
    request: Option<Result<(PreparedRequest, RequestKey), JevError>>,
    permit: OwnedSemaphorePermit,
}

impl InputRow {
    /// Converts a prepared row into an outcome while retaining its admission credit.
    fn finish(self, result: Result<Arc<SharedResponse>, Arc<JevError>>) -> RowOutcome {
        RowOutcome {
            sequence: self.sequence,
            source: self.source,
            result,
            _permit: self.permit,
        }
    }
}

/// Groups admitted duplicate rows awaiting one not-yet-dispatched request.
struct PendingGroup {
    key: RequestKey,
    request: PreparedRequest,
    rows: Vec<InputRow>,
}

/// Owns invocation-local row routing, duplicate groups, and ordered outcomes.
struct SupervisorState {
    /// Separate success LRU with approximate byte accounting, not a process-RSS ceiling.
    /// Eviction or oversized bypass permits a fresh request for a previously evaluated state.
    cache: CompletedCache,
    pending_groups: VecDeque<PendingGroup>,
    in_flight: HashMap<RequestKey, Vec<InputRow>>,
    ordered: BTreeMap<u64, RowOutcome>,
    next_sequence: u64,
}

/// Identifies one completed logical HTTP evaluation and its shared local identity.
type Completion = (
    RequestKey,
    String,
    Result<MeasuredSuccess<SystemOneResponse>, JevError>,
);

/// Returns a validated response or classified failure from one independent evaluation task.
type EvaluationResult = Result<MeasuredSuccess<SystemOneResponse>, JevError>;

/// Identifies a finished task even when it panicked before returning its evaluation result.
type TaskOutcome = Result<(Id, EvaluationResult), JoinError>;

/// Starts a bounded row stream without reading its first input value on the caller thread.
/// The output owns interrupt protection; a blocked external source may outlive its drop.
pub(crate) fn start(
    runtime: Arc<tokio::runtime::Runtime>,
    setup: StreamSetup,
    input: Box<dyn Iterator<Item = Value> + Send>,
    build: RowBuilder,
    handler: Option<HandlerGuard>,
    cancellation: (CancelHandle, CancelSignal),
) -> Result<ScheduledOutput, JevError> {
    let jobs = setup
        .config
        .jobs
        .expect("table settings include jobs")
        .get();
    // Queue capacities alone do not bound duplicate waiters or ordered results.
    // One credit follows each row across all stages; HTTP completion does not return it.
    let window = jobs * 2;
    let admission = Arc::new(Semaphore::new(window));
    // Each channel holds at most jobs rows, but their combined read-ahead also includes
    // active, duplicate, and reordered rows; the shared row credits enforce the total 2N.
    let (input_sender, input_receiver) = mpsc::channel(jobs);
    let (output_sender, output_receiver) = mpsc::channel(jobs);
    let (cancel, signal) = cancellation;
    let producer_runtime = Arc::clone(&runtime);
    let producer_signal = signal.clone();
    let producer_admission = Arc::clone(&admission);
    let service_root: Arc<str> = Arc::from(setup.config.base_url.as_str());
    // Arbitrary synchronous next() calls cannot be aborted. A dedicated thread avoids
    // blocking Tokio workers or making runtime shutdown wait for a stuck spawn_blocking call.
    thread::Builder::new()
        .name("jev-row-input".into())
        .spawn(move || {
            produce(
                input,
                build,
                producer_runtime,
                producer_admission,
                input_sender,
                producer_signal,
                service_root,
            );
        })
        .map_err(|_| JevError::Transport("cannot start Jev row producer"))?;
    let supervisor_cancel = cancel.clone();
    runtime.spawn(async move {
        supervise(
            setup,
            input_receiver,
            output_sender,
            signal,
            supervisor_cancel,
            admission,
        )
        .await;
    });
    Ok(ScheduledOutput {
        receiver: output_receiver,
        cancel,
        _handler: handler,
        _runtime: runtime,
    })
}

/// Reads and prepares admitted rows on a dedicated thread, never on Tokio workers.
/// A stuck next() retains its source, builder, runtime, and one local credit, not HTTP capacity.
fn produce(
    mut input: Box<dyn Iterator<Item = Value> + Send>,
    build: RowBuilder,
    runtime: Arc<tokio::runtime::Runtime>,
    admission: Arc<Semaphore>,
    sender: mpsc::Sender<InputRow>,
    mut cancel: CancelSignal,
    service_root: Arc<str>,
) {
    let mut sequence = 0_u64;
    loop {
        // Waiting blocks only this dedicated producer; the semaphore and cancellation
        // are awaited asynchronously, so the shared runtime remains free to drive HTTP.
        let permit = runtime.block_on(async {
            let acquired = Arc::clone(&admission).acquire_owned();
            let cancelled = cancel.cancelled();
            futures::pin_mut!(acquired, cancelled);
            match futures::future::select(cancelled, acquired).await {
                futures::future::Either::Left(((), _)) => None,
                futures::future::Either::Right((Ok(permit), _)) => Some(permit),
                futures::future::Either::Right((Err(_), _)) => None,
            }
        });
        let Some(permit) = permit else { break };
        if cancel.is_cancelled() || admission.is_closed() {
            break;
        }
        let Some(source) = input.next() else { break };
        // next() may return after worker cleanup but before saved-error delivery finishes.
        // Admission closure, unlike stream cancellation, already forbids preparing this row.
        if cancel.is_cancelled() || admission.is_closed() {
            break;
        }
        let request = build(&source).and_then(|wire| {
            let prepared = PreparedRequest::new(wire)?;
            let key = RequestKey::from_body(Arc::clone(&service_root), prepared.body.clone());
            Ok((prepared, key))
        });
        let row = InputRow {
            sequence,
            source,
            request: Some(request),
            permit,
        };
        // Request preparation can race with termination too. A closed input receiver
        // rejects any remaining send; the next iteration cannot acquire fresh admission.
        if cancel.is_cancelled() || admission.is_closed() {
            break;
        }
        let sent = runtime.block_on(async {
            let cancelled = cancel.cancelled().fuse();
            let send = sender.send(row).fuse();
            futures::pin_mut!(cancelled, send);
            futures::select_biased! {
                _ = cancelled => false,
                result = send => result.is_ok(),
            }
        });
        if !sent {
            break;
        }
        sequence = sequence.wrapping_add(1);
    }
}

/// Runs at most `jobs` unique evaluations while coalescing admitted duplicates.
/// Owns task cleanup separately from output delivery, preserving first-observed failure semantics.
async fn supervise(
    setup: StreamSetup,
    mut input: mpsc::Receiver<InputRow>,
    output: mpsc::Sender<RowOutcome>,
    mut signal: CancelSignal,
    cancel: CancelHandle,
    admission: Arc<Semaphore>,
) {
    let StreamSetup {
        client,
        config,
        key,
        unordered,
        fail_fast,
        #[cfg(test)]
        observer,
    } = setup;
    let jobs = config.jobs.expect("table settings include jobs").get();
    let mut state = SupervisorState {
        cache: CompletedCache::new(config.cache.expect("table settings include cache limits")),
        pending_groups: VecDeque::new(),
        in_flight: HashMap::new(),
        ordered: BTreeMap::new(),
        next_sequence: 0,
    };
    let config = Arc::new(config);
    let key = Arc::new(key);
    // Spawn complete evaluations, including deadlines and retries. Raw HTTP futures in
    // FuturesUnordered would stop progressing while this supervisor awaits a full output.
    let mut active = JoinSet::new();
    let mut task_routes = HashMap::new();
    let mut input_closed = false;

    let terminal = loop {
        // len() also counts completed, unjoined tasks. Separately, every retained row
        // still owns admission credit, even after joining a result frees a task position.
        while active.len() < jobs {
            let Some(group) = state.pending_groups.pop_front() else {
                break;
            };
            let PendingGroup {
                key: request_key,
                request,
                rows,
            } = group;
            let request_id = next_request_id();
            state.in_flight.insert(request_key.clone(), rows);
            let client = client.clone();
            let config = Arc::clone(&config);
            let key = Arc::clone(&key);
            let request_signal = signal.clone();
            let trace_id = request_id.clone();
            // Keep the deadline wrapper in the spawned task, not in the output path;
            // HTTP completion or timeout must release shared slots while output is stalled.
            let task = active.spawn(async move {
                crate::tracing::trace_evaluation(
                    "annotate",
                    &trace_id,
                    client.system_one_prepared_measured(&request, &config, &key, request_signal),
                )
                .await
            });
            // A panicking task has no return value; its Tokio ID still identifies its
            // duplicate group so task_completion can return a redacted error to every waiter.
            task_routes.insert(task.id(), (request_key, request_id));
            #[cfg(test)]
            if let Some(observer) = &observer {
                let _ = observer.send(SupervisorEvent::TaskCount(active.len()));
            }
        }
        if input_closed && state.pending_groups.is_empty() && active.is_empty() {
            break None;
        }
        let event = {
            let next_row = async {
                if input_closed {
                    pending().await
                } else {
                    input.recv().await
                }
            }
            .fuse();
            let completion = async {
                if active.is_empty() {
                    pending().await
                } else {
                    active.join_next_with_id().await
                }
            }
            .fuse();
            let closed = output.closed().fuse();
            let interrupted = signal.cancelled().fuse();
            futures::pin_mut!(next_row, completion, closed, interrupted);
            // Prefer termination and completed work over admitting another ready row.
            // Failure means first observed, not first completed: a successful send may
            // already be waiting for output capacity before another failure is joined.
            futures::select_biased! {
                _ = interrupted => Event::Interrupted,
                _ = closed => Event::OutputClosed,
                finished = completion => Event::Completed(finished),
                row = next_row => Event::Row(row),
            }
        };
        let routed = match event {
            Event::Interrupted => {
                tracing::info!(active_evaluations = active.len(), "annotation interrupted");
                break None;
            }
            Event::OutputClosed => {
                tracing::info!(
                    active_evaluations = active.len(),
                    "annotation output closed"
                );
                break None;
            }
            Event::Row(None) => {
                input_closed = true;
                continue;
            }
            Event::Row(Some(row)) => {
                route_row(row, &mut state, &output, &mut signal, unordered, fail_fast).await
            }
            Event::Completed(Some(task)) => {
                let completion = task_completion(task, &mut task_routes);
                #[cfg(test)]
                if let Some(observer) = &observer {
                    let next_ready = state.in_flight[&completion.0]
                        .iter()
                        .any(|row| row.sequence == state.next_sequence);
                    let terminal = completion
                        .2
                        .as_ref()
                        .err()
                        .is_some_and(|error| terminal_error(error, fail_fast));
                    if output.capacity() == 0 && (unordered || next_ready) && !terminal {
                        let _ = observer.send(SupervisorEvent::OutputBlocked);
                    }
                }
                finish_request(
                    completion,
                    &mut input,
                    &output,
                    &mut state,
                    &mut signal,
                    unordered,
                    fail_fast,
                )
                .await
            }
            Event::Completed(None) => break None,
        };
        match routed {
            Routing::Continue => {}
            Routing::Stopped => break None,
            Routing::Terminal(row) => break Some(row),
        }
    };
    // Stop the producer before dropping rows returns their credits. Close only this
    // invocation's row budget; closing the shared HTTP budget would cancel other callers.
    admission.close();
    input.close();
    drop(input);
    // shutdown aborts AND joins tasks. Their HTTP permits must be released before any
    // potentially unbounded wait to deliver the saved error. Never join the input thread:
    // an external next() may not return, but it owns no HTTP permit or shared mutex.
    active.shutdown().await;
    // Release the supervisor's cache, duplicate groups, key, and task-routing state
    // before a stalled consumer can retain the saved outcome and stream lifetime.
    drop(state);
    drop(task_routes);
    drop(config);
    drop(key);
    drop(client);
    #[cfg(test)]
    if let Some(observer) = &observer {
        let _ = observer.send(SupervisorEvent::WorkStopped);
    }
    if let Some(row) = terminal {
        #[cfg(test)]
        if output.capacity() == 0
            && let Some(observer) = &observer
        {
            let _ = observer.send(SupervisorEvent::OutputBlocked);
        }
        // Cancelling signal during worker cleanup would discard the original cause here.
        // External interrupt or output drop still cancels this wait through send_or_cancel.
        send_or_cancel(&output, row, &mut signal).await;
    }
    cancel.cancel();
    // Make closure visible before the test completion event. Relying on end-of-scope
    // drop lets the observer run first and incorrectly see an open output after Stopped.
    drop(output);
    #[cfg(test)]
    if let Some(observer) = observer {
        let _ = observer.send(SupervisorEvent::Stopped);
    }
}

/// Routes task failures without exposing panic payloads or leaving duplicate waiters stranded.
fn task_completion(
    task: TaskOutcome,
    routes: &mut HashMap<Id, (RequestKey, String)>,
) -> Completion {
    let (id, result) = match task {
        Ok(completed) => completed,
        Err(error) => {
            tracing::error!(
                task_cancelled = error.is_cancelled(),
                "Jev evaluation task failed"
            );
            (
                error.id(),
                Err(JevError::Transport("Jev evaluation task failed")),
            )
        }
    };
    let (key, request_id) = routes
        .remove(&id)
        .expect("evaluation task has routing state");
    (key, request_id, result)
}

/// Preserves terminal failures; otherwise joins admitted rows before retiring a shared group.
async fn finish_request(
    (request_key, request_id, result): Completion,
    input: &mut mpsc::Receiver<InputRow>,
    output: &mpsc::Sender<RowOutcome>,
    state: &mut SupervisorState,
    signal: &mut CancelSignal,
    unordered: bool,
    fail_fast: bool,
) -> Routing {
    let result = match result {
        Err(error) if terminal_error(&error, fail_fast) => {
            // Preserve the observed cause before inspecting queued input, whose own
            // failures or output sends could otherwise replace or delay this failure.
            // Keep the other duplicate rows and their credits until admission is closed;
            // dropping the whole group now would briefly let the producer read ahead.
            let first = state
                .in_flight
                .get_mut(&request_key)
                .expect("active request has waiters")
                .swap_remove(0);
            return Routing::Terminal(first.finish(Err(Arc::new(error))));
        }
        result => result,
    };
    // Drain only the currently queued prefix while this request is still in-flight.
    // Its duplicates must share this result even if the completed LRU cannot retain it.
    // A live drain-until-empty would let the producer delay group retirement indefinitely.
    let queued = input.len();
    for _ in 0..queued {
        let Ok(row) = input.try_recv() else { break };
        match route_row(row, state, output, signal, unordered, fail_fast).await {
            Routing::Continue => {}
            stopped => return stopped,
        }
    }
    let waiters = state
        .in_flight
        .remove(&request_key)
        .expect("active request has waiters");
    tracing::debug!(
        request_id = %request_id,
        shared_rows = waiters.len(),
        "Jev evaluation outcome ready"
    );
    let shared = match result {
        Ok(response) => {
            let result = Arc::new(SharedResponse::new(response, request_id));
            state.cache.insert(request_key, Arc::clone(&result));
            Ok(result)
        }
        Err(error) => Err(Arc::new(error)),
    };
    for row in waiters {
        match emit(
            row.finish(shared.clone()),
            output,
            &mut state.ordered,
            &mut state.next_sequence,
            unordered,
            fail_fast,
            signal,
        )
        .await
        {
            Routing::Continue => {}
            stopped => return stopped,
        }
    }
    Routing::Continue
}

/// Routes one accepted input row to cached, active, queued, or new work.
async fn route_row(
    mut row: InputRow,
    state: &mut SupervisorState,
    output: &mpsc::Sender<RowOutcome>,
    signal: &mut CancelSignal,
    unordered: bool,
    fail_fast: bool,
) -> Routing {
    let (request, request_key) = match row.request.take().expect("producer prepares every row") {
        Ok(request) => request,
        Err(error) => {
            return emit(
                row.finish(Err(Arc::new(error))),
                output,
                &mut state.ordered,
                &mut state.next_sequence,
                unordered,
                fail_fast,
                signal,
            )
            .await;
        }
    };
    if let Some(cached) = state.cache.get(&request_key) {
        tracing::debug!(
            request_id = %cached.request_id,
            row_sequence = row.sequence,
            "completed Jev result cache hit"
        );
        emit(
            row.finish(Ok(cached)),
            output,
            &mut state.ordered,
            &mut state.next_sequence,
            unordered,
            fail_fast,
            signal,
        )
        .await
    } else if let Some(waiters) = state.in_flight.get_mut(&request_key) {
        tracing::debug!(
            row_sequence = row.sequence,
            "joined in-flight Jev evaluation"
        );
        waiters.push(row);
        Routing::Continue
    } else if let Some(group) = state
        .pending_groups
        .iter_mut()
        .find(|group| group.key == request_key)
    {
        tracing::debug!(row_sequence = row.sequence, "joined queued Jev evaluation");
        group.rows.push(row);
        Routing::Continue
    } else {
        state.pending_groups.push_back(PendingGroup {
            key: request_key,
            request,
            rows: vec![row],
        });
        Routing::Continue
    }
}

/// Separates routine output progress from cancellation and a saved terminal outcome.
/// Terminal outcomes bypass ordered output so cleanup need not wait for earlier rows.
enum Routing {
    Continue,
    Stopped,
    Terminal(RowOutcome),
}

/// Stops on fail-policy errors and on unconditional output-destination collisions.
/// Keep/record cannot repair a reserved-field collision by passing the same conflicting row.
fn terminal_error(error: &JevError, fail_fast: bool) -> bool {
    fail_fast || matches!(error, JevError::FieldCollision(_))
}

/// Describes which bounded source of work became ready in the supervisor.
enum Event {
    Row(Option<InputRow>),
    Completed(Option<TaskOutcome>),
    Interrupted,
    OutputClosed,
}

/// Emits ready outcomes in sequence order unless unordered throughput is requested.
async fn emit(
    row: RowOutcome,
    output: &mpsc::Sender<RowOutcome>,
    ordered: &mut BTreeMap<u64, RowOutcome>,
    next_sequence: &mut u64,
    unordered: bool,
    fail_fast: bool,
    signal: &mut CancelSignal,
) -> Routing {
    if row
        .result
        .as_ref()
        .err()
        .is_some_and(|error| terminal_error(error, fail_fast))
    {
        // Do not send here: output may be full. Return ownership so the supervisor
        // can stop HTTP work before attempting terminal-error delivery.
        return Routing::Terminal(row);
    }
    if unordered {
        return if send_or_cancel(output, row, signal).await {
            Routing::Continue
        } else {
            Routing::Stopped
        };
    }
    // Each buffered outcome keeps its row credit; a slow first row cannot create
    // an unbounded reorder buffer by allowing completed later rows to admit more input.
    ordered.insert(row.sequence, row);
    while let Some(ready) = ordered.remove(next_sequence) {
        if !send_or_cancel(output, ready, signal).await {
            return Routing::Stopped;
        }
        *next_sequence = next_sequence.wrapping_add(1);
    }
    Routing::Continue
}

/// Awaits output capacity without blocking a worker and stops on local cancellation.
/// This wait does not poll task results; independently spawned evaluations still progress.
async fn send_or_cancel(
    output: &mpsc::Sender<RowOutcome>,
    row: RowOutcome,
    signal: &mut CancelSignal,
) -> bool {
    let cancelled = signal.cancelled().fuse();
    let send = output.send(row).fuse();
    futures::pin_mut!(cancelled, send);
    // When cancellation and capacity are both ready, cancellation wins; do not deliver
    // a late outcome merely because the consumer resumed at the same instant.
    futures::select_biased! {
        _ = cancelled => false,
        result = send => result.is_ok(),
    }
}

#[cfg(test)]
mod tests {
    /// Exercises independent task progress with full but open annotation output.
    mod backpressure;
    /// Checks cleanup before terminal-error delivery and uninterruptible input boundaries.
    mod terminal;

    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::{
        thread,
        time::{Duration, Instant},
    };

    use nu_protocol::Value;
    use serde_json::json;

    use crate::{
        api::{
            cancel::CancelHandle,
            client::{JevClient, JevClientPool},
            types::{Question, SystemOneRequest},
        },
        commands::tests::serve,
        config::{ApiKey, ConfigScope, ConfigSources, process::HttpAttemptLimit, resolve},
        h2_fixture,
        nu::value::to_json,
    };

    use super::{InputRow, Routing, StreamSetup, emit, finish_request, start, supervise};

    /// Builds a stable successful response for scheduler tests.
    fn answer(input_tokens: u64) -> serde_json::Value {
        json!({
            "model": "jev-fixed",
            "answers": {"q": {"type": "noul", "noul": 0.75}},
            "usage": {"input_tokens": input_tokens, "output_tokens": 1}
        })
    }

    /// Cancels a full output send even while the receiver remains alive.
    #[test]
    fn blocked_output_send_observes_cancellation() {
        runtime().block_on(async {
            let (output, _receiver) = tokio::sync::mpsc::channel(1);
            let admission = Arc::new(tokio::sync::Semaphore::new(2));
            let result = Err(Arc::new(crate::error::JevError::State("test row failure")));
            let first = InputRow {
                sequence: 0,
                source: Value::test_int(0),
                request: None,
                permit: Arc::clone(&admission).acquire_owned().await.unwrap(),
            }
            .finish(result.clone());
            output.send(first).await.unwrap();
            let second = InputRow {
                sequence: 1,
                source: Value::test_int(1),
                request: None,
                permit: admission.acquire_owned().await.unwrap(),
            }
            .finish(result);
            let (cancel, mut signal) = CancelHandle::new();
            let task = tokio::spawn(async move {
                emit(
                    second,
                    &output,
                    &mut std::collections::BTreeMap::new(),
                    &mut 1,
                    false,
                    false,
                    &mut signal,
                )
                .await
            });
            tokio::task::yield_now().await;
            assert!(!task.is_finished());
            cancel.cancel();
            assert!(matches!(
                tokio::time::timeout(Duration::from_millis(250), task)
                    .await
                    .expect("cancelled send must stop promptly")
                    .unwrap(),
                Routing::Stopped
            ));
        });
    }

    /// Tears down a backpressured supervisor without dropping its live receiver.
    #[test]
    fn interrupt_closes_full_output_without_receiver_drop() {
        let answer = answer(1);
        let (base_url, server) = serve(vec![answer.clone(), answer]);
        let (cancel, signal) = CancelHandle::new();
        let output = start(
            runtime(),
            setup(&base_url, 1),
            Box::new(vec![Value::test_int(0), Value::test_int(1)].into_iter()),
            Box::new(build),
            None,
            (cancel.clone(), signal),
        )
        .unwrap();
        server.wait_for_requests(2);
        let ready = Instant::now() + Duration::from_secs(1);
        while output.receiver.len() != 1 && Instant::now() < ready {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(output.receiver.len(), 1);
        assert!(!output.receiver.is_closed());
        cancel.cancel();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !output.receiver.is_closed() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            output.receiver.is_closed(),
            "supervisor did not close a full output after cancellation"
        );
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// An external iterator may stall, but its returned row is discarded after cancellation.
    #[test]
    fn blocked_upstream_does_not_dispatch_after_interrupt_or_drop() {
        /// Holds one upstream call until the test releases it.
        struct GatedInput {
            gate: Arc<std::sync::atomic::AtomicBool>,
            entered: Arc<std::sync::atomic::AtomicBool>,
            exited: Arc<std::sync::atomic::AtomicBool>,
        }

        impl Iterator for GatedInput {
            type Item = Value;

            /// Simulates a third-party `next()` that cannot observe plugin cancellation.
            fn next(&mut self) -> Option<Self::Item> {
                self.entered.store(true, Ordering::SeqCst);
                while !self.gate.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
                Some(Value::test_string("late"))
            }
        }

        impl Drop for GatedInput {
            /// Signals that the dedicated producer has stopped using the iterator.
            fn drop(&mut self) {
                self.exited.store(true, Ordering::SeqCst);
            }
        }

        for drop_output in [false, true] {
            let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let built = Arc::new(AtomicUsize::new(0));
            let built_for_request = Arc::clone(&built);
            let (cancel, signal) = CancelHandle::new();
            let output = start(
                runtime(),
                setup("http://127.0.0.1:9", 1),
                Box::new(GatedInput {
                    gate: Arc::clone(&gate),
                    entered: Arc::clone(&entered),
                    exited: Arc::clone(&exited),
                }),
                Box::new(move |value| {
                    built_for_request.fetch_add(1, Ordering::SeqCst);
                    build(value)
                }),
                None,
                (cancel.clone(), signal),
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(1);
            while !entered.load(Ordering::SeqCst) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            assert!(entered.load(Ordering::SeqCst));
            if drop_output {
                drop(output);
            } else {
                cancel.cancel();
                while !output.receiver.is_closed() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(1));
                }
                assert!(output.receiver.is_closed());
                drop(output);
            }
            gate.store(true, Ordering::SeqCst);
            while !exited.load(Ordering::SeqCst) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            assert!(exited.load(Ordering::SeqCst));
            assert_eq!(built.load(Ordering::SeqCst), 0);
        }
    }

    /// Builds a fixed Noul request from the input row's JSON state.
    fn build(value: &Value) -> Result<SystemOneRequest, crate::error::JevError> {
        Ok(SystemOneRequest {
            state: to_json(value).unwrap(),
            model: "jev-latest".into(),
            questions: Arc::new(
                [(
                    "q".into(),
                    Question::Noul {
                        instructions: None,
                        criteria: None,
                    },
                )]
                .into(),
            ),
        })
    }

    /// Creates a live setup against the local test server without engine access per row.
    fn setup(base_url: &str, jobs: usize) -> StreamSetup {
        let mut config = resolve(&ConfigSources::default(), ConfigScope::Table).unwrap();
        config.base_url = reqwest::Url::parse(base_url).unwrap();
        config.jobs = std::num::NonZeroUsize::new(jobs);
        StreamSetup {
            client: JevClient::new().unwrap(),
            config,
            key: ApiKey::for_test("local-key"),
            unordered: false,
            fail_fast: true,
            observer: None,
        }
    }

    /// Builds a Tokio runtime explicitly for the scheduler's async supervisor.
    fn runtime() -> Arc<tokio::runtime::Runtime> {
        Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),
        )
    }

    /// Waits for logical attempt admission independently of actual HTTP sends.
    async fn wait_for_admissions(
        receiver: &mut tokio::sync::mpsc::UnboundedReceiver<()>,
        n: usize,
    ) {
        tokio::time::timeout(Duration::from_secs(3), async {
            for _ in 0..n {
                receiver.recv().await.expect("admission observer closed");
            }
        })
        .await
        .expect("table did not admit its jobs allowance");
    }

    /// Jobs and read-ahead stay local while the shared budget stalls queued or active rows.
    #[test]
    fn process_budget_preserves_table_bounds_and_local_output_drop() {
        for unordered in [false, true] {
            for (jobs, capacity) in [(Some(8), 2), (None, 128)] {
                let gate = h2_fixture::ResponseGate::default();
                let held = gate.clone();
                let (base_url, server) = h2_fixture::serve_unbounded(move |_, _| {
                    let mut response = h2_fixture::Response::json(200, answer(1));
                    response.body_gate = Some(held.clone());
                    response
                });
                let pool = JevClientPool::new(HttpAttemptLimit::new(capacity).unwrap()).unwrap();
                let mut client = pool
                    .for_policy(&crate::config::ProxyPolicy::Direct)
                    .unwrap();
                let (observed, mut admissions) = tokio::sync::mpsc::unbounded_channel();
                client.observe_admission(observed);
                let runtime = runtime();
                let mut busy = None;
                if capacity == 2 {
                    let mut settings = setup(&base_url, 2);
                    settings.client = client.clone();
                    busy = Some(
                        start(
                            Arc::clone(&runtime),
                            settings,
                            Box::new(
                                [Value::test_string("busy-1"), Value::test_string("busy-2")]
                                    .into_iter(),
                            ),
                            Box::new(build),
                            None,
                            CancelHandle::new(),
                        )
                        .unwrap(),
                    );
                    runtime.block_on(async {
                        wait_for_admissions(&mut admissions, 2).await;
                        gate.wait_for_arrivals(2).await;
                    });
                }
                let mut settings = setup(&base_url, jobs.unwrap_or(16));
                if jobs.is_none() {
                    settings.config =
                        resolve(&ConfigSources::default(), ConfigScope::Table).unwrap();
                    settings.config.base_url = base_url.parse().unwrap();
                }
                let n = settings.config.jobs.unwrap().get();
                assert_eq!(n, jobs.unwrap_or(16));
                settings.client = client.clone();
                settings.unordered = unordered;
                let reads = Arc::new(AtomicUsize::new(0));
                let input_reads = Arc::clone(&reads);
                let input = (0..1000).map(move |index| {
                    input_reads.fetch_add(1, Ordering::SeqCst);
                    Value::test_string(format!("row-{index}"))
                });
                let output = start(
                    Arc::clone(&runtime),
                    settings,
                    Box::new(input),
                    Box::new(build),
                    None,
                    CancelHandle::new(),
                )
                .unwrap();
                runtime.block_on(async {
                    wait_for_admissions(&mut admissions, n).await;
                    if capacity == 128 {
                        gate.wait_for_arrivals(n).await;
                    }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    assert!(
                        admissions.try_recv().is_err(),
                        "jobs allowed extra evaluations"
                    );
                });
                assert!(reads.load(Ordering::SeqCst) <= 2 * n);
                assert_eq!(server.request_count(), if capacity == 2 { 2 } else { n });
                drop(output);
                if let Some(busy) = busy {
                    runtime.block_on(async { tokio::time::sleep(Duration::from_millis(30)).await });
                    assert_eq!(client.free_attempt_slots(), 0);
                    assert!(
                        !busy.receiver.is_closed(),
                        "queued cancellation stopped a neighbor"
                    );
                    gate.release();
                    let rows: Vec<_> = busy.collect();
                    assert_eq!(rows.len(), 2);
                    assert!(rows.iter().all(|row| row.result.is_ok()));
                }
                runtime.block_on(async {
                    tokio::time::timeout(Duration::from_secs(3), async {
                        while client.free_attempt_slots() != capacity {
                            tokio::task::yield_now().await;
                        }
                    })
                    .await
                    .expect("output drop retained HTTP capacity");
                });
                let (healthy_url, healthy_server) = serve(vec![answer(1)]);
                let mut healthy = setup(&healthy_url, 1);
                healthy.client = client;
                let rows: Vec<_> = start(
                    Arc::clone(&runtime),
                    healthy,
                    Box::new(std::iter::once(Value::test_string("healthy"))),
                    Box::new(build),
                    None,
                    CancelHandle::new(),
                )
                .unwrap()
                .collect();
                assert!(rows[0].result.is_ok());
                gate.release();
                assert!(reads.load(Ordering::SeqCst) <= 2 * n);
                assert_eq!(
                    server.join().unwrap().len(),
                    if capacity == 2 { 2 } else { n }
                );
                healthy_server.join().unwrap();
            }
        }
    }

    /// A completed-cache hit stays available while a different invocation owns the only slot.
    #[test]
    fn cached_rows_bypass_a_full_process_budget() {
        /// Releases the dedicated producer even if a cache assertion fails.
        struct InputRelease(Arc<std::sync::atomic::AtomicBool>);

        impl Drop for InputRelease {
            /// Makes an intentionally stalled upstream iterator return during test cleanup.
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let gate = h2_fixture::ResponseGate::default();
        let held = gate.clone();
        let (base_url, server) = h2_fixture::serve(2, move |_, request| {
            let mut response = h2_fixture::Response::json(200, answer(1));
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            if body["state"] == "busy" {
                response.body_gate = Some(held.clone());
            }
            response
        });
        let pool = JevClientPool::new(HttpAttemptLimit::new(1).unwrap()).unwrap();
        let client = pool
            .for_policy(&crate::config::ProxyPolicy::Direct)
            .unwrap();
        let runtime = runtime();
        let input_release = InputRelease(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let mut settings = setup(&base_url, 1);
        settings.client = client.clone();
        let mut output = start(
            Arc::clone(&runtime),
            settings,
            gated_input(vec!["same", "same"], Arc::clone(&input_release.0)),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap();
        let first = output.next().unwrap().result.unwrap();
        let mut settings = setup(&base_url, 1);
        settings.client = client;
        let busy = start(
            Arc::clone(&runtime),
            settings,
            Box::new(std::iter::once(Value::test_string("busy"))),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap();
        runtime.block_on(gate.wait_for_arrivals(1));
        input_release.0.store(true, Ordering::SeqCst);
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), async {
                while output.receiver.is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("cache hit waited for a network slot");
        });
        let second = output.next().unwrap().result.unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(output.next().is_none());
        assert_eq!(server.request_count(), 2);
        drop(busy);
        gate.release();
        server.join().unwrap();
    }

    /// Dropping an admitted stream releases only its slot and leaves a held neighbor active.
    #[test]
    fn output_drop_preserves_another_active_http_attempt() {
        let gate = h2_fixture::ResponseGate::default();
        let held = gate.clone();
        let (base_url, server) = h2_fixture::serve(2, move |_, _| {
            let mut response = h2_fixture::Response::json(200, answer(1));
            response.body_gate = Some(held.clone());
            response
        });
        let pool = JevClientPool::new(HttpAttemptLimit::new(2).unwrap()).unwrap();
        let client = pool
            .for_policy(&crate::config::ProxyPolicy::Direct)
            .unwrap();
        let runtime = runtime();
        let mut first = setup(&base_url, 1);
        first.client = client.clone();
        let mut second = setup(&base_url, 1);
        second.client = client.clone();
        let cancelled = start(
            Arc::clone(&runtime),
            first,
            Box::new((0..100).map(|index| Value::test_string(format!("cancel-{index}")))),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap();
        let healthy = start(
            Arc::clone(&runtime),
            second,
            Box::new(std::iter::once(Value::test_string("healthy"))),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap();
        runtime.block_on(gate.wait_for_arrivals(2));
        drop(cancelled);
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(3), async {
                while client.free_attempt_slots() != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("active cancellation lost or retained a neighbor's permit");
        });
        assert!(!healthy.receiver.is_closed());
        assert!(healthy.receiver.is_empty());
        gate.release();
        let rows: Vec<_> = healthy.collect();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].result.is_ok());
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// Shares one logical evaluation and provenance across equal admitted rows.
    #[test]
    fn duplicate_rows_share_a_single_request() {
        let response = answer(4);
        let (base_url, server) = serve(vec![response]);
        let output = start(
            runtime(),
            setup(&base_url, 2),
            Box::new(vec![Value::test_string("same"), Value::test_string("same")].into_iter()),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap();
        let rows: Vec<_> = output.collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].sequence, 0);
        assert_eq!(rows[1].sequence, 1);
        let first = rows[0].result.as_ref().unwrap();
        let second = rows[1].result.as_ref().unwrap();
        assert_eq!(first.request_id, second.request_id);
        assert_eq!(first.base_url, format!("{base_url}/"));
        assert_eq!(first.measurement.attempts, 1);
        assert!(first.measurement.request_bytes > 0);
        assert!(first.measurement.response_bytes > 0);
        assert!(Arc::ptr_eq(first, second));
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// Acquires admission credit before reading another source item.
    #[test]
    fn stopped_consumer_bounds_input_reads_to_twice_jobs() {
        let response = answer(4);
        let (base_url, server) = serve(vec![response.clone(), response]);
        let reads = Arc::new(AtomicUsize::new(0));
        let input_reads = Arc::clone(&reads);
        let input = (0..100).map(move |index| {
            input_reads.fetch_add(1, Ordering::SeqCst);
            Value::test_int(index)
        });
        let output = start(
            runtime(),
            setup(&base_url, 1),
            Box::new(input),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(reads.load(Ordering::SeqCst) <= 2);
        drop(output);
        let _ = server.join();
    }

    /// A slow first request cannot make the producer read beyond the admission window.
    #[test]
    fn stalled_first_row_keeps_read_ahead_bounded() {
        let (base_url, server) = serve_out_of_order();
        let reads = Arc::new(AtomicUsize::new(0));
        let input_reads = Arc::clone(&reads);
        let input = (0..100).map(move |index| {
            input_reads.fetch_add(1, Ordering::SeqCst);
            Value::test_int(index)
        });
        let output = start(
            runtime(),
            setup(&base_url, 2),
            Box::new(input),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(40));
        assert!(reads.load(Ordering::SeqCst) <= 4);
        drop(output);
        let _ = server.join();
    }

    /// Counts overlapping HTTP/2 streams and returns their peak on join.
    struct PeakServer {
        server: h2_fixture::TestServer,
        peak: Arc<AtomicUsize>,
    }

    impl PeakServer {
        /// Stops the server after its two requests and returns peak concurrency.
        fn join(self) -> thread::Result<usize> {
            self.server.join().map(|_| self.peak.load(Ordering::SeqCst))
        }
    }

    /// Serves two streams concurrently and intentionally delays state zero.
    fn serve_out_of_order() -> (String, PeakServer) {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let observed_peak = Arc::clone(&peak);
        let (root, server) = h2_fixture::serve_unbounded(move |_, captured| {
            let request: serde_json::Value = serde_json::from_slice(&captured.body).unwrap();
            let count = active.fetch_add(1, Ordering::SeqCst) + 1;
            observed_peak.fetch_max(count, Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(1);
            while observed_peak.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            active.fetch_sub(1, Ordering::SeqCst);
            let mut response = h2_fixture::Response::json(200, answer(1));
            if request["state"] == 0 {
                response.delay = Duration::from_millis(120);
            }
            response
        });
        (root, PeakServer { server, peak })
    }

    /// Delays one local response long enough for duplicate admission or failure bypass.
    fn serve_delayed_one(response: serde_json::Value) -> (String, h2_fixture::TestServer) {
        h2_fixture::serve_unbounded(move |_, _| {
            let mut response = h2_fixture::Response::json(200, response.clone());
            response.delay = Duration::from_millis(120);
            response
        })
    }

    /// Gates the first HTTP/2 response and counts each request stream.
    struct GatedServer {
        root: String,
        started: Option<tokio::sync::oneshot::Receiver<()>>,
        release: Option<std::sync::mpsc::Sender<()>>,
        worker: h2_fixture::TestServer,
    }

    impl GatedServer {
        /// Starts a loopback server that holds its first response until explicitly released.
        fn new(response: serde_json::Value) -> Self {
            let (started_tx, started) = tokio::sync::oneshot::channel();
            let (release, release_rx) = std::sync::mpsc::channel();
            let started_tx = Mutex::new(Some(started_tx));
            let release_rx = Mutex::new(release_rx);
            let (root, worker) = h2_fixture::serve_unbounded(move |index, _| {
                if index == 0 {
                    if let Some(started_tx) = started_tx.lock().unwrap().take() {
                        let _ = started_tx.send(());
                    }
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap();
                }
                h2_fixture::Response::json(200, response.clone())
            });
            Self {
                root,
                started: Some(started),
                release: Some(release),
                worker,
            }
        }

        /// Waits until the first HTTP request body has reached the server.
        async fn wait_started(&mut self) {
            self.started.take().unwrap().await.unwrap();
        }

        /// Allows the held first response to reach the client.
        fn release(&mut self) {
            self.release.take().unwrap().send(()).unwrap();
        }

        /// Stops the fixture and returns the number of accepted HTTP/2 streams.
        fn finish(self) -> usize {
            self.worker.join().unwrap().len()
        }
    }

    /// Prepares one manually admitted row for the real HTTP supervisor test.
    async fn prepared_input_row(
        sequence: u64,
        root: Arc<str>,
        admission: Arc<tokio::sync::Semaphore>,
    ) -> InputRow {
        let source = Value::test_string("same");
        let prepared = crate::api::client::PreparedRequest::new(build(&source).unwrap()).unwrap();
        let key = crate::nu::cache::RequestKey::from_body(root, prepared.body.clone());
        InputRow {
            sequence,
            source,
            request: Some(Ok((prepared, key))),
            permit: admission.acquire_owned().await.unwrap(),
        }
    }

    /// Returns one retryable status before a valid response for the same logical request.
    fn serve_retry_once() -> (String, h2_fixture::TestServer) {
        h2_fixture::serve(2, |attempt, _| {
            if attempt == 0 {
                let mut response = h2_fixture::Response::json(503, json!({}));
                response.headers.push(("retry-after".into(), "0".into()));
                response
            } else {
                h2_fixture::Response::json(200, answer(1))
            }
        })
    }

    /// Returns one retryable failure, then success, to observe whether output drop stops retries.
    fn serve_long_retry() -> (String, Arc<AtomicUsize>, h2_fixture::TestServer) {
        let calls = Arc::new(AtomicUsize::new(0));
        let server_calls = Arc::clone(&calls);
        let (root, server) = h2_fixture::serve_unbounded(move |attempt, _| {
            server_calls.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                let mut response = h2_fixture::Response::json(503, json!({}));
                response.headers.push(("retry-after".into(), "1".into()));
                response
            } else {
                h2_fixture::Response::json(200, answer(1))
            }
        });
        (root, calls, server)
    }

    /// Keeps normal output ordered while unordered mode releases the faster row first.
    #[test]
    fn bounds_unique_concurrency_and_obeys_output_order_mode() {
        for unordered in [false, true] {
            let (base_url, server) = serve_out_of_order();
            let mut setup = setup(&base_url, 2);
            setup.unordered = unordered;
            let output = start(
                runtime(),
                setup,
                Box::new(vec![Value::test_int(0), Value::test_int(1)].into_iter()),
                Box::new(build),
                None,
                CancelHandle::new(),
            )
            .unwrap();
            let sequences: Vec<_> = output.map(|row| row.sequence).collect();
            assert_eq!(sequences, if unordered { vec![1, 0] } else { vec![0, 1] });
            assert_eq!(server.join().unwrap(), 2);
        }
    }

    /// Gates the last row until earlier outcomes have been consumed downstream.
    fn gated_input(
        states: Vec<&'static str>,
        gate: Arc<std::sync::atomic::AtomicBool>,
    ) -> Box<dyn Iterator<Item = Value> + Send> {
        Box::new(states.into_iter().enumerate().map(move |(index, state)| {
            if index > 0 {
                while !gate.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
            }
            Value::test_string(state)
        }))
    }

    /// Re-evaluates identical state after an error because failures never enter the cache.
    #[test]
    fn failed_result_is_not_cached() {
        let good = answer(1);
        let (base_url, server) = serve(vec![json!({"bad": true}), good]);
        let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut setup = setup(&base_url, 1);
        setup.fail_fast = false;
        let mut output = start(
            runtime(),
            setup,
            gated_input(vec!["same", "same"], Arc::clone(&gate)),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap();
        let first = output.next().unwrap();
        assert!(first.result.is_err());
        drop(first);
        gate.store(true, Ordering::SeqCst);
        assert!(output.next().unwrap().result.is_ok());
        assert!(output.next().is_none());
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// A small cache must permit a new identity after the previous success is evicted.
    #[test]
    fn eviction_allows_a_fresh_logical_request() {
        let good = answer(1);
        let (base_url, server) = serve(vec![good.clone(), good.clone(), good]);
        let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut setup = setup(&base_url, 1);
        setup.config.cache.as_mut().unwrap().max_entries = std::num::NonZeroUsize::new(1).unwrap();
        let input = Box::new(vec!["A", "B", "A"].into_iter().enumerate().map({
            let gate = Arc::clone(&gate);
            move |(index, state)| {
                if index == 2 {
                    while !gate.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
                Value::test_string(state)
            }
        }));
        let mut output = start(
            runtime(),
            setup,
            input,
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap();
        let first = output.next().unwrap();
        let first_id = first.result.as_ref().unwrap().request_id.clone();
        drop(first);
        drop(output.next().unwrap());
        gate.store(true, Ordering::SeqCst);
        let last = output.next().unwrap();
        assert_ne!(last.result.as_ref().unwrap().request_id, first_id);
        drop(last);
        assert!(output.next().is_none());
        assert_eq!(server.join().unwrap().len(), 3);
    }

    /// Mandatory in-flight sharing works even when the completed cache cannot retain a result.
    #[test]
    fn concurrent_duplicates_share_when_cache_is_ineligible() {
        let (base_url, server) = serve_delayed_one(answer(1));
        let mut setup = setup(&base_url, 2);
        setup.config.cache.as_mut().unwrap().max_approx_bytes =
            std::num::NonZeroUsize::new(1).unwrap();
        let rows: Vec<_> = start(
            runtime(),
            setup,
            Box::new(vec![Value::test_string("same"), Value::test_string("same")].into_iter()),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap()
        .collect();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.result.is_ok()));
        assert_eq!(
            rows[0].result.as_ref().unwrap().request_id,
            rows[1].result.as_ref().unwrap().request_id
        );
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// Joins a channel-admitted duplicate before retiring a success or error group.
    #[test]
    fn completion_routes_queued_duplicates_before_retirement() {
        for success in [true, false] {
            runtime().block_on(async {
                let source = Value::test_string("same");
                let prepared =
                    crate::api::client::PreparedRequest::new(build(&source).unwrap()).unwrap();
                let key = crate::nu::cache::RequestKey::from_body(
                    Arc::from("http://example.test/"),
                    prepared.body.clone(),
                );
                let admission = Arc::new(tokio::sync::Semaphore::new(2));
                let first = InputRow {
                    sequence: 0,
                    source: source.clone(),
                    request: None,
                    permit: Arc::clone(&admission).acquire_owned().await.unwrap(),
                };
                let second = InputRow {
                    sequence: 1,
                    source,
                    request: Some(Ok((prepared, key.clone()))),
                    permit: admission.acquire_owned().await.unwrap(),
                };
                let (input_sender, mut input) = tokio::sync::mpsc::channel(2);
                input_sender.send(second).await.unwrap();
                let (output, mut outcomes) = tokio::sync::mpsc::channel(2);
                let config = setup("http://example.test", 2).config;
                let mut limits = config.cache.unwrap();
                limits.max_approx_bytes = std::num::NonZeroUsize::new(1).unwrap();
                let mut state = super::SupervisorState {
                    cache: crate::nu::cache::CompletedCache::new(limits),
                    pending_groups: std::collections::VecDeque::new(),
                    in_flight: std::collections::HashMap::from([(key.clone(), vec![first])]),
                    ordered: std::collections::BTreeMap::new(),
                    next_sequence: 0,
                };
                let result = if success {
                    Ok(crate::api::client::MeasuredSuccess {
                        response: serde_json::from_value(answer(1)).unwrap(),
                        base_url: "http://example.test/".into(),
                        measurement: crate::api::client::HttpMeasurement {
                            request_bytes: 1,
                            response_bytes: 1,
                            elapsed: Duration::ZERO,
                            attempt_elapsed: Duration::ZERO,
                            attempts: 1,
                            http_version: reqwest::Version::HTTP_11,
                        },
                    })
                } else {
                    Err(crate::error::JevError::Response("test failure"))
                };
                let (_cancel, mut signal) = CancelHandle::new();
                assert!(matches!(
                    finish_request(
                        (key, "jev-shared".into(), result),
                        &mut input,
                        &output,
                        &mut state,
                        &mut signal,
                        false,
                        false,
                    )
                    .await,
                    Routing::Continue
                ));
                assert!(input.is_empty());
                assert!(state.pending_groups.is_empty());
                assert!(state.in_flight.is_empty());
                let first = outcomes.recv().await.unwrap();
                let second = outcomes.recv().await.unwrap();
                assert_eq!((first.sequence, second.sequence), (0, 1));
                match (first.result, second.result) {
                    (Ok(first), Ok(second)) => {
                        assert!(success);
                        assert!(Arc::ptr_eq(&first, &second));
                        assert_eq!(first.request_id, "jev-shared");
                    }
                    (Err(first), Err(second)) => {
                        assert!(!success);
                        assert!(Arc::ptr_eq(&first, &second));
                    }
                    _ => panic!("duplicate outcomes diverged"),
                }
            });
        }
    }

    /// Shares one gated HTTP operation with a duplicate admitted before its response is released.
    #[test]
    fn gated_http_completion_shares_admitted_success_and_error() {
        for success in [true, false] {
            let response = if success {
                answer(1)
            } else {
                json!({"bad": true})
            };
            let mut server = GatedServer::new(response);
            let mut setup = setup(&server.root, 2);
            setup.fail_fast = false;
            setup.config.cache.as_mut().unwrap().max_approx_bytes =
                std::num::NonZeroUsize::new(1).unwrap();
            let root: Arc<str> = Arc::from(setup.config.base_url.as_str());
            let (first, second) = runtime().block_on(async {
                let admission = Arc::new(tokio::sync::Semaphore::new(2));
                let (input_sender, input_receiver) = tokio::sync::mpsc::channel(2);
                let (output_sender, mut output_receiver) = tokio::sync::mpsc::channel(2);
                let (cancel, signal) = CancelHandle::new();
                let supervisor = tokio::spawn(supervise(
                    setup,
                    input_receiver,
                    output_sender,
                    signal,
                    cancel,
                    Arc::clone(&admission),
                ));
                input_sender
                    .send(prepared_input_row(0, Arc::clone(&root), Arc::clone(&admission)).await)
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(2), server.wait_started())
                    .await
                    .unwrap();
                input_sender
                    .send(prepared_input_row(1, root, admission).await)
                    .await
                    .unwrap();
                drop(input_sender);
                server.release();
                let first = tokio::time::timeout(Duration::from_secs(2), output_receiver.recv())
                    .await
                    .unwrap()
                    .unwrap();
                let second = tokio::time::timeout(Duration::from_secs(2), output_receiver.recv())
                    .await
                    .unwrap()
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(2), supervisor)
                    .await
                    .unwrap()
                    .unwrap();
                (first, second)
            });
            assert_eq!((first.sequence, second.sequence), (0, 1));
            match (first.result, second.result) {
                (Ok(first), Ok(second)) => {
                    assert!(success);
                    assert!(Arc::ptr_eq(&first, &second));
                    assert_eq!(first.request_id, second.request_id);
                }
                (Err(first), Err(second)) => {
                    assert!(!success);
                    assert!(Arc::ptr_eq(&first, &second));
                }
                _ => panic!("gated duplicate outcomes diverged"),
            }
            assert_eq!(server.finish(), 1);
        }
    }

    /// Shares one failed HTTP result with duplicates admitted before completion.
    #[test]
    fn concurrent_duplicates_share_an_uncached_error() {
        let (base_url, server) = serve_delayed_one(json!({"bad": true}));
        let mut setup = setup(&base_url, 2);
        setup.fail_fast = false;
        let rows: Vec<_> = start(
            runtime(),
            setup,
            Box::new(vec![Value::test_string("same"), Value::test_string("same")].into_iter()),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap()
        .collect();
        assert_eq!(rows.len(), 2);
        assert!(Arc::ptr_eq(
            rows[0].result.as_ref().unwrap_err(),
            rows[1].result.as_ref().unwrap_err()
        ));
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// Subscribers retain one logical identity through every retry attempt.
    #[test]
    fn concurrent_duplicates_share_the_retry_sequence() {
        let (base_url, server) = serve_retry_once();
        let mut setup = setup(&base_url, 2);
        setup.config.cache.as_mut().unwrap().max_approx_bytes =
            std::num::NonZeroUsize::new(1).unwrap();
        let rows: Vec<_> = start(
            runtime(),
            setup,
            Box::new(vec![Value::test_string("same"), Value::test_string("same")].into_iter()),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap()
        .collect();
        assert_eq!(rows.len(), 2);
        let first = rows[0].result.as_ref().unwrap();
        let second = rows[1].result.as_ref().unwrap();
        assert_eq!(first.request_id, second.request_id);
        assert!(Arc::ptr_eq(first, second));
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// A terminal row failure bypasses a slow earlier slot in ordered mode.
    #[test]
    fn fail_fast_bypasses_stalled_ordered_row() {
        let (base_url, server) = serve_delayed_one(answer(1));
        let build = Box::new(|value: &Value| {
            if value.as_int().unwrap() == 1 {
                Err(crate::error::JevError::State("bad row"))
            } else {
                build(value)
            }
        });
        let mut output = start(
            runtime(),
            setup(&base_url, 2),
            Box::new(vec![Value::test_int(0), Value::test_int(1)].into_iter()),
            build,
            None,
            CancelHandle::new(),
        )
        .unwrap();
        let first = output.next().unwrap();
        assert_eq!(first.sequence, 1);
        assert!(first.result.is_err());
        drop(first);
        assert!(output.next().is_none());
        server.join().unwrap();
    }

    /// A destination collision bypasses ordering even when ordinary errors are kept.
    #[test]
    fn field_collision_bypasses_stalled_row_under_keep_policy() {
        let (base_url, server) = serve_delayed_one(answer(1));
        let build = Box::new(|value: &Value| {
            if value.as_int().unwrap() == 1 {
                Err(crate::error::JevError::FieldCollision(
                    "Jev destination field already exists",
                ))
            } else {
                build(value)
            }
        });
        let mut settings = setup(&base_url, 2);
        settings.fail_fast = false;
        let mut output = start(
            runtime(),
            settings,
            Box::new(vec![Value::test_int(0), Value::test_int(1)].into_iter()),
            build,
            None,
            CancelHandle::new(),
        )
        .unwrap();
        let first = output.next().unwrap();
        assert_eq!(first.sequence, 1);
        assert!(matches!(
            first.result.unwrap_err().as_ref(),
            crate::error::JevError::FieldCollision(_)
        ));
        assert!(output.next().is_none());
        server.join().unwrap();
    }

    /// Nonterminal failed rows still advance the ordered sequence for keep/record policies.
    #[test]
    fn nonterminal_error_advances_ordered_output() {
        let good = answer(1);
        let (base_url, server) = serve(vec![good.clone(), good]);
        let build = Box::new(|value: &Value| {
            if value.as_int().unwrap() == 1 {
                Err(crate::error::JevError::State("bad row"))
            } else {
                build(value)
            }
        });
        let mut setup = setup(&base_url, 2);
        setup.fail_fast = false;
        let rows: Vec<_> = start(
            runtime(),
            setup,
            Box::new(vec![0, 1, 2].into_iter().map(Value::test_int)),
            build,
            None,
            CancelHandle::new(),
        )
        .unwrap()
        .collect();
        assert_eq!(
            rows.iter().map(|row| row.sequence).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(rows[0].result.is_ok());
        assert!(rows[1].result.is_err());
        assert!(rows[2].result.is_ok());
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// An outcome stays reusable after its first source row is consumed downstream.
    #[test]
    fn completed_success_survives_downstream_discard() {
        let good = answer(1);
        let (base_url, server) = serve(vec![good]);
        let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut output = start(
            runtime(),
            setup(&base_url, 1),
            gated_input(vec!["same", "same"], Arc::clone(&gate)),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap();
        let first = output.next().unwrap();
        let first_result = Arc::clone(first.result.as_ref().unwrap());
        drop(first);
        gate.store(true, Ordering::SeqCst);
        let second = output.next().unwrap();
        let second_result = second.result.as_ref().unwrap();
        assert_eq!(second_result.request_id, first_result.request_id);
        assert_eq!(second_result.base_url, first_result.base_url);
        assert!(Arc::ptr_eq(second_result, &first_result));
        drop(second);
        assert!(output.next().is_none());
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// A result too large for the cache is re-evaluated with new provenance.
    #[test]
    fn oversized_bypass_gets_new_request_identity() {
        let good = answer(1);
        let (base_url, server) = serve(vec![good.clone(), good]);
        let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut setup = setup(&base_url, 1);
        setup.config.cache.as_mut().unwrap().max_approx_bytes =
            std::num::NonZeroUsize::new(1).unwrap();
        let mut output = start(
            runtime(),
            setup,
            gated_input(vec!["same", "same"], Arc::clone(&gate)),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap();
        let first = output.next().unwrap();
        let first_id = first.result.as_ref().unwrap().request_id.clone();
        drop(first);
        gate.store(true, Ordering::SeqCst);
        let second = output.next().unwrap();
        assert_ne!(second.result.as_ref().unwrap().request_id, first_id);
        drop(second);
        assert!(output.next().is_none());
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// Reusing the process client does not reuse an earlier invocation's cached result.
    #[test]
    fn cache_does_not_cross_invocation_boundary() {
        let good = answer(1);
        let (base_url, server) = serve(vec![good.clone(), good]);
        let runtime = runtime();
        let client = JevClient::new().unwrap();
        let ids: Vec<_> = (0..2)
            .map(|_| {
                let mut setup = setup(&base_url, 1);
                setup.client = client.clone();
                let mut output = start(
                    Arc::clone(&runtime),
                    setup,
                    Box::new(std::iter::once(Value::test_string("same"))),
                    Box::new(build),
                    None,
                    CancelHandle::new(),
                )
                .unwrap();
                let row = output.next().unwrap();
                row.result.as_ref().unwrap().request_id.clone()
            })
            .collect();
        assert_ne!(ids[0], ids[1]);
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// Evicting a fast success does not invalidate its buffered duplicate outcomes.
    #[test]
    fn eviction_preserves_queued_shared_outcomes() {
        let (base_url, server) = serve_out_of_order();
        let mut setup = setup(&base_url, 2);
        setup.config.cache.as_mut().unwrap().max_entries = std::num::NonZeroUsize::new(1).unwrap();
        let rows: Vec<_> = start(
            runtime(),
            setup,
            Box::new(vec![Value::test_int(0), Value::test_int(1), Value::test_int(1)].into_iter()),
            Box::new(build),
            None,
            CancelHandle::new(),
        )
        .unwrap()
        .collect();
        assert_eq!(
            rows.iter().map(|row| row.sequence).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        let first_duplicate = rows[1].result.as_ref().unwrap();
        let second_duplicate = rows[2].result.as_ref().unwrap();
        assert_eq!(first_duplicate.request_id, second_duplicate.request_id);
        assert!(Arc::ptr_eq(first_duplicate, second_duplicate));
        assert_ne!(
            rows[0].result.as_ref().unwrap().request_id,
            first_duplicate.request_id
        );
        assert_eq!(server.join().unwrap(), 2);
    }

    /// Closing one output during Retry-After does not cancel a simultaneous invocation.
    #[test]
    fn output_drop_cancels_retry_without_affecting_another_invocation() {
        for close_output in [true, false] {
            let (stalled_url, stalled_calls, stalled_server) = serve_long_retry();
            let (healthy_url, healthy_server) = serve(vec![answer(1)]);
            let runtime = runtime();
            let pool = JevClientPool::new(HttpAttemptLimit::new(1).unwrap()).unwrap();
            let mut client = pool
                .for_policy(&crate::config::ProxyPolicy::Direct)
                .unwrap();
            let (retry_started, mut retry_waits) = tokio::sync::mpsc::channel(1);
            client.observe_retry_waits(retry_started);
            let mut stalled_setup = setup(&stalled_url, 1);
            stalled_setup.client = client.clone();
            stalled_setup.key = ApiKey::for_test("stalled-key");
            let stalled = start(
                Arc::clone(&runtime),
                stalled_setup,
                Box::new(std::iter::once(Value::test_string("slow"))),
                Box::new(build),
                None,
                CancelHandle::new(),
            )
            .unwrap();
            let (retry_due, delay) = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(2), retry_waits.recv())
                    .await
                    .expect("client did not enter its retry wait")
                    .expect("retry observer closed")
            });
            assert_eq!(delay, Duration::from_secs(1));
            assert_eq!(stalled_calls.load(Ordering::SeqCst), 1);
            let mut healthy_setup = setup(&healthy_url, 1);
            healthy_setup.client = client;
            healthy_setup.key = ApiKey::for_test("healthy-key");
            let healthy = start(
                Arc::clone(&runtime),
                healthy_setup,
                Box::new(std::iter::once(Value::test_string("healthy"))),
                Box::new(build),
                None,
                CancelHandle::new(),
            )
            .unwrap();
            let mut stalled = Some(stalled);
            if close_output {
                assert!(
                    Instant::now() < retry_due,
                    "retry wait ended before output drop"
                );
                drop(stalled.take());
            }
            let rows: Vec<_> = healthy.collect();
            assert_eq!(rows.len(), 1);
            assert!(rows[0].result.is_ok());
            assert_eq!(
                healthy_server.join().unwrap()[0].authorization.as_deref(),
                Some("Bearer healthy-key")
            );
            let observed_until = retry_due + Duration::from_millis(250);
            thread::sleep(observed_until.saturating_duration_since(Instant::now()));
            let expected = if close_output { 1 } else { 2 };
            if let Some(mut output) = stalled.take() {
                stalled_server.wait_for_requests(2);
                let row = output
                    .next()
                    .expect("open output should receive the retried result");
                assert!(row.result.is_ok());
                drop(row);
                assert!(output.next().is_none());
            }
            assert_eq!(stalled_calls.load(Ordering::SeqCst), expected);
            assert_eq!(stalled_server.join().unwrap().len(), expected);
        }
    }
}
