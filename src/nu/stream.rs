//! Schedules independent row requests with bounded admission and invocation-local reuse.
//!
//! At most `2 * jobs` source rows are admitted, including queued, active, ordered,
//! and output-buffered rows. Admission credit is returned only when downstream
//! consumes an outcome. Each bridge channel holds at most `jobs` items, and no
//! Tokio worker reads Nu input or performs blocking channel operations. Ordered
//! output may stall behind an early request; unordered output may return sooner.
//!
//! Completed successes use a separate bounded LRU. Its approximate key/result/
//! provenance weight is not a process-RSS cap or an invocation-wide guarantee
//! of once-only evaluation: eviction and oversized bypass permit new requests.
//! One array passed to `jev ask` is a different, intentional shared state.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    future::pending,
    sync::Arc,
    thread,
};

use futures::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use nu_protocol::{HandlerGuard, Value};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

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
    /// Shared HTTP connection pool for this invocation.
    pub(crate) client: JevClient,
    /// Validated caller-scoped model, transport, and scheduling settings.
    pub(crate) config: InvocationConfig,
    /// Caller credential retained only for this live invocation.
    pub(crate) key: ApiKey,
    /// Emits whichever completed row is ready without sequence buffering.
    pub(crate) unordered: bool,
    /// Stops on the first row failure instead of returning error outcomes.
    pub(crate) fail_fast: bool,
}

/// Owns a source row, its result, and admission credit until downstream consumes it.
pub(crate) struct RowOutcome {
    /// Input sequence number, independent of completion order.
    pub(crate) sequence: u64,
    /// Original row preserved for annotation or error policies.
    pub(crate) source: Value,
    /// Shared successful result or classified row failure.
    pub(crate) result: Result<Arc<SharedResponse>, Arc<JevError>>,
    /// Row admission credit released only after this outcome is consumed.
    _permit: OwnedSemaphorePermit,
}

/// Receives bounded outcomes and cancels the invocation when dropped early.
pub(crate) struct ScheduledOutput {
    receiver: mpsc::Receiver<RowOutcome>,
    cancel: CancelHandle,
    _handler: Option<HandlerGuard>,
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
    /// Stops input admission and abandons pending HTTP operations.
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

/// Identifies one completed logical HTTP evaluation and its shared local identity.
type Completion = (
    RequestKey,
    String,
    Result<MeasuredSuccess<SystemOneResponse>, JevError>,
);

/// Starts a bounded row stream without reading its first input value on the caller thread.
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
    let window = jobs * 2;
    let admission = Arc::new(Semaphore::new(window));
    let (input_sender, input_receiver) = mpsc::channel(jobs);
    let (output_sender, output_receiver) = mpsc::channel(jobs);
    let (cancel, signal) = cancellation;
    let producer_runtime = Arc::clone(&runtime);
    let producer_signal = signal.clone();
    let producer_admission = Arc::clone(&admission);
    let service_root: Arc<str> = Arc::from(setup.config.base_url.as_str());
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

/// Reads rows only after admission credit is available, off Tokio worker threads.
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
        let Some(source) = input.next() else { break };
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
        if sender.blocking_send(row).is_err() {
            break;
        }
        sequence = sequence.wrapping_add(1);
    }
}

/// Runs at most `jobs` unique evaluations while coalescing admitted duplicates.
async fn supervise(
    setup: StreamSetup,
    mut input: mpsc::Receiver<InputRow>,
    output: mpsc::Sender<RowOutcome>,
    mut signal: CancelSignal,
    cancel: CancelHandle,
) {
    let StreamSetup {
        client,
        config,
        key,
        unordered,
        fail_fast,
    } = setup;
    let jobs = config.jobs.expect("table settings include jobs").get();
    let mut cache = CompletedCache::new(config.cache.expect("table settings include cache limits"));
    let config = Arc::new(config);
    let key = Arc::new(key);
    let mut pending_groups: VecDeque<PendingGroup> = VecDeque::new();
    let mut in_flight: HashMap<RequestKey, Vec<InputRow>> = HashMap::new();
    let mut active: FuturesUnordered<BoxFuture<'static, Completion>> = FuturesUnordered::new();
    let mut ordered: BTreeMap<u64, RowOutcome> = BTreeMap::new();
    let mut next_sequence = 0_u64;
    let mut input_closed = false;

    loop {
        while active.len() < jobs {
            let Some(group) = pending_groups.pop_front() else {
                break;
            };
            let PendingGroup {
                key: request_key,
                request,
                rows,
            } = group;
            let request_id = next_request_id();
            in_flight.insert(request_key.clone(), rows);
            let client = client.clone();
            let config = Arc::clone(&config);
            let key = Arc::clone(&key);
            let request_signal = signal.clone();
            let trace_id = request_id.clone();
            active.push(
                async move {
                    let result = crate::tracing::trace_evaluation(
                        "annotate",
                        &trace_id,
                        client.system_one_prepared_measured(
                            &request,
                            &config,
                            &key,
                            request_signal,
                        ),
                    )
                    .await;
                    (request_key, request_id, result)
                }
                .boxed(),
            );
        }
        if input_closed && pending_groups.is_empty() && active.is_empty() {
            break;
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
                    active.next().await
                }
            }
            .fuse();
            let closed = output.closed().fuse();
            let interrupted = signal.cancelled().fuse();
            futures::pin_mut!(next_row, completion, closed, interrupted);
            futures::select_biased! {
                _ = interrupted => Event::Interrupted,
                _ = closed => Event::OutputClosed,
                finished = completion => Event::Completed(finished),
                row = next_row => Event::Row(row),
            }
        };
        match event {
            Event::Interrupted => {
                tracing::info!(active_evaluations = active.len(), "annotation interrupted");
                break;
            }
            Event::OutputClosed => {
                tracing::info!(
                    active_evaluations = active.len(),
                    "annotation output closed"
                );
                break;
            }
            Event::Row(None) => input_closed = true,
            Event::Row(Some(mut row)) => {
                let (request, request_key) =
                    match row.request.take().expect("producer prepares every row") {
                        Ok(request) => request,
                        Err(error) => {
                            if !emit(
                                row.finish(Err(Arc::new(error))),
                                &output,
                                &mut ordered,
                                &mut next_sequence,
                                unordered,
                                fail_fast,
                                &mut signal,
                            )
                            .await
                            {
                                break;
                            }
                            continue;
                        }
                    };
                if let Some(cached) = cache.get(&request_key) {
                    tracing::debug!(
                        request_id = %cached.request_id,
                        row_sequence = row.sequence,
                        "completed Jev result cache hit"
                    );
                    if !emit(
                        row.finish(Ok(cached)),
                        &output,
                        &mut ordered,
                        &mut next_sequence,
                        unordered,
                        fail_fast,
                        &mut signal,
                    )
                    .await
                    {
                        break;
                    }
                } else if let Some(waiters) = in_flight.get_mut(&request_key) {
                    tracing::debug!(
                        row_sequence = row.sequence,
                        "joined in-flight Jev evaluation"
                    );
                    waiters.push(row);
                } else if let Some(group) = pending_groups
                    .iter_mut()
                    .find(|group| group.key == request_key)
                {
                    tracing::debug!(row_sequence = row.sequence, "joined queued Jev evaluation");
                    group.rows.push(row);
                } else {
                    pending_groups.push_back(PendingGroup {
                        key: request_key,
                        request,
                        rows: vec![row],
                    });
                }
            }
            Event::Completed(Some((request_key, request_id, result))) => {
                let waiters = in_flight
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
                        cache.insert(request_key, Arc::clone(&result));
                        Ok(result)
                    }
                    Err(error) => Err(Arc::new(error)),
                };
                let mut keep_running = true;
                for row in waiters {
                    if !emit(
                        row.finish(shared.clone()),
                        &output,
                        &mut ordered,
                        &mut next_sequence,
                        unordered,
                        fail_fast,
                        &mut signal,
                    )
                    .await
                    {
                        keep_running = false;
                        break;
                    }
                }
                if !keep_running {
                    break;
                }
            }
            Event::Completed(None) => break,
        }
    }
    cancel.cancel();
}

/// Describes which bounded source of work became ready in the supervisor.
enum Event {
    Row(Option<InputRow>),
    Completed(Option<Completion>),
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
) -> bool {
    let terminal =
        row.result.as_ref().err().is_some_and(|error| {
            fail_fast || matches!(error.as_ref(), JevError::FieldCollision(_))
        });
    if unordered || terminal {
        return send_or_cancel(output, row, signal).await && !terminal;
    }
    ordered.insert(row.sequence, row);
    while let Some(ready) = ordered.remove(next_sequence) {
        if !send_or_cancel(output, ready, signal).await {
            return false;
        }
        *next_sequence = next_sequence.wrapping_add(1);
    }
    true
}

/// Stops a backpressured send immediately when the invocation is cancelled.
async fn send_or_cancel(
    output: &mpsc::Sender<RowOutcome>,
    row: RowOutcome,
    signal: &mut CancelSignal,
) -> bool {
    let cancelled = signal.cancelled().fuse();
    let send = output.send(row).fuse();
    futures::pin_mut!(cancelled, send);
    futures::select_biased! {
        _ = cancelled => false,
        result = send => result.is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
        time::{Duration, Instant},
    };

    use nu_protocol::Value;
    use serde_json::json;

    use crate::{
        api::{
            cancel::CancelHandle,
            client::JevClient,
            types::{Question, SystemOneRequest},
        },
        commands::tests::serve,
        config::{ApiKey, ConfigScope, ConfigSources, resolve},
        nu::value::to_json,
    };

    use super::{InputRow, StreamSetup, emit, start};

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
            assert!(
                !tokio::time::timeout(Duration::from_millis(250), task)
                    .await
                    .expect("cancelled send must stop promptly")
                    .unwrap()
            );
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
        assert_eq!(server.join().unwrap().len(), 2);
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

    /// Serves two requests concurrently and intentionally delays state zero.
    fn serve_out_of_order() -> (String, thread::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let handle = thread::spawn(move || {
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    let (mut stream, _) = listener.accept().unwrap();
                    let active = Arc::clone(&active);
                    let peak = Arc::clone(&peak);
                    let barrier = Arc::clone(&barrier);
                    thread::spawn(move || {
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        let mut length = 0;
                        loop {
                            line.clear();
                            reader.read_line(&mut line).unwrap();
                            if line == "\r\n" {
                                break;
                            }
                            if let Some((name, value)) = line.split_once(':')
                                && name.eq_ignore_ascii_case("content-length")
                            {
                                length = value.trim().parse().unwrap();
                            }
                        }
                        let mut body = vec![0; length];
                        reader.read_exact(&mut body).unwrap();
                        let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                        let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(count, Ordering::SeqCst);
                        barrier.wait();
                        if request["state"] == 0 {
                            thread::sleep(Duration::from_millis(120));
                        }
                        let response = answer(1).to_string();
                        let _ = write!(
                            stream,
                            concat!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n",
                                "Content-Length: {}\r\nConnection: close\r\n\r\n{response}"
                            ),
                            response.len(),
                            response = response
                        );
                        active.fetch_sub(1, Ordering::SeqCst);
                    })
                })
                .collect();
            workers
                .into_iter()
                .for_each(|worker| worker.join().unwrap());
            peak.load(Ordering::SeqCst)
        });
        (root, handle)
    }

    /// Delays one local response long enough for duplicate admission or failure bypass.
    fn serve_delayed_one() -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => return,
                }
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let mut length = 0;
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            if reader.read_exact(&mut body).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(120));
            let response = answer(1).to_string();
            let _ = write!(
                stream,
                concat!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n",
                    "Content-Length: {}\r\nConnection: close\r\n\r\n{response}"
                ),
                response.len(),
                response = response
            );
        });
        (root, handle)
    }

    /// Returns one retryable status before a valid response for the same logical request.
    fn serve_retry_once() -> (String, thread::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            (0..2)
                .map(|attempt| {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let mut length = 0;
                    loop {
                        line.clear();
                        reader.read_line(&mut line).unwrap();
                        if line == "\r\n" {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':')
                            && name.eq_ignore_ascii_case("content-length")
                        {
                            length = value.trim().parse().unwrap();
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    if attempt == 0 {
                        let _ = write!(
                            stream,
                            concat!(
                                "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\n",
                                "Content-Length: 0\r\nConnection: close\r\n\r\n"
                            )
                        );
                        thread::sleep(Duration::from_millis(40));
                    } else {
                        let response = answer(1).to_string();
                        let _ = write!(
                            stream,
                            concat!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n",
                                "Content-Length: {}\r\nConnection: close\r\n\r\n{response}"
                            ),
                            response.len(),
                            response = response
                        );
                    }
                    1_usize
                })
                .sum()
        });
        (root, handle)
    }

    /// Holds one retryable failure long enough to test output-drop cancellation.
    fn serve_long_retry() -> (String, Arc<AtomicUsize>, thread::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let server_calls = Arc::clone(&calls);
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_millis(1400);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            continue;
                        }
                        let mut length = 0;
                        loop {
                            line.clear();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                break;
                            }
                            if line == "\r\n" {
                                break;
                            }
                            if let Some((name, value)) = line.split_once(':')
                                && name.eq_ignore_ascii_case("content-length")
                            {
                                length = value.trim().parse().unwrap();
                            }
                        }
                        let mut body = vec![0; length];
                        if reader.read_exact(&mut body).is_err() {
                            continue;
                        }
                        server_calls.fetch_add(1, Ordering::SeqCst);
                        let _ = write!(
                            stream,
                            concat!(
                                "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 1\r\n",
                                "Content-Length: 0\r\nConnection: close\r\n\r\n"
                            )
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("retry mock accept failed: {error}"),
                }
            }
            server_calls.load(Ordering::SeqCst)
        });
        (root, calls, handle)
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
        let (base_url, server) = serve_delayed_one();
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
        server.join().unwrap();
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
        assert_eq!(server.join().unwrap(), 2);
    }

    /// A terminal row failure bypasses a slow earlier slot in ordered mode.
    #[test]
    fn fail_fast_bypasses_stalled_ordered_row() {
        let (base_url, server) = serve_delayed_one();
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
        let (base_url, server) = serve_delayed_one();
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
        let (stalled_url, stalled_calls, stalled_server) = serve_long_retry();
        let good = answer(1);
        let (healthy_url, healthy_server) = serve(vec![good]);
        let runtime = runtime();
        let client = JevClient::new().unwrap();
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
        let deadline = Instant::now() + Duration::from_secs(1);
        while stalled_calls.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
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
        drop(stalled);
        let rows: Vec<_> = healthy.collect();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].result.is_ok());
        assert_eq!(
            healthy_server.join().unwrap()[0].authorization.as_deref(),
            Some("Bearer healthy-key")
        );
        assert_eq!(stalled_server.join().unwrap(), 1);
    }
}
