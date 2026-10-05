//! Proves observed terminal failures stop local work before waiting for output capacity.

use std::{
    sync::{Arc, Weak, atomic::AtomicUsize},
    time::{Duration, Instant},
};

use nu_protocol::Value;
use tokio::sync::{mpsc, oneshot};

use crate::{
    api::cancel::CancelSignal,
    error::JevError,
    h2_fixture::{CapturedRequest, Response, ResponseGate, TestServer},
};

use super::super::{
    InputRow, Routing, ScheduledOutput, SupervisorEvent, SupervisorState, finish_request,
};
use super::{
    ApiKey, CancelHandle, HttpAttemptLimit, JevClient, JevClientPool, Ordering, answer, build,
    h2_fixture, runtime, setup, start, wait_for_admissions,
};

/// Fills output and retains enough logical jobs for a failure, body, waiter, and backoff.
const JOBS: usize = 4;
/// Holds the failed response and a sibling body while another evaluation awaits capacity.
const CAPACITY: usize = 2;
/// Bounds rendezvous independently of request deadlines and retry guidance.
const WATCHDOG: Duration = Duration::from_secs(5);

/// Holds a synchronous source call until the test explicitly releases its returned row.
struct LateInput {
    sender: std::sync::mpsc::Sender<Value>,
    entered: oneshot::Receiver<()>,
    retained: Weak<()>,
}

/// Separates HTTP cleanup from delivery while preserving the live output and source handles.
struct TerminalOutput {
    runtime: Arc<tokio::runtime::Runtime>,
    output: Option<ScheduledOutput>,
    client: JevClient,
    cancel: CancelHandle,
    signal: CancelSignal,
    failure: ResponseGate,
    body: ResponseGate,
    events: mpsc::UnboundedReceiver<SupervisorEvent>,
    admissions: mpsc::UnboundedReceiver<()>,
    retry_waits: mpsc::Receiver<(Instant, Duration)>,
    reads: Arc<AtomicUsize>,
    builds: Arc<AtomicUsize>,
    late: Option<LateInput>,
    server: Option<TestServer>,
    root: String,
}

impl TerminalOutput {
    /// Gates one failure while siblings reach active, retrying, and queued phases.
    fn new(unordered: bool, blocked_input: bool) -> Self {
        // Rows 0..3 fill output. Rows 4..7 then occupy failure, retry, body, and
        // capacity-wait phases; the two HTTP slots belong to the failure and held body.
        let failure = ResponseGate::default();
        let body = ResponseGate::default();
        let held_failure = failure.clone();
        let held_body = body.clone();
        let retry_calls = AtomicUsize::new(0);
        let (root, server) =
            h2_fixture::serve_unbounded(move |_, request| match request_state(request).as_str() {
                "row-4" => {
                    let mut response = Response::json(400, serde_json::json!({}));
                    response.header_gate = Some(held_failure.clone());
                    response
                }
                "row-5" if retry_calls.fetch_add(1, Ordering::SeqCst) == 0 => {
                    let mut response = Response::json(503, serde_json::json!({}));
                    response
                        .headers
                        .push(("retry-after-ms".into(), "2000".into()));
                    response
                }
                "row-6" | "row-7" => {
                    let mut response = Response::json(200, answer(1));
                    response.body_gate = Some(held_body.clone());
                    response
                }
                _ => Response::json(200, answer(1)),
            });
        let pool = JevClientPool::new(HttpAttemptLimit::new(CAPACITY).unwrap()).unwrap();
        let mut client = pool
            .for_policy(&crate::config::ProxyPolicy::Direct)
            .unwrap();
        let (admitted, admissions) = mpsc::unbounded_channel();
        client.observe_admission(admitted);
        let (retry_started, retry_waits) = mpsc::channel(1);
        client.observe_retry_waits(retry_started);
        let mut settings = setup(&root, JOBS);
        settings.client = client.clone();
        settings.unordered = unordered;
        // Keep deadlines longer than WATCHDOG: prompt cleanup must come from terminal
        // cancellation, not from sibling timeouts coincidentally freeing the slots.
        settings.config.timeout = Duration::from_secs(30);
        let (observed, events) = mpsc::unbounded_channel();
        settings.observer = Some(observed);
        let reads = Arc::new(AtomicUsize::new(0));
        let source_reads = Arc::clone(&reads);
        let (rows, late): (Box<dyn Iterator<Item = Value> + Send>, _) = if blocked_input {
            let (sender, receiver) = std::sync::mpsc::channel();
            let (entered, entered_receiver) = oneshot::channel();
            let mut entered = Some(entered);
            let retained = Arc::new(());
            let weak = Arc::downgrade(&retained);
            // The eighth admitted row blocks in next(), retaining the last local credit.
            // No HTTP request or builder call exists for it unless the source returns.
            let mut prefix = (0..7).map(|index| Value::test_string(format!("row-{index}")));
            let rows = std::iter::from_fn(move || {
                let _retained = &retained;
                source_reads.fetch_add(1, Ordering::SeqCst);
                prefix.next().or_else(|| {
                    if let Some(entered) = entered.take() {
                        let _ = entered.send(());
                    }
                    receiver.recv().ok()
                })
            });
            (
                Box::new(rows),
                Some(LateInput {
                    sender,
                    entered: entered_receiver,
                    retained: weak,
                }),
            )
        } else {
            let rows = (0..10_000).map(move |index| {
                source_reads.fetch_add(1, Ordering::SeqCst);
                Value::test_string(format!("row-{index}"))
            });
            (Box::new(rows), None)
        };
        let builds = Arc::new(AtomicUsize::new(0));
        let source_builds = Arc::clone(&builds);
        let runtime = runtime();
        let (cancel, signal) = CancelHandle::new();
        let output = start(
            Arc::clone(&runtime),
            settings,
            rows,
            Box::new(move |row| {
                source_builds.fetch_add(1, Ordering::SeqCst);
                build(row)
            }),
            None,
            (cancel.clone(), signal.clone()),
        )
        .unwrap();
        Self {
            runtime,
            output: Some(output),
            client,
            cancel,
            signal,
            failure,
            body,
            events,
            admissions,
            retry_waits,
            reads,
            builds,
            late,
            server: Some(server),
            root,
        }
    }

    /// Confirms a full open output with all slots occupied before releasing the failure.
    async fn wait_saturated(&mut self) -> Instant {
        let evaluations = if self.late.is_some() { 7 } else { 2 * JOBS };
        wait_for_admissions(&mut self.admissions, evaluations).await;
        self.failure.wait_for_arrivals(1).await;
        self.body.wait_for_arrivals(1).await;
        let (due, delay) = tokio::time::timeout(WATCHDOG, self.retry_waits.recv())
            .await
            .expect("missing sibling retry pause")
            .unwrap();
        assert_eq!(delay, Duration::from_secs(2));
        if let Some(late) = &mut self.late {
            tokio::time::timeout(WATCHDOG, &mut late.entered)
                .await
                .expect("source did not enter its blocked next call")
                .unwrap();
        }
        tokio::time::timeout(WATCHDOG, async {
            while self.output.as_ref().unwrap().receiver.len() != JOBS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("output did not fill before failure release");
        assert_eq!(self.client.free_attempt_slots(), 0);
        assert_eq!(self.reads.load(Ordering::SeqCst), 2 * JOBS);
        assert_eq!(self.builds.load(Ordering::SeqCst), evaluations);
        assert!(!self.output.as_ref().unwrap().receiver.is_closed());
        assert!(
            Instant::now() < due,
            "retry pause ended before controlled failure"
        );
        due
    }

    /// Waits for task and routing cleanup without consuming or dropping any output.
    async fn wait_work_stopped(&mut self) {
        tokio::time::timeout(WATCHDOG, async {
            loop {
                match self
                    .events
                    .recv()
                    .await
                    .expect("supervisor observer closed")
                {
                    SupervisorEvent::TaskCount(count) => assert!(count <= JOBS),
                    SupervisorEvent::WorkStopped => break,
                    SupervisorEvent::Stopped => panic!("delivery ended before cleanup boundary"),
                    SupervisorEvent::OutputBlocked => {}
                }
            }
        })
        .await
        .expect("HTTP cleanup waited for output capacity or source return");
        assert_eq!(self.client.free_attempt_slots(), CAPACITY);
        assert_eq!(self.output.as_ref().unwrap().receiver.len(), JOBS);
        assert!(!self.output.as_ref().unwrap().receiver.is_closed());
        assert!(
            !self.signal.is_cancelled(),
            "cleanup cancelled saved error delivery"
        );
    }

    /// Shows an independent request can use reclaimed capacity before error delivery.
    async fn neighbor(&self) {
        let (_cancel, signal) = CancelHandle::new();
        self.client
            .system_one(
                &build(&Value::test_string("neighbor")).unwrap(),
                &setup(&self.root, 1).config,
                &ApiKey::for_test("neighbor-key"),
                signal,
            )
            .await
            .expect("healthy neighbor could not use reclaimed capacity");
    }

    /// Observes final supervisor termination after consuming, dropping, or interrupting output.
    async fn wait_stopped(&mut self) {
        tokio::time::timeout(WATCHDOG, async {
            while !matches!(
                self.events
                    .recv()
                    .await
                    .expect("supervisor observer closed"),
                SupervisorEvent::Stopped
            ) {}
        })
        .await
        .expect("saved-error delivery did not terminate");
        assert_eq!(self.client.free_attempt_slots(), CAPACITY);
    }

    /// Drains the prior successes and the original error, with no later row outcomes.
    async fn consume_original_error(&mut self) {
        let output = self.output.as_mut().unwrap();
        let mut successes = 0;
        let mut failed = false;
        tokio::time::timeout(WATCHDOG, async {
            while let Some(row) = output.receiver.recv().await {
                match &row.result {
                    Ok(_) => {
                        assert!(!failed, "success followed the terminal error");
                        successes += 1;
                    }
                    Err(error) => {
                        assert!(!failed);
                        assert_eq!(row.sequence, 4);
                        assert_eq!(row.source, Value::test_string("row-4"));
                        assert!(matches!(error.as_ref(), JevError::Http { status: 400 }));
                        failed = true;
                    }
                }
            }
        })
        .await
        .expect("error delivery did not finish after output consumption");
        assert_eq!(successes, JOBS);
        assert!(failed, "original error was lost during cleanup");
        self.wait_stopped().await;
    }

    /// Releases only fixture-owned gates and source handles before stopping the local server.
    fn finish(mut self) -> Vec<CapturedRequest> {
        drop(self.output.take());
        drop(self.late.take());
        self.failure.release();
        self.body.release();
        self.server.take().unwrap().join().unwrap()
    }
}

impl Drop for TerminalOutput {
    /// Unblocks test-only sources and server waits when an assertion ends the test early.
    fn drop(&mut self) {
        self.cancel.cancel();
        self.failure.release();
        self.body.release();
        drop(self.late.take());
    }
}

/// Extracts only synthetic fixture state for deterministic request accounting.
fn request_state(request: &CapturedRequest) -> String {
    serde_json::from_slice::<serde_json::Value>(&request.body).unwrap()["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Waits beyond the reported retry boundary only after confirmed worker cleanup.
/// This rules out another retry without mistaking already-started remote work for late dispatch.
async fn pass_retry_boundary(due: Instant) {
    tokio::time::sleep(due.saturating_duration_since(Instant::now())).await;
    tokio::task::yield_now().await;
}

/// Active bodies, queued acquisition, and retry work are stopped before error delivery.
#[test]
fn terminal_cleanup_precedes_delivery_with_full_open_output() {
    for unordered in [false, true] {
        let mut fixture = TerminalOutput::new(unordered, false);
        let runtime = Arc::clone(&fixture.runtime);
        runtime.block_on(async {
            let retry_due = fixture.wait_saturated().await;
            fixture.failure.release();
            fixture.wait_work_stopped().await;
            fixture.neighbor().await;
            pass_retry_boundary(retry_due).await;
            assert_eq!(fixture.reads.load(Ordering::SeqCst), 2 * JOBS);
            fixture.consume_original_error().await;
        });
        let requests = fixture.finish();
        // A slot waiter might already have sent while the failure was being observed.
        // Assert only the controlled retry and new-row boundaries after proven cleanup.
        assert_eq!(
            requests
                .iter()
                .filter(|r| request_state(r) == "row-5")
                .count(),
            1
        );
        assert!(!requests.iter().any(|r| request_state(r) == "row-8"));
    }
}

/// Closing output after cleanup ends the saved-error wait without resuming local work.
#[test]
fn output_drop_after_terminal_cleanup_ends_error_delivery() {
    for unordered in [false, true] {
        let mut fixture = TerminalOutput::new(unordered, false);
        let runtime = Arc::clone(&fixture.runtime);
        runtime.block_on(async {
            let retry_due = fixture.wait_saturated().await;
            fixture.failure.release();
            fixture.wait_work_stopped().await;
            drop(fixture.output.take());
            fixture.wait_stopped().await;
            fixture.neighbor().await;
            pass_retry_boundary(retry_due).await;
            assert_eq!(fixture.reads.load(Ordering::SeqCst), 2 * JOBS);
        });
        let requests = fixture.finish();
        assert_eq!(
            requests
                .iter()
                .filter(|r| request_state(r) == "row-5")
                .count(),
            1
        );
    }
}

/// Stream-lifetime interruption still cancels delivery after workers are already cleaned up.
#[test]
fn interrupt_after_terminal_cleanup_ends_error_delivery() {
    for unordered in [false, true] {
        let mut fixture = TerminalOutput::new(unordered, false);
        let runtime = Arc::clone(&fixture.runtime);
        runtime.block_on(async {
            fixture.wait_saturated().await;
            fixture.failure.release();
            fixture.wait_work_stopped().await;
            fixture.cancel.cancel();
            fixture.wait_stopped().await;
            assert!(fixture.output.as_ref().unwrap().receiver.is_closed());
            fixture.neighbor().await;
        });
        fixture.finish();
    }
}

/// A blocked source retains its thread only until return, without delaying HTTP cleanup.
#[test]
fn terminal_cleanup_does_not_wait_for_a_blocked_source() {
    for unordered in [false, true] {
        let mut fixture = TerminalOutput::new(unordered, true);
        let runtime = Arc::clone(&fixture.runtime);
        runtime.block_on(async {
            fixture.wait_saturated().await;
            fixture.failure.release();
            fixture.wait_work_stopped().await;
            fixture.neighbor().await;
            let late = fixture.late.as_ref().unwrap();
            assert!(late.retained.upgrade().is_some());
            late.sender.send(Value::test_string("late-row")).unwrap();
            tokio::time::timeout(WATCHDOG, async {
                while late.retained.upgrade().is_some() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("producer did not exit after its next call returned");
            assert_eq!(fixture.reads.load(Ordering::SeqCst), 2 * JOBS);
            assert_eq!(fixture.builds.load(Ordering::SeqCst), 7);
            fixture.consume_original_error().await;
        });
        let requests = fixture.finish();
        assert!(!requests.iter().any(|r| request_state(r) == "late-row"));
    }
}

/// Already observed HTTP failure wins over queued input without any output-capacity wait.
#[test]
fn observed_terminal_failure_does_not_drain_queued_rows() {
    runtime().block_on(async {
        let root = "http://fixture.invalid";
        let prepared =
            crate::api::client::PreparedRequest::new(build(&Value::test_string("first")).unwrap())
                .unwrap();
        let key = crate::nu::cache::RequestKey::from_body(Arc::from(root), prepared.body);
        let admission = Arc::new(tokio::sync::Semaphore::new(3));
        let first = InputRow {
            sequence: 0,
            source: Value::test_string("first"),
            request: None,
            permit: Arc::clone(&admission).acquire_owned().await.unwrap(),
        };
        let queued = InputRow {
            sequence: 2,
            source: Value::test_string("queued"),
            request: Some(Err(JevError::State("later queued failure"))),
            permit: Arc::clone(&admission).acquire_owned().await.unwrap(),
        };
        let duplicate = InputRow {
            sequence: 1,
            source: Value::test_string("first"),
            request: None,
            permit: Arc::clone(&admission).acquire_owned().await.unwrap(),
        };
        let (input_sender, mut input) = mpsc::channel(1);
        input_sender.send(queued).await.unwrap();
        let (output, outcomes) = mpsc::channel(1);
        let mut state = SupervisorState {
            cache: crate::nu::cache::CompletedCache::new(setup(root, 1).config.cache.unwrap()),
            pending_groups: Default::default(),
            in_flight: [(key.clone(), vec![first, duplicate])].into(),
            ordered: Default::default(),
            next_sequence: 0,
        };
        let (_cancel, mut signal) = CancelHandle::new();
        let routed = finish_request(
            (
                key,
                "first-request".into(),
                Err(JevError::Http { status: 400 }),
            ),
            &mut input,
            &output,
            &mut state,
            &mut signal,
            false,
            true,
        )
        .await;
        let Routing::Terminal(row) = routed else {
            panic!("observed terminal error was not preserved");
        };
        assert_eq!(row.sequence, 0);
        assert!(matches!(
            row.result.as_ref().err().unwrap().as_ref(),
            JevError::Http { status: 400 }
        ));
        assert_eq!(input.len(), 1);
        assert_eq!(admission.available_permits(), 0);
        assert_eq!(state.in_flight.values().next().unwrap().len(), 1);
        assert!(
            outcomes.is_empty(),
            "routing attempted error delivery before cleanup"
        );
    });
}
