//! Runs local HTTP/2 service fixtures on Axum's maintained server implementation.

use std::{
    convert::Infallible,
    future::IntoFuture,
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderMap, Response as HttpResponse},
    routing::any,
    serve::ListenerExt,
};
use bytes::Bytes;

/// Controls fixture progress with non-blocking async waits and bounded test rendezvous.
#[derive(Clone)]
pub(crate) struct ResponseGate {
    arrivals: tokio::sync::watch::Sender<usize>,
    released: tokio::sync::watch::Sender<bool>,
}

impl Default for ResponseGate {
    /// Creates a closed gate with no announced arrivals.
    fn default() -> Self {
        Self {
            arrivals: tokio::sync::watch::channel(0).0,
            released: tokio::sync::watch::channel(false).0,
        }
    }
}

impl ResponseGate {
    /// Announces a reached header/body boundary and waits asynchronously for test release.
    async fn wait(&self) {
        // watch remembers release even when it precedes subscription, so gates cannot
        // lose a wakeup when the test and HTTP handler reach this boundary concurrently.
        let mut released = self.released.subscribe();
        self.arrivals.send_modify(|arrivals| *arrivals += 1);
        while !*released.borrow_and_update() {
            released.changed().await.expect("fixture gate retained");
        }
    }

    /// Waits for body gates without blocking the runtime that drives client requests.
    pub(crate) async fn wait_for_arrivals(&self, count: usize) {
        let mut arrivals = self.arrivals.subscribe();
        tokio::time::timeout(Duration::from_secs(3), async {
            while *arrivals.borrow_and_update() < count {
                arrivals.changed().await.expect("fixture gate retained");
            }
        })
        .await
        .expect("missing fixture gate arrival");
    }

    /// Reports how many response bodies or handlers have reached this gate.
    pub(crate) fn arrivals(&self) -> usize {
        *self.arrivals.borrow()
    }

    /// Opens the gate for existing and future waiters.
    pub(crate) fn release(&self) {
        self.released.send_replace(true);
    }

    /// Gates blocking fixture callbacks until all peers arrive, with a protective deadline.
    pub(crate) fn rendezvous(&self, peers: usize) {
        self.arrivals.send_modify(|arrivals| *arrivals += 1);
        if self.arrivals() >= peers {
            self.release();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !*self.released.borrow() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(*self.released.borrow(), "HTTP attempts did not overlap");
    }
}

/// Captures fields needed to assert the wire behavior of a local request.
#[derive(Clone)]
pub(crate) struct CapturedRequest {
    /// HTTP method of the request.
    pub(crate) method: String,
    /// Request path and query.
    pub(crate) path: String,
    /// HTTP/2 authority or HTTP/1.1 absolute-form authority, when supplied.
    pub(crate) authority: Option<String>,
    /// Bearer header, if supplied.
    pub(crate) authorization: Option<String>,
    /// Content-Type header, if supplied.
    pub(crate) content_type: Option<String>,
    /// Complete request body.
    pub(crate) body: Vec<u8>,
    /// HTTP version seen by the server.
    pub(crate) version: axum::http::Version,
}

/// Describes a fixture response, including optional delay and body chunking.
#[derive(Clone)]
pub(crate) struct Response {
    /// Response status code.
    pub(crate) status: u16,
    /// Additional response headers.
    pub(crate) headers: Vec<(String, String)>,
    /// Body sent as HTTP/2 DATA frames.
    pub(crate) body: Vec<u8>,
    /// Delay before response headers are sent.
    pub(crate) delay: Duration,
    /// Maximum size of one response body chunk.
    pub(crate) chunk_size: usize,
    /// Optional asynchronous gate before the response headers are sent.
    pub(crate) header_gate: Option<ResponseGate>,
    /// Optional asynchronous gate after headers and before the first body chunk.
    pub(crate) body_gate: Option<ResponseGate>,
}

impl Response {
    /// Creates a JSON response with no extra headers or delay.
    pub(crate) fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: value.to_string().into_bytes(),
            delay: Duration::ZERO,
            chunk_size: 16_384,
            header_gate: None,
            body_gate: None,
        }
    }
}

/// Owns a local server until assertions finish and its request log is collected.
pub(crate) struct TestServer {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<Vec<CapturedRequest>>>,
    expected: Option<usize>,
    requests: Arc<AtomicUsize>,
    connections: Arc<AtomicUsize>,
}

impl TestServer {
    /// Reads the accepted request count without waiting or stopping the fixture.
    pub(crate) fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    /// Stops the server and returns all captured requests in arrival order.
    pub(crate) fn join(self) -> thread::Result<Vec<CapturedRequest>> {
        self.join_with_connections().map(|(requests, _)| requests)
    }

    /// Waits for expected requests, then stops and reports requests and accepted connections.
    pub(crate) fn join_with_connections(mut self) -> thread::Result<(Vec<CapturedRequest>, usize)> {
        if let Some(expected) = self.expected {
            self.wait_for_requests(expected);
        }
        self.stop.take().expect("server stop sender").send(()).ok();
        let captured = self.thread.take().expect("server thread").join()?;
        if let Some(expected) = self.expected {
            assert_eq!(captured.len(), expected, "unexpected HTTP request count");
        }
        Ok((captured, self.connections.load(Ordering::SeqCst)))
    }

    /// Waits at most two seconds for request bodies to arrive at the handler.
    pub(crate) fn wait_for_requests(&self, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.requests.load(Ordering::SeqCst) < expected && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            self.requests.load(Ordering::SeqCst) >= expected,
            "missing fixture request"
        );
    }
}

impl Drop for TestServer {
    /// Stops accepting work even when a test fails before collecting the request log.
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.send(()).ok();
        }
    }
}

/// Serves requests through Axum and returns a handle that stops it on join.
pub(crate) fn serve<F>(expected: usize, handler: F) -> (String, TestServer)
where
    F: Fn(usize, &CapturedRequest) -> Response + Send + Sync + 'static,
{
    serve_inner(Some(expected), handler)
}

/// Serves until stopped when cancellation makes the final request count variable.
pub(crate) fn serve_unbounded<F>(handler: F) -> (String, TestServer)
where
    F: Fn(usize, &CapturedRequest) -> Response + Send + Sync + 'static,
{
    serve_inner(None, handler)
}

/// Starts one local server with either a fixed or variable expected request count.
fn serve_inner<F>(expected: Option<usize>, handler: F) -> (String, TestServer)
where
    F: Fn(usize, &CapturedRequest) -> Response + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind HTTP/2 fixture");
    listener.set_nonblocking(true).expect("nonblocking fixture");
    let root = format!("http://{}", listener.local_addr().expect("fixture address"));
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed_requests = Arc::clone(&requests);
    let connections = Arc::new(AtomicUsize::new(0));
    let observed_connections = Arc::clone(&connections);
    let thread = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fixture runtime");
        let captures = Arc::new(Mutex::new(Vec::new()));
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::from_std(listener)
                .expect("Tokio listener")
                .tap_io(move |_| {
                    observed_connections.fetch_add(1, Ordering::SeqCst);
                });
            let sequence = Arc::new(AtomicUsize::new(0));
            let handler = Arc::new(handler);
            let app = Router::new().fallback(any({
                let captures = Arc::clone(&captures);
                move |request: Request| {
                    let captures = Arc::clone(&captures);
                    let sequence = Arc::clone(&sequence);
                    let handler = Arc::clone(&handler);
                    let requests = Arc::clone(&observed_requests);
                    async move {
                        let index = sequence.fetch_add(1, Ordering::SeqCst);
                        let captured = capture(request).await;
                        captures
                            .lock()
                            .expect("capture lock")
                            .push((index, captured.clone()));
                        requests.fetch_add(1, Ordering::SeqCst);
                        // A synchronous rendezvous may wait for another request. Offload
                        // it so this current-thread server can still receive that peer.
                        let response =
                            tokio::task::spawn_blocking(move || handler(index, &captured))
                                .await
                                .expect("response handler");
                        tokio::time::sleep(response.delay).await;
                        if let Some(gate) = &response.header_gate {
                            gate.wait().await;
                        }
                        make_response(response)
                    }
                }
            }));
            tokio::select! {
                result = axum::serve(listener, app).into_future() => {
                    panic!("HTTP/2 fixture stopped unexpectedly: {result:?}");
                }
                _ = stopped => {}
            }
        });
        let mut captures = captures.lock().expect("capture lock").clone();
        captures.sort_by_key(|(index, _)| *index);
        captures.into_iter().map(|(_, capture)| capture).collect()
    });
    (
        root,
        TestServer {
            stop: Some(stop),
            thread: Some(thread),
            expected,
            requests,
            connections,
        },
    )
}

/// Captures the request after reading its finite test body.
async fn capture(request: Request) -> CapturedRequest {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, usize::MAX)
        .await
        .expect("read fixture request body")
        .to_vec();
    CapturedRequest {
        method: parts.method.to_string(),
        path: parts
            .uri
            .path_and_query()
            .map_or("/", |path| path.as_str())
            .to_owned(),
        authority: parts.uri.authority().map(ToString::to_string),
        authorization: header(&parts.headers, "authorization"),
        content_type: header(&parts.headers, "content-type"),
        body,
        version: parts.version,
    }
}

/// Copies one optional header into an owned value for later assertions.
fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// Converts a fixture response to an Axum response with streamed body chunks.
fn make_response(response: Response) -> HttpResponse<Body> {
    let mut builder = HttpResponse::builder()
        .status(response.status)
        .header("content-type", "application/json");
    for (name, value) in response.headers {
        builder = builder.header(name, value);
    }
    let chunk_size = response.chunk_size.max(1);
    let chunks = futures::stream::unfold(
        (Bytes::from(response.body), response.body_gate),
        move |(mut remaining, gate)| async move {
            if let Some(gate) = gate {
                gate.wait().await;
            }
            if remaining.is_empty() {
                None
            } else {
                let chunk = remaining.split_to(remaining.len().min(chunk_size));
                Some((Ok::<Bytes, Infallible>(chunk), (remaining, None)))
            }
        },
    );
    builder
        .body(Body::from_stream(chunks))
        .expect("valid fixture response")
}
