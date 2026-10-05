//! Tests shared HTTP admission with controlled Axum responses and synthetic credentials.

use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;

use crate::api::cancel::{CancelHandle, CancelSignal};
use crate::config::{ApiKey, InvocationConfig, ProxyPolicy};
use crate::error::JevError;
use crate::h2_fixture::{self, Response, ResponseGate};
use crate::tracing::trace_evaluation;

use super::tests::{
    CapturedDiagnostics, ReleaseOnDrop, answer, config, many_question_exchange, model_list,
    models_config, request,
};
use super::{HttpAttemptLimit, HttpMeasurement, JevClient, JevClientPool, PreparedRequest};
use super::{MAX_RESPONSE_BYTES, ResponseWorkPause};

/// Builds a single-thread runtime so stalled admission cannot hide a blocked worker.
/// A blocking acquire here would also prevent active response handling and timers from advancing.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Creates a direct client carrying an explicitly sized shared process budget.
fn limited_client(capacity: usize) -> JevClient {
    JevClientPool::new(HttpAttemptLimit::new(capacity).unwrap())
        .unwrap()
        .for_policy(&ProxyPolicy::Direct)
        .unwrap()
}

/// Starts either endpoint with prepared evaluation bytes and a retained local signal.
fn spawn_operation(
    client: JevClient,
    settings: InvocationConfig,
    signal: CancelSignal,
    models: bool,
) -> tokio::task::JoinHandle<Result<HttpMeasurement, JevError>> {
    let prepared = PreparedRequest::new(request()).unwrap();
    tokio::spawn(async move {
        let key = ApiKey::for_test("local-key");
        if models {
            let mut transport = models_config(settings.base_url);
            transport.timeout = settings.timeout;
            transport.retries = settings.retries;
            client
                .models_measured(&transport, &key, signal)
                .await
                .map(|success| success.measurement)
        } else {
            client
                .system_one_prepared_measured(&prepared, &settings, &key, signal)
                .await
                .map(|success| success.measurement)
        }
    })
}

/// Awaits a spawned operation with a watchdog independent of the tested deadline.
async fn finish(
    operation: tokio::task::JoinHandle<Result<HttpMeasurement, JevError>>,
) -> Result<HttpMeasurement, JevError> {
    tokio::time::timeout(Duration::from_secs(5), operation)
        .await
        .expect("HTTP operation stalled")
        .expect("HTTP task panicked")
}

/// Observes attempted admission without blocking the current-thread runtime.
/// Events precede slot acquisition; they do not prove that an HTTP request was sent.
async fn admissions(receiver: &mut mpsc::UnboundedReceiver<()>, count: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        for _ in 0..count {
            receiver.recv().await.expect("admission observer closed");
        }
    })
    .await
    .expect("missing attempt admission");
}

/// Supplies a valid endpoint-specific response with an optional stalled body.
fn successful_response(path: &str, gate: Option<ResponseGate>) -> Response {
    let body = if path.ends_with("/models") {
        model_list()
    } else {
        answer()
    };
    let mut response = Response::json(200, body);
    response.body_gate = gate;
    response
}

/// Different logical operations overlap rather than serializing on shared client state.
#[test]
fn distinct_operations_overlap_under_the_shared_budget() {
    let gate = ResponseGate::default();
    let (root, server) = h2_fixture::serve(2, move |_, captured| {
        gate.rendezvous(2);
        successful_response(&captured.path, None)
    });
    let client = limited_client(2);
    runtime().block_on(async {
        let (_cancel, signal) = CancelHandle::new();
        let settings = config(root.parse().unwrap());
        let evaluation = spawn_operation(client.clone(), settings.clone(), signal.clone(), false);
        let listing = spawn_operation(client, settings, signal, true);
        finish(evaluation).await.unwrap();
        finish(listing).await.unwrap();
    });
    assert_eq!(server.join().unwrap().len(), 2);
}

/// Two shared slots remain occupied through bodies across roots and proxy policies.
#[test]
fn aggregate_budget_holds_through_response_bodies() {
    let gates: Vec<_> = (0..3).map(|_| ResponseGate::default()).collect();
    let sequence = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let make_handler = || {
        let gates = gates.clone();
        let sequence = Arc::clone(&sequence);
        move |_: usize, captured: &h2_fixture::CapturedRequest| {
            let index = sequence.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            successful_response(&captured.path, Some(gates[index / 2].clone()))
        }
    };
    let (first_root, first_server) = h2_fixture::serve_unbounded(make_handler());
    let (second_root, second_server) = h2_fixture::serve_unbounded(make_handler());
    let pool = JevClientPool::new(HttpAttemptLimit::new(2).unwrap()).unwrap();
    let policies = [
        ProxyPolicy::Auto,
        ProxyPolicy::Direct,
        ProxyPolicy::Explicit(first_root.clone()),
    ];
    let (observed, mut waiting) = mpsc::unbounded_channel();
    let clients: Vec<_> = policies
        .iter()
        .map(|policy| {
            let mut client = pool.for_policy(policy).unwrap();
            client.observe_admission(observed.clone());
            client
        })
        .collect();
    let budget = Arc::clone(&clients[0].attempts);
    runtime().block_on(async {
        let (_cancel, signal) = CancelHandle::new();
        let operations: Vec<_> = (0..6)
            .map(|index| {
                let root = if index % 3 == 1 {
                    &second_root
                } else {
                    &first_root
                };
                let mut settings = config(root.parse().unwrap());
                settings.timeout = Duration::from_secs(5);
                spawn_operation(
                    clients[index % 3].clone(),
                    settings,
                    signal.clone(),
                    index % 2 == 0,
                )
            })
            .collect();
        admissions(&mut waiting, 6).await;
        for (index, gate) in gates.iter().enumerate() {
            gate.wait_for_arrivals(2).await;
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert_eq!(budget.available_permits(), 0);
            assert_eq!(
                first_server.request_count() + second_server.request_count(),
                2 * (index + 1)
            );
            assert!(
                gates
                    .iter()
                    .skip(index + 1)
                    .all(|later| later.arrivals() == 0)
            );
            gate.release();
        }
        for operation in operations {
            assert_eq!(finish(operation).await.unwrap().attempts, 1);
        }
        assert_eq!(budget.available_permits(), 2);
    });
    assert_eq!(
        first_server.join().unwrap().len() + second_server.join().unwrap().len(),
        6
    );
}

/// Decoding and validation yield their slots while offloaded response work is held.
#[test]
fn response_work_releases_network_capacity() {
    for validation in [false, true] {
        let (submitted, body) = if validation {
            many_question_exchange()
        } else {
            let mut body = answer();
            body["padding"] = json!("x".repeat(70_000));
            (request(), body)
        };
        let (root, server) = h2_fixture::serve(2, move |index, _| {
            Response::json(200, if index == 0 { body.clone() } else { answer() })
        });
        let healthy = limited_client(1);
        let mut paused = healthy.clone();
        let release = ReleaseOnDrop(Arc::new(AtomicBool::new(false)));
        let (entered, mut processing) = mpsc::unbounded_channel();
        paused.response_work_pause = Some(ResponseWorkPause {
            entered,
            release: Arc::clone(&release.0),
        });
        let prepared = PreparedRequest::new(submitted).unwrap();
        runtime().block_on(async {
            let (_cancel, signal) = CancelHandle::new();
            let first_settings = config(root.parse().unwrap());
            let first_signal = signal.clone();
            let first = tokio::spawn(async move {
                paused
                    .system_one_prepared_measured(
                        &prepared,
                        &first_settings,
                        &ApiKey::for_test("local-key"),
                        first_signal,
                    )
                    .await
            });
            admissions(&mut processing, 1).await;
            assert_eq!(healthy.attempts.available_permits(), 1);
            let next = spawn_operation(
                healthy.clone(),
                config(root.parse().unwrap()),
                signal,
                false,
            );
            finish(next).await.unwrap();
            assert!(!first.is_finished(), "response gate released too early");
            release.0.store(true, std::sync::atomic::Ordering::Release);
            tokio::time::timeout(Duration::from_secs(3), first)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(healthy.attempts.available_permits(), 1);
        });
        server.join().unwrap();
    }
}

/// Both endpoints cancel or time out before admission without disabling neighboring work.
#[test]
fn queued_admission_obeys_local_cancellation_and_deadline() {
    for models in [false, true] {
        for cancel_early in [false, true] {
            let (root, server) =
                h2_fixture::serve(1, |_, captured| successful_response(&captured.path, None));
            let mut client = limited_client(1);
            let (observed, mut waiting) = mpsc::unbounded_channel();
            client.observe_admission(observed);
            runtime().block_on(async {
                let permit = client.attempts.clone().acquire_owned().await.unwrap();
                let mut settings = config(root.parse().unwrap());
                settings.timeout = if cancel_early {
                    Duration::from_secs(2)
                } else {
                    Duration::from_millis(150)
                };
                let (cancel, signal) = CancelHandle::new();
                let queued = spawn_operation(client.clone(), settings, signal, models);
                admissions(&mut waiting, 1).await;
                if cancel_early {
                    cancel.cancel();
                }
                let error = finish(queued).await.unwrap_err();
                assert_eq!(
                    error.kind_name(),
                    if cancel_early { "cancelled" } else { "timeout" }
                );
                assert_eq!(server.request_count(), 0);
                assert_eq!(client.attempts.available_permits(), 0);
                drop(permit);
                let (_keep, signal) = CancelHandle::new();
                let next = spawn_operation(
                    client.clone(),
                    config(root.parse().unwrap()),
                    signal,
                    models,
                );
                finish(next).await.unwrap();
                assert_eq!(client.attempts.available_permits(), 1);
            });
            assert_eq!(server.join().unwrap().len(), 1);
        }
    }
}

/// Backoff frees capacity, retries reacquire it, and only send-based durations include later waits.
#[test]
fn retry_releases_and_reacquires_capacity_with_original_measurements() {
    let (root, server) = h2_fixture::serve(2, |index, _| {
        if index == 0 {
            let mut response = Response::json(503, json!({}));
            response
                .headers
                .push(("retry-after-ms".into(), "200".into()));
            response
        } else {
            Response::json(200, answer())
        }
    });
    let body_gate = ResponseGate::default();
    let gate = body_gate.clone();
    let (neighbor_root, neighbor) = h2_fixture::serve(1, move |_, captured| {
        successful_response(&captured.path, Some(gate.clone()))
    });
    let mut client = limited_client(1);
    let (retrying, mut retries) = mpsc::channel(1);
    let (observed, mut waiting) = mpsc::unbounded_channel();
    client.observe_retry_waits(retrying);
    client.observe_admission(observed);
    runtime().block_on(async {
        let (_keep, signal) = CancelHandle::new();
        let mut settings = config(root.parse().unwrap());
        settings.retries = 1;
        let operation = spawn_operation(client.clone(), settings, signal.clone(), false);
        admissions(&mut waiting, 1).await;
        let (_, delay) = tokio::time::timeout(Duration::from_secs(3), retries.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(delay, Duration::from_millis(200));
        assert_eq!(client.attempts.available_permits(), 1);
        let healthy = spawn_operation(
            client.clone(),
            config(neighbor_root.parse().unwrap()),
            signal,
            true,
        );
        body_gate.wait_for_arrivals(1).await;
        admissions(&mut waiting, 2).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            server.request_count(),
            1,
            "retry sent without reacquiring its slot"
        );
        body_gate.release();
        finish(healthy).await.unwrap();
        let measured = finish(operation).await.unwrap();
        assert_eq!(measured.attempts, 2);
        assert!(measured.elapsed >= measured.attempt_elapsed + delay + Duration::from_millis(100));
        assert_eq!(
            measured.request_bytes,
            PreparedRequest::new(request()).unwrap().body.len()
        );
        assert_eq!(measured.response_bytes, answer().to_string().len());
        assert_eq!(client.attempts.available_permits(), 1);
    });
    server.join().unwrap();
    neighbor.join().unwrap();
}

/// A retry's admission wait cannot extend the deadline after its backoff completes.
#[test]
fn retry_capacity_wait_uses_the_original_deadline() {
    for models in [false, true] {
        let (root, server) = h2_fixture::serve(1, |_, _| {
            let mut response = Response::json(503, json!({}));
            response
                .headers
                .push(("retry-after-ms".into(), "100".into()));
            response
        });
        let mut client = limited_client(1);
        let (retrying, mut retries) = mpsc::channel(1);
        client.observe_retry_waits(retrying);
        runtime().block_on(async {
            let (_keep, signal) = CancelHandle::new();
            let mut settings = config(root.parse().unwrap());
            settings.retries = 1;
            settings.timeout = Duration::from_millis(300);
            let operation = spawn_operation(client.clone(), settings, signal, models);
            tokio::time::timeout(Duration::from_secs(3), retries.recv())
                .await
                .unwrap()
                .unwrap();
            let permit = client.attempts.clone().acquire_owned().await.unwrap();
            assert_eq!(finish(operation).await.unwrap_err().kind_name(), "timeout");
            assert_eq!(server.request_count(), 1);
            drop(permit);
            assert_eq!(client.attempts.available_permits(), 1);
        });
        server.join().unwrap();
    }
}

/// Initial contention is outside HTTP metrics but inside the broader operation diagnostic.
#[test]
fn initial_admission_wait_preserves_http_measurements() {
    let (root, server) = h2_fixture::serve(1, |_, _| Response::json(200, answer()));
    let mut client = limited_client(1);
    let (observed, mut waiting) = mpsc::unbounded_channel();
    client.observe_admission(observed);
    let output = Arc::new(Mutex::new(Vec::new()));
    let (writer, guard) = tracing_appender::non_blocking(CapturedDiagnostics(output.clone()));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .finish();
    let measured = tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        runtime().block_on(async {
            let permit = client.attempts.clone().acquire_owned().await.unwrap();
            let (_keep, signal) = CancelHandle::new();
            let prepared = PreparedRequest::new(request()).unwrap();
            let settings = config(root.parse().unwrap());
            let operation = tokio::spawn(async move {
                trace_evaluation(
                    "ask",
                    "initial-slot-test",
                    client.system_one_prepared_measured(
                        &prepared,
                        &settings,
                        &ApiKey::for_test("local-key"),
                        signal,
                    ),
                )
                .await
            });
            admissions(&mut waiting, 1).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(server.request_count(), 0);
            drop(permit);
            tokio::time::timeout(Duration::from_secs(3), operation)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .measurement
        })
    });
    drop(guard);
    assert_eq!(measured.attempts, 1);
    assert_eq!(measured.elapsed, measured.attempt_elapsed);
    let output = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    let completed = output
        .lines()
        .find(|line| line.contains("evaluation completed"))
        .unwrap();
    let duration_ms: u128 = completed
        .split_whitespace()
        .find_map(|field| field.strip_prefix("duration_ms="))
        .unwrap()
        .parse()
        .unwrap();
    assert!(duration_ms >= measured.elapsed.as_millis() + 199);
    assert!(completed.contains(&format!("elapsed_ns={}", measured.elapsed.as_nanos())));
    assert!(completed.contains(&format!(
        "attempt_elapsed_ns={}",
        measured.attempt_elapsed.as_nanos()
    )));
    assert!(completed.contains("input_tokens=10"));
    assert!(completed.contains("output_tokens=2"));
    server.join().unwrap();
}

/// All terminal attempt paths return capacity that a fresh operation can then consume.
#[test]
fn terminal_attempt_paths_release_full_capacity() {
    for failure in [
        "http",
        "transport",
        "body_size",
        "decode",
        "timeout",
        "cancelled",
        "success",
    ] {
        let gate = ResponseGate::default();
        let body_gate = gate.clone();
        let (root, server) = h2_fixture::serve_unbounded(move |_, _| {
            let mut response = Response::json(200, answer());
            match failure {
                "http" => response.status = 401,
                "body_size" => {
                    response.headers.push((
                        "content-length".into(),
                        (MAX_RESPONSE_BYTES + 1).to_string(),
                    ));
                    response.body_gate = Some(body_gate.clone());
                }
                "decode" => response.body = b"not-json".to_vec(),
                "timeout" | "cancelled" => response.body_gate = Some(body_gate.clone()),
                _ => {}
            }
            response
        });
        let (healthy_root, healthy_server) =
            h2_fixture::serve(1, |_, _| Response::json(200, answer()));
        let client = limited_client(1);
        runtime().block_on(async {
            let (cancel, signal) = CancelHandle::new();
            let root = if failure == "transport" {
                "http://127.0.0.1:1"
            } else {
                &root
            };
            let mut settings = config(root.parse().unwrap());
            if failure == "timeout" {
                settings.timeout = Duration::from_secs(1);
            }
            let operation = spawn_operation(client.clone(), settings, signal, false);
            if matches!(failure, "timeout" | "cancelled") {
                gate.wait_for_arrivals(1).await;
                if failure == "cancelled" {
                    cancel.cancel();
                }
            }
            let result = finish(operation).await;
            if failure == "success" {
                result.unwrap();
            } else {
                let expected = if matches!(failure, "body_size" | "decode") {
                    "response"
                } else {
                    failure
                };
                let error = result.unwrap_err();
                assert_eq!(error.kind_name(), expected);
                if failure == "body_size" {
                    assert!(matches!(error, JevError::ResponseTooLarge { .. }));
                }
            }
            assert_eq!(client.attempts.available_permits(), 1);
            let (_healthy, signal) = CancelHandle::new();
            let healthy = spawn_operation(
                client.clone(),
                config(healthy_root.parse().unwrap()),
                signal,
                false,
            );
            finish(healthy).await.unwrap();
            assert_eq!(client.attempts.available_permits(), 1);
            gate.release();
        });
        server.join().unwrap();
        healthy_server.join().unwrap();
    }
}
