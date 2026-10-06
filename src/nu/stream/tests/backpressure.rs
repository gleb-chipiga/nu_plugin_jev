//! Checks HTTP progress and cleanup without consuming a full annotation output.

use std::{
    sync::{Arc, atomic::AtomicUsize},
    time::Duration,
};

use nu_protocol::{Handlers, LabeledError, SignalAction, Signals, Value};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    api::client::{MeasuredSuccess, PreparedRequest},
    api::types::SystemOneResponse,
    config::ProxyPolicy,
    error::JevError,
    h2_fixture::{Response, ResponseGate, TestServer},
    nu::signals::register_with,
};

use super::super::{EvaluationResult, ScheduledOutput, SupervisorEvent, task_completion};
use super::{
    ApiKey, CancelHandle, HttpAttemptLimit, JevClient, JevClientPool, Ordering, answer, build,
    h2_fixture, runtime, setup, start, wait_for_admissions,
};

/// Uses more logical jobs than network slots so an unpolled waiter can reserve capacity.
const JOBS: usize = 3;
/// Saturates the shared budget with response bodies, not idle connections.
const CAPACITY: usize = 2;
/// Bounds test rendezvous independently of the deadline under examination.
const WATCHDOG: Duration = Duration::from_secs(5);

/// Holds one full output while its remaining evaluations occupy all HTTP slots.
/// The output stays open and unread: dropping it would test cancellation, not backpressure.
struct FullOutput {
    runtime: Arc<tokio::runtime::Runtime>,
    output: Option<ScheduledOutput>,
    client: JevClient,
    cancel: CancelHandle,
    gates: [ResponseGate; JOBS],
    events: mpsc::UnboundedReceiver<SupervisorEvent>,
    admissions: mpsc::UnboundedReceiver<()>,
    retry_waits: mpsc::Receiver<(std::time::Instant, Duration)>,
    reads: Arc<AtomicUsize>,
    resume_input: Option<mpsc::Sender<()>>,
    server: Option<TestServer>,
    root: String,
}

impl FullOutput {
    /// Streams fast rows first, then gates bodies for each remaining admitted row.
    fn new(unordered: bool, timeout: Duration, retry_last: bool) -> Self {
        let gates = std::array::from_fn(|_| ResponseGate::default());
        let held = gates.clone();
        let last_calls = AtomicUsize::new(0);
        let (root, server) = h2_fixture::serve_unbounded(move |_, request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let row = body["state"]
                .as_str()
                .and_then(|state| state.strip_prefix("row-"))
                .and_then(|index| index.parse::<usize>().ok());
            if retry_last
                && row == Some(2 * JOBS - 1)
                && last_calls.fetch_add(1, Ordering::SeqCst) == 0
            {
                let mut response = Response::json(503, serde_json::json!({}));
                response
                    .headers
                    .push(("retry-after-ms".into(), "20".into()));
                return response;
            }
            let mut response = Response::json(200, answer(1));
            response.body_gate = row
                .and_then(|row| row.checked_sub(JOBS))
                .and_then(|index| held.get(index))
                .cloned();
            response
        });
        let pool = JevClientPool::new(HttpAttemptLimit::new(CAPACITY).unwrap()).unwrap();
        let mut client = pool.for_policy(&ProxyPolicy::Direct).unwrap();
        let (admitted, admissions) = mpsc::unbounded_channel();
        client.observe_admission(admitted);
        let (retry_started, retry_waits) = mpsc::channel(1);
        client.observe_retry_waits(retry_started);
        let mut settings = setup(&root, JOBS);
        settings.client = client.clone();
        settings.unordered = unordered;
        settings.config.timeout = timeout;
        let (observed, events) = mpsc::unbounded_channel();
        settings.observer = Some(observed);
        let reads = Arc::new(AtomicUsize::new(0));
        let source_reads = Arc::clone(&reads);
        let (resume_input, mut resumed) = mpsc::channel(1);
        let mut indices = 0..10_000;
        let rows = std::iter::from_fn(move || {
            let index = indices.next()?;
            source_reads.fetch_add(1, Ordering::SeqCst);
            if index == JOBS || index == 2 * JOBS - 1 {
                // Only the dedicated source thread blocks. Stage held bodies after the
                // fast prefix, then the capacity waiter after both body slots are occupied.
                // Task spawn order alone cannot establish either HTTP admission boundary.
                resumed.blocking_recv()?;
            }
            Some(Value::test_string(format!("row-{index}")))
        });
        let runtime = runtime();
        let (cancel, signal) = CancelHandle::new();
        let output = start(
            Arc::clone(&runtime),
            settings,
            Box::new(rows),
            Box::new(build),
            None,
            (cancel.clone(), signal),
        )
        .unwrap();
        Self {
            runtime,
            output: Some(output),
            client,
            cancel,
            gates,
            events,
            admissions,
            retry_waits,
            reads,
            resume_input: Some(resume_input),
            server: Some(server),
            root,
        }
    }

    /// Confirms output is full and every process slot belongs to a held annotation body.
    async fn wait_saturated(&mut self) {
        wait_for_admissions(&mut self.admissions, JOBS).await;
        tokio::time::timeout(WATCHDOG, async {
            while self.output.as_ref().unwrap().receiver.len() != JOBS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("annotation did not fill its output");
        assert!(self.gates.iter().all(|gate| gate.arrivals() == 0));
        self.resume_input.as_ref().unwrap().send(()).await.unwrap();
        wait_for_admissions(&mut self.admissions, CAPACITY).await;
        self.gates[0].wait_for_arrivals(1).await;
        self.gates[1].wait_for_arrivals(1).await;
        self.resume_input.take().unwrap().send(()).await.unwrap();
        wait_for_admissions(&mut self.admissions, 1).await;
        // A spare slot would let the neighbor pass even with stalled HTTP futures.
        // Prove saturation first so its later progress requires A to release capacity.
        assert_eq!(self.client.free_attempt_slots(), 0);
        assert_eq!(self.reads.load(Ordering::SeqCst), 2 * JOBS);
        assert!(!self.output.as_ref().unwrap().receiver.is_closed());
    }

    /// Waits until the next ready outcome must block, checking the independent task bound.
    async fn wait_output_blocked(&mut self) {
        tokio::time::timeout(WATCHDOG, async {
            loop {
                match self
                    .events
                    .recv()
                    .await
                    .expect("supervisor observer closed")
                {
                    SupervisorEvent::TaskCount(count) => assert!(count <= JOBS),
                    SupervisorEvent::OutputBlocked => break,
                    SupervisorEvent::WorkStopped | SupervisorEvent::Stopped => {
                        panic!("supervisor stopped before output stall");
                    }
                }
            }
        })
        .await
        .expect("missing full-output rendezvous");
    }

    /// Starts a distinct request sharing the exact process budget and client pool.
    fn spawn_neighbor(&self) -> JoinHandle<Result<MeasuredSuccess<SystemOneResponse>, JevError>> {
        let client = self.client.clone();
        let mut config = setup(&self.root, 1).config;
        config.timeout = WATCHDOG;
        let request =
            PreparedRequest::new(build(&Value::test_string("neighbor")).unwrap()).unwrap();
        self.runtime.spawn(async move {
            let (_cancel, signal) = CancelHandle::new();
            client
                .system_one_prepared_measured(
                    &request,
                    &config,
                    &ApiKey::for_test("neighbor-key"),
                    signal,
                )
                .await
        })
    }

    /// Verifies task cleanup and eventual slot release without consuming queued outcomes.
    async fn wait_stopped(&mut self) {
        tokio::time::timeout(WATCHDOG, async {
            loop {
                match self
                    .events
                    .recv()
                    .await
                    .expect("supervisor observer closed")
                {
                    SupervisorEvent::TaskCount(count) => assert!(count <= JOBS),
                    SupervisorEvent::Stopped => break,
                    SupervisorEvent::OutputBlocked | SupervisorEvent::WorkStopped => {}
                }
            }
            assert_eq!(self.client.free_attempt_slots(), CAPACITY);
        })
        .await
        .expect("supervisor did not join cancelled tasks");
    }

    /// Closes local output and releases server gates before joining its blocking fixture thread.
    fn finish(mut self) -> Vec<h2_fixture::CapturedRequest> {
        drop(self.output.take());
        self.gates.iter().for_each(ResponseGate::release);
        self.server.take().unwrap().join().unwrap()
    }
}

impl Drop for FullOutput {
    /// Releases fixture waiters even when an assertion aborts the test early.
    fn drop(&mut self) {
        self.cancel.cancel();
        self.gates.iter().for_each(ResponseGate::release);
        // Closing a partially completed setup unblocks its dedicated input thread too.
        drop(self.resume_input.take());
    }
}

/// Body completion frees every occupied slot while output remains full and open.
#[test]
fn full_open_output_does_not_stall_a_neighbor_after_bodies_complete() {
    for unordered in [false, true] {
        let mut fixture = FullOutput::new(unordered, WATCHDOG, false);
        let runtime = Arc::clone(&fixture.runtime);
        runtime.block_on(async {
            fixture.wait_saturated().await;
            fixture.gates[0].release();
            fixture.wait_output_blocked().await;
            fixture.gates[2].wait_for_arrivals(1).await;
            assert_eq!(fixture.client.free_attempt_slots(), 0);
            let neighbor = fixture.spawn_neighbor();
            wait_for_admissions(&mut fixture.admissions, 1).await;
            assert!(!neighbor.is_finished());
            fixture.gates[1].release();
            fixture.gates[2].release();
            tokio::time::timeout(WATCHDOG, neighbor)
                .await
                .expect("backpressured annotation stalled its neighbor")
                .unwrap()
                .unwrap();
            assert_eq!(fixture.output.as_ref().unwrap().receiver.len(), JOBS);
            assert!(!fixture.output.as_ref().unwrap().receiver.is_closed());
            assert_eq!(fixture.reads.load(Ordering::SeqCst), 2 * JOBS);
            assert!(fixture.admissions.try_recv().is_err());
            drop(fixture.output.take());
            fixture.wait_stopped().await;
        });
        assert_eq!(fixture.finish().len(), 2 * JOBS + 1);
    }
}

/// Deadlines release occupied slots even when the supervisor cannot send its next row.
#[test]
fn full_open_output_does_not_stall_a_neighbor_after_deadlines() {
    for unordered in [false, true] {
        let mut fixture = FullOutput::new(unordered, Duration::from_secs(2), false);
        let runtime = Arc::clone(&fixture.runtime);
        runtime.block_on(async {
            fixture.wait_saturated().await;
            fixture.gates[0].release();
            fixture.wait_output_blocked().await;
            fixture.gates[2].wait_for_arrivals(1).await;
            assert_eq!(fixture.client.free_attempt_slots(), 0);
            let neighbor = fixture.spawn_neighbor();
            wait_for_admissions(&mut fixture.admissions, 1).await;
            assert!(!neighbor.is_finished());
            tokio::time::timeout(WATCHDOG, neighbor)
                .await
                .expect("unpolled deadlines retained HTTP capacity")
                .unwrap()
                .unwrap();
            assert_eq!(fixture.output.as_ref().unwrap().receiver.len(), JOBS);
            assert!(!fixture.output.as_ref().unwrap().receiver.is_closed());
            assert_eq!(fixture.reads.load(Ordering::SeqCst), 2 * JOBS);
            drop(fixture.output.take());
            fixture.wait_stopped().await;
        });
        assert_eq!(fixture.finish().len(), 2 * JOBS + 1);
    }
}

/// A retry and its backoff run inside the task while completed output is blocked.
#[test]
fn full_open_output_keeps_retrying_dispatched_evaluations() {
    for unordered in [false, true] {
        let mut fixture = FullOutput::new(unordered, WATCHDOG, true);
        let runtime = Arc::clone(&fixture.runtime);
        runtime.block_on(async {
            fixture.wait_saturated().await;
            fixture.gates[0].release();
            fixture.wait_output_blocked().await;
            let (_, delay) = tokio::time::timeout(WATCHDOG, fixture.retry_waits.recv())
                .await
                .expect("retry did not start with full output")
                .unwrap();
            assert_eq!(delay, Duration::from_millis(20));
            fixture.gates[2].wait_for_arrivals(1).await;
            assert_eq!(fixture.output.as_ref().unwrap().receiver.len(), JOBS);
            assert_eq!(fixture.reads.load(Ordering::SeqCst), 2 * JOBS);
            drop(fixture.output.take());
            fixture.wait_stopped().await;
        });
        assert_eq!(fixture.finish().len(), 2 * JOBS + 1);
    }
}

/// Interrupting a full but open output aborts and joins its HTTP tasks before closure.
#[test]
fn full_open_output_cancellation_joins_tasks_and_reclaims_capacity() {
    for unordered in [false, true] {
        let mut fixture = FullOutput::new(unordered, WATCHDOG, false);
        let runtime = Arc::clone(&fixture.runtime);
        runtime.block_on(async {
            fixture.wait_saturated().await;
            fixture.gates[0].release();
            fixture.wait_output_blocked().await;
            fixture.gates[2].wait_for_arrivals(1).await;
            assert_eq!(fixture.client.free_attempt_slots(), 0);
            fixture.cancel.cancel();
            fixture.wait_stopped().await;
            tokio::time::timeout(WATCHDOG, async {
                while !fixture.output.as_ref().unwrap().receiver.is_closed() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("cancelled supervisor retained open output");
            let neighbor = fixture.spawn_neighbor();
            neighbor.await.unwrap().unwrap();
            assert_eq!(fixture.reads.load(Ordering::SeqCst), 2 * JOBS);
            drop(fixture.output.take());
        });
        assert_eq!(fixture.finish().len(), 2 * JOBS + 1);
    }
}

/// Fast joined results retain row credits behind a stalled first ordered row.
#[test]
fn independent_tasks_preserve_the_ordered_row_window() {
    let first = ResponseGate::default();
    let held = first.clone();
    let (root, server) = h2_fixture::serve_unbounded(move |_, request| {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let mut response = Response::json(200, answer(1));
        if body["state"] == "row-0" {
            response.body_gate = Some(held.clone());
        }
        response
    });
    let reads = Arc::new(AtomicUsize::new(0));
    let source_reads = Arc::clone(&reads);
    let rows = (0..10_000).map(move |index| {
        source_reads.fetch_add(1, Ordering::SeqCst);
        Value::test_string(format!("row-{index}"))
    });
    let (observed, mut events) = mpsc::unbounded_channel();
    let mut settings = setup(&root, JOBS);
    settings.observer = Some(observed);
    let runtime = runtime();
    let output = start(
        Arc::clone(&runtime),
        settings,
        Box::new(rows),
        Box::new(build),
        None,
        CancelHandle::new(),
    )
    .unwrap();
    runtime.block_on(async {
        first.wait_for_arrivals(1).await;
        tokio::time::timeout(WATCHDOG, async {
            while server.request_count() < 2 * JOBS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("later evaluations did not progress behind the first row");
        while let Ok(event) = events.try_recv() {
            if let SupervisorEvent::TaskCount(count) = event {
                assert!(count <= JOBS);
            }
        }
        assert_eq!(reads.load(Ordering::SeqCst), 2 * JOBS);
        assert!(output.receiver.is_empty());
    });
    drop(output);
    first.release();
    assert_eq!(server.join().unwrap().len(), 2 * JOBS);
}

/// A returned stream retains interrupt registration until it is dropped.
#[test]
fn returned_stream_retains_its_interrupt_guard() {
    let handlers = Handlers::new();
    let retained = Arc::new(());
    let weak = Arc::downgrade(&retained);
    let registration = register_with(
        &Signals::empty(),
        nu_protocol::Span::test_data(),
        |handler| {
            handlers
                .register(Box::new(move |action| {
                    let _retained = &retained;
                    handler(action);
                }))
                .map_err(LabeledError::from)
        },
    )
    .unwrap();
    let signal = registration.signal.clone();
    let cancel = registration.cancel.clone();
    let (source, receiver) = std::sync::mpsc::channel::<Value>();
    let output = start(
        runtime(),
        setup("http://127.0.0.1:9", 1),
        Box::new(std::iter::from_fn(move || receiver.recv().ok())),
        Box::new(build),
        Some(registration.guard),
        (registration.cancel, registration.signal),
    )
    .unwrap();
    assert!(!signal.is_cancelled());
    assert!(weak.upgrade().is_some());
    handlers.run(SignalAction::Interrupt);
    assert!(signal.is_cancelled());
    drop(output);
    assert!(weak.upgrade().is_none());
    drop(source);
    drop(cancel);
}

/// Task panics and unexpected aborts retain their row identity and leave no routing entry.
#[test]
fn failed_tasks_preserve_routing_without_exposing_join_errors() {
    runtime().block_on(async {
        for abort in [false, true] {
            let mut tasks = tokio::task::JoinSet::<EvaluationResult>::new();
            let task = tasks.spawn(async move {
                if abort {
                    std::future::pending::<EvaluationResult>().await
                } else {
                    panic!("task fixture panic");
                }
            });
            let request = build(&Value::test_string("row"));
            let key = crate::nu::cache::RequestKey::new(
                &"http://fixture.invalid".parse().unwrap(),
                &request.unwrap(),
            );
            let mut routes = [(task.id(), (key.clone(), "local-request".into()))].into();
            if abort {
                task.abort();
            }
            let outcome = tasks.join_next_with_id().await.unwrap();
            let (returned_key, request_id, result) = task_completion(outcome, &mut routes);
            assert_eq!(returned_key, key);
            assert_eq!(request_id, "local-request");
            let error = result.err().unwrap();
            assert_eq!(error.kind_name(), "transport");
            assert_eq!(error.to_string(), "Jev evaluation task failed");
            assert!(routes.is_empty());
            assert!(tasks.is_empty());
        }
    });
}

/// Fail-fast cleans up sibling tasks before any consumer reads or drops the terminal outcome.
#[test]
fn terminal_failure_cleans_up_without_output_consumption() {
    let peers = ResponseGate::default();
    let body = ResponseGate::default();
    let held = body.clone();
    let (root, server) = h2_fixture::serve_unbounded(move |_, request| {
        let state: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        if state["state"] == "neighbor" {
            return Response::json(200, answer(1));
        }
        peers.rendezvous(2);
        if state["state"] == "row-1" {
            Response::json(400, serde_json::json!({}))
        } else {
            let mut response = Response::json(200, answer(1));
            response.body_gate = Some(held.clone());
            response
        }
    });
    let pool = JevClientPool::new(HttpAttemptLimit::new(CAPACITY).unwrap()).unwrap();
    let client = pool.for_policy(&ProxyPolicy::Direct).unwrap();
    let mut settings = setup(&root, 2);
    settings.client = client.clone();
    let (observed, mut events) = mpsc::unbounded_channel();
    settings.observer = Some(observed);
    let runtime = runtime();
    let output = start(
        Arc::clone(&runtime),
        settings,
        Box::new([Value::test_string("row-0"), Value::test_string("row-1")].into_iter()),
        Box::new(build),
        None,
        CancelHandle::new(),
    )
    .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(WATCHDOG, async {
            while !matches!(
                events.recv().await.expect("supervisor observer closed"),
                SupervisorEvent::Stopped
            ) {}
        })
        .await
        .expect("terminal failure did not clean up sibling tasks");
        assert_eq!(client.free_attempt_slots(), CAPACITY);
        assert_eq!(output.receiver.len(), 1);
        let request = build(&Value::test_string("neighbor")).unwrap();
        let (_cancel, signal) = CancelHandle::new();
        client
            .system_one(
                &request,
                &setup(&root, 1).config,
                &ApiKey::for_test("local-key"),
                signal,
            )
            .await
            .unwrap();
    });
    drop(output);
    body.release();
    assert_eq!(server.join().unwrap().len(), 3);
}
