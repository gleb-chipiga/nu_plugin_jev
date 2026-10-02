//! Implements authenticated, bounded-deadline HTTP calls with status-only retries.

use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use futures::future::{Either, select};
use lru::LruCache;
use reqwest::{
    Client, StatusCode, Url,
    header::{CONTENT_TYPE, HeaderMap, RETRY_AFTER},
};
use tokio::time::Instant;

use crate::{
    config::{ApiKey, InvocationConfig, ProxyPolicy},
    error::{ErrorKind, JevError},
};

use super::{
    cancel::CancelSignal,
    types::{SystemOneRequest, SystemOneResponse},
    validate::validate_response,
};

/// Reuses one HTTP connection pool for every request using the same policy.
#[derive(Clone)]
pub(crate) struct JevClient {
    http: Arc<Client>,
}

/// Retains the startup automatic client and a bounded set of alternate pools.
pub(crate) struct JevClientPool {
    auto: JevClient,
    alternate: Mutex<LruCache<ProxyPolicy, JevClient>>,
}

impl JevClientPool {
    /// Captures ordinary process and OS proxy settings in the startup client.
    pub(crate) fn new() -> Result<Self, JevError> {
        Ok(Self {
            auto: JevClient::new()?,
            alternate: Mutex::new(LruCache::new(
                NonZeroUsize::new(8).expect("nonzero pool limit"),
            )),
        })
    }

    /// Selects a retained pool on the synchronous command thread, never a Tokio worker.
    pub(crate) fn for_policy(&self, policy: &ProxyPolicy) -> Result<JevClient, JevError> {
        if *policy == ProxyPolicy::Auto {
            return Ok(self.auto.clone());
        }
        let mut alternate = self.alternate.lock().map_err(|_| {
            JevError::new(ErrorKind::Transport, "Jev HTTP client pool is unavailable")
        })?;
        if let Some(client) = alternate.get(policy) {
            return Ok(client.clone());
        }
        let client = JevClient::for_policy(policy)?;
        alternate.put(policy.clone(), client.clone());
        Ok(client)
    }
}

/// Couples a typed request with JSON bytes prepared outside Tokio workers.
pub(crate) struct PreparedRequest {
    /// The validated request retained for response-contract checks.
    pub(crate) wire: SystemOneRequest,
    /// The exact reusable HTTP body and canonical cache-key body.
    pub(crate) body: Bytes,
}

impl PreparedRequest {
    /// Serializes one request once before scheduling its HTTP evaluation.
    pub(crate) fn new(wire: SystemOneRequest) -> Result<Self, JevError> {
        let body = encode_request(&wire)?;
        Ok(Self { wire, body })
    }
}

/// Encodes a validated request without exposing its contents in errors.
fn encode_request(request: &SystemOneRequest) -> Result<Bytes, JevError> {
    serde_json::to_vec(request)
        .map(Bytes::from)
        .map_err(|_| JevError::new(ErrorKind::Validation, "cannot encode Jev request"))
}

impl JevClient {
    /// Builds an automatic-proxy client without authenticated redirects or hidden retries.
    pub(crate) fn new() -> Result<Self, JevError> {
        Self::for_policy(&ProxyPolicy::Auto)
    }

    /// Builds a client for one effective policy before entering the async runtime.
    fn for_policy(policy: &ProxyPolicy) -> Result<Self, JevError> {
        let builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never());
        let builder = match policy {
            ProxyPolicy::Auto => builder,
            ProxyPolicy::Direct => builder.no_proxy(),
            ProxyPolicy::Explicit(url) => {
                let proxy = reqwest::Proxy::all(url).map_err(|_| {
                    JevError::new(ErrorKind::Validation, "cannot configure Jev proxy")
                })?;
                builder.proxy(proxy)
            }
        };
        let http = builder.build().map_err(|_| {
            JevError::new(ErrorKind::Transport, "cannot initialize Jev HTTP client")
        })?;
        Ok(Self {
            http: Arc::new(http),
        })
    }

    /// Evaluates one complete request and validates every returned typed answer.
    pub(crate) async fn system_one(
        &self,
        request: &SystemOneRequest,
        config: &InvocationConfig,
        key: &ApiKey,
        cancel: CancelSignal,
    ) -> Result<SystemOneResponse, JevError> {
        let body = encode_request(request)?;
        self.system_one_encoded(request, &body, config, key, cancel)
            .await
    }

    /// Reuses a prepared body across attempts without JSON work on Tokio workers.
    pub(crate) async fn system_one_prepared(
        &self,
        prepared: &PreparedRequest,
        config: &InvocationConfig,
        key: &ApiKey,
        cancel: CancelSignal,
    ) -> Result<SystemOneResponse, JevError> {
        self.system_one_encoded(&prepared.wire, &prepared.body, config, key, cancel)
            .await
    }

    /// Sends encoded JSON and validates its response against the typed questions.
    async fn system_one_encoded(
        &self,
        request: &SystemOneRequest,
        body: &Bytes,
        config: &InvocationConfig,
        key: &ApiKey,
        cancel: CancelSignal,
    ) -> Result<SystemOneResponse, JevError> {
        let response: SystemOneResponse = self.send_system_one(body, config, key, cancel).await?;
        validate_response(&response, &request.questions).map_err(|_| {
            JevError::new(
                ErrorKind::Response,
                "Jev answer does not match the submitted questions",
            )
        })?;
        Ok(response)
    }

    /// Sends one authenticated System One operation with retryable status handling.
    async fn send_system_one(
        &self,
        body: &Bytes,
        config: &InvocationConfig,
        key: &ApiKey,
        mut cancel: CancelSignal,
    ) -> Result<SystemOneResponse, JevError> {
        if cancel.is_cancelled() {
            return Err(JevError::new(
                ErrorKind::Cancelled,
                "Jev invocation was cancelled",
            ));
        }
        let url = system_one_url(&config.base_url)?;
        let deadline = Instant::now() + config.timeout;
        let operation = async {
            for attempt in 0..=config.retries {
                let attempt_number = attempt + 1;
                tracing::debug!(attempt = attempt_number, "Jev HTTP attempt started");
                let request = self
                    .http
                    .post(url.clone())
                    .bearer_auth(key.as_str())
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.clone());
                let response = request.send().await.map_err(|_| {
                    tracing::debug!(attempt = attempt_number, "Jev HTTP transport failed");
                    JevError::new(ErrorKind::Transport, "Jev HTTP transport failed")
                })?;
                let status = response.status();
                if let Some(server_request_id) =
                    server_request_id(response.headers()).filter(|id| !id.contains(key.as_str()))
                {
                    tracing::debug!(
                        attempt = attempt_number,
                        status = status.as_u16(),
                        server_request_id,
                        "Jev HTTP response received"
                    );
                } else {
                    tracing::debug!(
                        attempt = attempt_number,
                        status = status.as_u16(),
                        "Jev HTTP response received"
                    );
                }
                if status.is_success() {
                    return response.json::<SystemOneResponse>().await.map_err(|_| {
                        JevError::new(
                            ErrorKind::Response,
                            "Jev returned malformed JSON or missing fields",
                        )
                    });
                }
                if attempt < config.retries && retryable(status) {
                    let delay = retry_delay(response.headers(), SystemTime::now(), attempt);
                    if Instant::now()
                        .checked_add(delay)
                        .is_none_or(|next| next >= deadline)
                    {
                        return Err(JevError::new(
                            ErrorKind::Timeout,
                            "Jev retry delay exceeds the evaluation deadline",
                        ));
                    }
                    tracing::debug!(
                        status = status.as_u16(),
                        attempt = attempt_number,
                        next_attempt = attempt_number + 1,
                        delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        "retrying Jev request"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
                return Err(JevError::http(status.as_u16()));
            }
            unreachable!("retry loop returns on success or terminal failure")
        };
        let cancelled = cancel.cancelled();
        let timed_operation = tokio::time::timeout_at(deadline, operation);
        futures::pin_mut!(cancelled, timed_operation);
        match select(cancelled, timed_operation).await {
            Either::Left(((), _)) => Err(JevError::new(
                ErrorKind::Cancelled,
                "Jev invocation was cancelled",
            )),
            Either::Right((result, _)) => result.unwrap_or_else(|_| {
                Err(JevError::new(
                    ErrorKind::Timeout,
                    "Jev evaluation deadline expired",
                ))
            }),
        }
    }
}

/// Appends the fixed System One endpoint below the configured service root.
fn system_one_url(root: &Url) -> Result<Url, JevError> {
    let mut url = root.clone();
    let mut segments = url.path_segments_mut().map_err(|_| {
        JevError::new(
            ErrorKind::Validation,
            "Jev service root cannot contain path segments",
        )
    })?;
    segments.pop_if_empty().push("v1").push("systemone");
    drop(segments);
    Ok(url)
}

/// Recognizes only the statuses explicitly approved for automatic retries.
fn retryable(status: StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 502 | 503 | 504 | 529)
}

/// Parses either Retry-After delta-seconds or an HTTP date relative to now.
fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse::<u64>().ok().map(Duration::from_secs);
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|date| date.duration_since(now).unwrap_or(Duration::ZERO))
}

/// Parses a finite, nonnegative millisecond delay without truncating fractions.
fn retry_after_ms(value: &str) -> Option<Duration> {
    let millis = value.trim().parse::<f64>().ok()?;
    if !millis.is_finite() || millis < 0.0 {
        return None;
    }
    Duration::try_from_secs_f64(millis / 1_000.0).ok()
}

/// Selects service guidance before the bounded unguided retry delay.
fn retry_delay(headers: &HeaderMap, now: SystemTime, attempt: usize) -> Duration {
    headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(retry_after_ms)
        .or_else(|| {
            headers
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| retry_after(value, now))
        })
        .unwrap_or_else(|| jittered_backoff(attempt))
}

/// Accepts only a short printable server ID for per-attempt diagnostics.
fn server_request_id(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("x-typesafe-request-id")?.to_str().ok()?;
    let bytes = value.as_bytes();
    (!bytes.is_empty()
        && bytes.len() <= 128
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b':' | b'-')))
    .then_some(value)
}

/// Computes capped exponential backoff with a bounded per-attempt jitter.
fn jittered_backoff(attempt: usize) -> Duration {
    let entropy = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    jittered_backoff_with_entropy(attempt, entropy)
}

/// Applies deterministic 0–25% downward jitter to a capped exponential base.
fn jittered_backoff_with_entropy(attempt: usize, entropy: u64) -> Duration {
    let base_ms = 500_u64.saturating_mul(1_u64 << attempt.min(4)).min(5_000);
    let spread_ms = base_ms / 4;
    Duration::from_millis(base_ms - spread_ms + entropy % (spread_ms + 1))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        process::Command,
        sync::{Arc, Mutex},
        thread,
        time::{Duration, Instant as StdInstant, SystemTime},
    };

    use reqwest::{Client, StatusCode, header::HeaderMap};
    use serde_json::json;

    use crate::{
        api::{
            cancel::CancelHandle,
            types::{Question, SystemOneRequest},
        },
        config::{ApiKey, InvocationConfig, ProxyPolicy},
        error::{ErrorKind, JevError},
        tracing::trace_evaluation,
    };

    use super::{
        Bytes, JevClient, JevClientPool, jittered_backoff, jittered_backoff_with_entropy,
        retry_after, retry_after_ms, retry_delay, retryable, server_request_id, system_one_url,
    };

    /// Captures only fields needed to verify outgoing mock requests.
    struct CapturedRequest {
        method: String,
        path: String,
        authorization: Option<String>,
        content_type: Option<String>,
        body: Vec<u8>,
    }

    /// Describes one response sent by the local mock server.
    struct MockResponse {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
        delay: Duration,
    }

    /// Collects test diagnostics through a dedicated non-blocking tracing worker.
    struct CapturedDiagnostics(Arc<Mutex<Vec<u8>>>);

    impl Write for CapturedDiagnostics {
        /// Appends one diagnostic buffer outside the Tokio runtime worker.
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        /// Flushes no extra state beyond the shared in-memory buffer.
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl MockResponse {
        /// Creates a JSON response without extra headers or delay.
        fn json(status: u16, body: serde_json::Value) -> Self {
            Self {
                status,
                headers: Vec::new(),
                body: body.to_string(),
                delay: Duration::ZERO,
            }
        }

        /// Adds one response header for retry or redirect tests.
        fn header(mut self, name: &str, value: &str) -> Self {
            self.headers.push((name.to_owned(), value.to_owned()));
            self
        }
    }

    /// Runs a bounded local HTTP fixture on a dedicated blocking test thread.
    fn serve(
        responses: Vec<MockResponse>,
    ) -> (reqwest::Url, thread::JoinHandle<Vec<CapturedRequest>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url =
            reqwest::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let handle = thread::spawn(move || {
            responses
                .into_iter()
                .map(|response| {
                    let deadline = StdInstant::now() + Duration::from_secs(5);
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
                                && StdInstant::now() < deadline => thread::sleep(Duration::from_millis(2)),
                            Err(error) => panic!("mock server did not receive request: {error}"),
                        }
                    };
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let parts: Vec<_> = line.split_whitespace().collect();
                    let method = parts[0].to_owned();
                    let path = parts[1].to_owned();
                    let mut authorization = None;
                    let mut content_type = None;
                    let mut content_length = 0;
                    loop {
                        line.clear();
                        reader.read_line(&mut line).unwrap();
                        if line == "\r\n" {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':') {
                            let value = value.trim();
                            if name.eq_ignore_ascii_case("authorization") {
                                authorization = Some(value.to_owned());
                            }
                            if name.eq_ignore_ascii_case("content-type") {
                                content_type = Some(value.to_owned());
                            }
                            if name.eq_ignore_ascii_case("content-length") {
                                content_length = value.parse().unwrap();
                            }
                        }
                    }
                    let mut body = vec![0; content_length];
                    reader.read_exact(&mut body).unwrap();
                    thread::sleep(response.delay);
                    let mut headers = format!(
                        "HTTP/1.1 {} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                        response.status,
                        response.body.len()
                    );
                    for (name, value) in response.headers {
                        headers.push_str(&format!("{name}: {value}\r\n"));
                    }
                    headers.push_str("\r\n");
                    let _ = stream.write_all(headers.as_bytes());
                    let _ = stream.write_all(response.body.as_bytes());
                    CapturedRequest { method, path, authorization, content_type, body }
                })
                .collect()
        });
        (url, handle)
    }

    /// Creates the default mock invocation settings without contacting TypeSafe.
    fn config(base_url: reqwest::Url) -> InvocationConfig {
        InvocationConfig {
            model: "jev-latest".to_owned(),
            base_url,
            proxy: crate::config::ProxyPolicy::Auto,
            timeout: Duration::from_secs(2),
            jobs: None,
            retries: 0,
            cache: None,
        }
    }

    /// Builds one typed Noul request for transport tests.
    fn request() -> SystemOneRequest {
        SystemOneRequest {
            state: json!({"message": "hello"}),
            model: "jev-latest".to_owned(),
            questions: BTreeMap::from([(
                "spam".to_owned(),
                Question::Noul {
                    instructions: Some(json!("Is this spam?")),
                    criteria: None,
                },
            )]),
        }
    }

    /// Returns one complete valid typed answer envelope.
    fn answer() -> serde_json::Value {
        json!({"model": "jev-2026-09", "answers": {"spam": {"type": "noul", "noul": 0.9}},
            "usage": {"input_tokens": 10, "output_tokens": 2}})
    }

    /// Joins the System One path independently of the root's trailing slash.
    #[test]
    fn joins_endpoints_below_roots() {
        for root in ["http://localhost:1234", "http://localhost:1234/"] {
            let root = reqwest::Url::parse(root).unwrap();
            assert_eq!(
                system_one_url(&root).unwrap().as_str(),
                "http://localhost:1234/v1/systemone"
            );
        }
    }

    /// Parses both service guidance formats and excludes unapproved statuses.
    #[test]
    fn retry_guidance_and_statuses() {
        let now = SystemTime::now();
        assert_eq!(retry_after("2", now), Some(Duration::from_secs(2)));
        let date = httpdate::fmt_http_date(now + Duration::from_secs(5));
        assert!(retry_after(&date, now).is_some_and(|delay| delay <= Duration::from_secs(5)));
        assert_eq!(retry_after("broken", now), None);
        assert_eq!(retry_after_ms("250"), Some(Duration::from_millis(250)));
        assert_eq!(retry_after_ms("0.5"), Some(Duration::from_micros(500)));
        assert_eq!(retry_after_ms("0"), Some(Duration::ZERO));
        for invalid in ["", "-1", "NaN", "Infinity", "1e100", "broken"] {
            assert_eq!(retry_after_ms(invalid), None, "{invalid}");
        }
        let mut headers = HeaderMap::new();
        headers.insert("retry-after-ms", "250".parse().unwrap());
        headers.insert("retry-after", "2".parse().unwrap());
        assert_eq!(retry_delay(&headers, now, 0), Duration::from_millis(250));
        headers.insert("retry-after-ms", "invalid".parse().unwrap());
        assert_eq!(retry_delay(&headers, now, 0), Duration::from_secs(2));
        headers.remove("retry-after");
        assert!(
            (Duration::from_millis(375)..=Duration::from_millis(500))
                .contains(&retry_delay(&headers, now, 0))
        );
        for status in [429, 502, 503, 504, 529] {
            assert!(retryable(StatusCode::from_u16(status).unwrap()));
        }
        for status in [400, 401, 422, 500] {
            assert!(!retryable(StatusCode::from_u16(status).unwrap()));
        }
        assert!(
            (Duration::from_millis(375)..=Duration::from_millis(500))
                .contains(&jittered_backoff(0))
        );
        assert!(jittered_backoff(100) <= Duration::from_secs(5));
        for (attempt, lower, upper) in [
            (0, 375, 500),
            (1, 750, 1_000),
            (2, 1_500, 2_000),
            (3, 3_000, 4_000),
            (4, 3_750, 5_000),
            (100, 3_750, 5_000),
        ] {
            assert_eq!(
                jittered_backoff_with_entropy(attempt, 0),
                Duration::from_millis(lower)
            );
            assert_eq!(
                jittered_backoff_with_entropy(attempt, upper - lower),
                Duration::from_millis(upper)
            );
        }
    }

    /// Omits absent, malformed, and oversized service IDs without allocation.
    #[test]
    fn server_request_id_is_bounded_and_sanitized() {
        let mut headers = HeaderMap::new();
        assert_eq!(server_request_id(&headers), None);
        for (value, expected) in [
            ("req_123:abc-1.2", Some("req_123:abc-1.2")),
            ("", None),
            ("req bad", None),
            ("key=secret", None),
            ("req/123", None),
        ] {
            headers.insert("x-typesafe-request-id", value.parse().unwrap());
            assert_eq!(server_request_id(&headers), expected);
        }
        let max = "a".repeat(128);
        headers.insert("x-typesafe-request-id", max.parse().unwrap());
        assert_eq!(server_request_id(&headers), Some(max.as_str()));
        headers.insert("x-typesafe-request-id", "a".repeat(129).parse().unwrap());
        assert_eq!(server_request_id(&headers), None);
    }

    /// Verifies the System One method, path, body, and caller token.
    #[test]
    fn sends_authenticated_requests_to_local_endpoints() {
        let (url, server) = serve(vec![MockResponse::json(200, answer())]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let config = config(url);
            let key = ApiKey::for_test("local-test-key");
            let (_handle, signal) = CancelHandle::new();
            let response = client
                .system_one(&request(), &config, &key, signal)
                .await
                .unwrap();
            assert_eq!(response.model, "jev-2026-09");
        });
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            (&*requests[0].method, &*requests[0].path),
            ("POST", "/v1/systemone")
        );
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer local-test-key")
        );
        assert_eq!(
            requests[0].content_type.as_deref(),
            Some("application/json")
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&requests[0].body).unwrap(),
            serde_json::to_value(request()).unwrap()
        );
    }

    /// Retries approved statuses and preserves a response-contract failure.
    #[test]
    fn retries_only_approved_statuses() {
        let (url, server) = serve(vec![
            MockResponse::json(503, json!({})).header("Retry-After", "0"),
            MockResponse::json(200, answer()),
        ]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let mut config = config(url);
            config.retries = 3;
            let (_handle, signal) = CancelHandle::new();
            assert!(
                client
                    .system_one(&request(), &config, &ApiKey::for_test("test"), signal)
                    .await
                    .is_ok()
            );
        });
        assert_eq!(server.join().unwrap().len(), 2);

        for status in [400, 401, 422] {
            let (url, server) = serve(vec![MockResponse::json(status, json!({}))]);
            runtime.block_on(async {
                let client = JevClient::new().unwrap();
                let mut config = config(url);
                config.retries = 3;
                let (_handle, signal) = CancelHandle::new();
                let error = client
                    .system_one(&request(), &config, &ApiKey::for_test("test"), signal)
                    .await
                    .unwrap_err();
                assert_eq!(error.kind, ErrorKind::Http);
                assert_eq!(error.status, Some(status));
            });
            assert_eq!(server.join().unwrap().len(), 1);
        }

        let (url, server) = serve(vec![
            MockResponse::json(529, json!({})).header("Retry-After", "0"),
            MockResponse::json(200, answer()),
        ]);
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let mut config = config(url);
            config.retries = 3;
            let (_handle, signal) = CancelHandle::new();
            assert!(
                client
                    .system_one(&request(), &config, &ApiKey::for_test("test"), signal)
                    .await
                    .is_ok()
            );
        });
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// Uses millisecond guidance instead of a longer Retry-After fallback.
    #[test]
    fn millisecond_guidance_takes_precedence_on_the_wire() {
        let (url, server) = serve(vec![
            MockResponse::json(503, json!({}))
                .header("retry-after-ms", "250")
                .header("Retry-After", "2"),
            MockResponse::json(200, answer()),
        ]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = StdInstant::now();
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let mut config = config(url);
            config.retries = 1;
            config.timeout = Duration::from_millis(1_500);
            let (_handle, signal) = CancelHandle::new();
            assert!(
                client
                    .system_one(&request(), &config, &ApiKey::for_test("test"), signal)
                    .await
                    .is_ok()
            );
        });
        assert!(started.elapsed() >= Duration::from_millis(240));
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// Refuses an authenticated redirect rather than forwarding its token.
    #[test]
    fn authenticated_redirect_is_terminal() {
        let (url, server) = serve(vec![
            MockResponse::json(302, json!({})).header("Location", "http://127.0.0.1:1/other"),
        ]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let (_handle, signal) = CancelHandle::new();
            let error = client
                .system_one(&request(), &config(url), &ApiKey::for_test("test"), signal)
                .await
                .unwrap_err();
            assert_eq!(error.status, Some(302));
        });
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// Ends before Retry-After when guidance cannot fit within the deadline.
    #[test]
    fn retry_guidance_cannot_override_deadline() {
        let (url, server) = serve(vec![
            MockResponse::json(429, json!({}))
                .header("retry-after-ms", "2000")
                .header("Retry-After", "0"),
        ]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let mut config = config(url);
            config.retries = 3;
            config.timeout = Duration::from_millis(100);
            let (_handle, signal) = CancelHandle::new();
            let error = client
                .system_one(&request(), &config, &ApiKey::for_test("test"), signal)
                .await
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Timeout);
        });
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// Falls back to Retry-After instead of retrying early on invalid milliseconds.
    #[test]
    fn invalid_millisecond_guidance_uses_retry_after_deadline() {
        let (url, server) = serve(vec![
            MockResponse::json(429, json!({}))
                .header("retry-after-ms", "Infinity")
                .header("Retry-After", "2"),
        ]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let mut config = config(url);
            config.retries = 1;
            config.timeout = Duration::from_millis(100);
            let (_handle, signal) = CancelHandle::new();
            let error = client
                .system_one(&request(), &config, &ApiKey::for_test("test"), signal)
                .await
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Timeout);
        });
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// Interrupts a stalled HTTP response without waiting for its timeout.
    #[test]
    fn cancellation_interrupts_stalled_response() {
        let mut response = MockResponse::json(200, answer());
        response.delay = Duration::from_millis(400);
        let (url, server) = serve(vec![response]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = StdInstant::now();
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let (handle, signal) = CancelHandle::new();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(40)).await;
                handle.cancel();
            });
            let error = client
                .system_one(&request(), &config(url), &ApiKey::for_test("test"), signal)
                .await
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Cancelled);
        });
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// Preserves the terminal status after exactly the configured attempt budget.
    #[test]
    fn retry_budget_counts_additional_attempts() {
        let responses = (0..4)
            .map(|_| MockResponse::json(503, json!({})).header("Retry-After", "0"))
            .collect();
        let (url, server) = serve(responses);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let mut config = config(url);
            config.retries = 3;
            let (_handle, signal) = CancelHandle::new();
            let error = client
                .system_one(&request(), &config, &ApiKey::for_test("test"), signal)
                .await
                .unwrap_err();
            assert_eq!(error.status, Some(503));
        });
        assert_eq!(server.join().unwrap().len(), 4);

        let (url, server) = serve(vec![MockResponse::json(503, json!({}))]);
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let (_handle, signal) = CancelHandle::new();
            let error = client
                .system_one(&request(), &config(url), &ApiKey::for_test("test"), signal)
                .await
                .unwrap_err();
            assert_eq!(error.status, Some(503));
        });
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// Falls back to bounded jitter when Retry-After is malformed.
    #[test]
    fn malformed_retry_guidance_uses_backoff() {
        let (url, server) = serve(vec![
            MockResponse::json(503, json!({})).header("Retry-After", "not-a-date"),
            MockResponse::json(200, answer()),
        ]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = StdInstant::now();
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let mut config = config(url);
            config.retries = 1;
            let (_handle, signal) = CancelHandle::new();
            assert!(
                client
                    .system_one(&request(), &config, &ApiKey::for_test("test"), signal)
                    .await
                    .is_ok()
            );
        });
        assert!(started.elapsed() >= Duration::from_millis(350));
        assert_eq!(server.join().unwrap().len(), 2);
    }

    /// Rejects malformed and contract-invalid responses without retrying them.
    #[test]
    fn response_failures_are_redacted_and_not_retried() {
        let invalid = MockResponse {
            status: 200,
            headers: Vec::new(),
            body: "not-json caller-secret message".to_owned(),
            delay: Duration::ZERO,
        };
        let (url, server) = serve(vec![invalid]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let mut config = config(url);
            config.retries = 3;
            let (_handle, signal) = CancelHandle::new();
            let error = client
                .system_one(
                    &request(),
                    &config,
                    &ApiKey::for_test("caller-secret"),
                    signal,
                )
                .await
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Response);
            assert_eq!(error.status, None);
            assert!(!error.to_string().contains("caller-secret"));
            assert!(!error.to_string().contains("message"));
        });
        assert_eq!(server.join().unwrap().len(), 1);

        let missing = json!({"model": "jev-model", "answers": {"spam": {"type": "noul"}},
            "usage": {"input_tokens": 1, "output_tokens": 1}});
        let (url, server) = serve(vec![MockResponse::json(200, missing)]);
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let (_handle, signal) = CancelHandle::new();
            let error = client
                .system_one(&request(), &config(url), &ApiKey::for_test("test"), signal)
                .await
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Response);
        });
        assert_eq!(server.join().unwrap().len(), 1);

        let wrong_answer = json!({"model": "jev-model", "answers": {
            "spam": {"type": "choice", "choice": "x", "confidence": 0.8,
                "probabilities": {"x": 0.8}}},
            "usage": {"input_tokens": 1, "output_tokens": 1}});
        let (url, server) = serve(vec![MockResponse::json(200, wrong_answer)]);
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let (_handle, signal) = CancelHandle::new();
            let error = client
                .system_one(&request(), &config(url), &ApiKey::for_test("test"), signal)
                .await
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Response);
        });
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// Cancels a retry wait immediately instead of waiting for Retry-After.
    #[test]
    fn cancellation_interrupts_retry_wait() {
        let (url, server) = serve(vec![
            MockResponse::json(429, json!({}))
                .header("retry-after-ms", "10000")
                .header("Retry-After", "0"),
        ]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = StdInstant::now();
        runtime.block_on(async {
            let client = JevClient::new().unwrap();
            let mut config = config(url);
            config.retries = 3;
            config.timeout = Duration::from_secs(30);
            let (handle, signal) = CancelHandle::new();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(40)).await;
                handle.cancel();
            });
            let error = client
                .system_one(&request(), &config, &ApiKey::for_test("test"), signal)
                .await
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Cancelled);
        });
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(server.join().unwrap().len(), 1);
    }

    /// Confirms HTTP, success, and failure diagnostics omit credentials and payloads.
    #[test]
    fn retry_diagnostics_are_redacted() {
        let (url, server) = serve(vec![
            MockResponse::json(503, json!({}))
                .header("Retry-After", "0")
                .header("x-typesafe-request-id", "req_retry_1"),
            MockResponse::json(503, json!({}))
                .header("Retry-After", "0")
                .header("x-typesafe-request-id", "req_local-test-key"),
            MockResponse::json(200, answer()).header("x-typesafe-request-id", "req_success_3"),
        ]);
        let output = Arc::new(Mutex::new(Vec::new()));
        let (writer, guard) = tracing_appender::non_blocking(CapturedDiagnostics(output.clone()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_max_level(tracing::Level::DEBUG)
            .finish();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tracing::subscriber::with_default(subscriber, || {
            tracing::callsite::rebuild_interest_cache();
            runtime.block_on(async {
                let client = JevClient::new().unwrap();
                let mut config = config(url);
                config.retries = 2;
                let (_handle, signal) = CancelHandle::new();
                assert!(
                    trace_evaluation(
                        "ask",
                        "jev-trace-test",
                        client.system_one(
                            &request(),
                            &config,
                            &ApiKey::for_test("local-test-key"),
                            signal,
                        ),
                    )
                    .await
                    .is_ok()
                );
                let failure =
                    trace_evaluation("ask", "jev-failed", async { Err(JevError::http(401)) }).await;
                assert_eq!(failure.unwrap_err().status, Some(401));
            });
        });
        drop(guard);
        assert_eq!(server.join().unwrap().len(), 3);
        let diagnostics = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        for (attempt, expected_id) in [(1, "req_retry_1"), (3, "req_success_3")] {
            let line = diagnostics
                .lines()
                .find(|line| {
                    line.contains("Jev HTTP response received")
                        && line.contains(&format!("attempt={attempt}"))
                })
                .expect("attempt response diagnostic");
            assert!(line.contains("request_id=\"jev-trace-test\""));
            assert!(line.contains(&format!("server_request_id=\"{expected_id}\"")));
        }
        for secret in [
            "local-test-key",
            "hello",
            "Is this spam?",
            "Authorization",
            "req_local-test-key",
            "x-typesafe-request-id",
        ] {
            assert!(!diagnostics.contains(secret));
        }
    }

    /// Reuses compatible pools, bounds alternates, and preserves active evicted clients.
    #[test]
    fn proxy_client_cache_reuses_and_safely_evicts() {
        let pool = JevClientPool::new().unwrap();
        let direct = pool.for_policy(&ProxyPolicy::Direct).unwrap();
        let same = pool.for_policy(&ProxyPolicy::Direct).unwrap();
        assert!(Arc::ptr_eq(&direct.http, &same.http));
        let auto = pool.for_policy(&ProxyPolicy::Auto).unwrap();
        assert!(!Arc::ptr_eq(&direct.http, &auto.http));
        for port in 10000..10008 {
            pool.for_policy(&ProxyPolicy::Explicit(format!("http://127.0.0.1:{port}")))
                .unwrap();
        }
        assert_eq!(pool.alternate.lock().unwrap().len(), 8);
        let replacement = pool.for_policy(&ProxyPolicy::Direct).unwrap();
        assert!(!Arc::ptr_eq(&direct.http, &replacement.http));
        assert!(Arc::strong_count(&direct.http) >= 1);
    }

    /// Routes through an explicit HTTP proxy even for a localhost target.
    #[test]
    fn explicit_http_proxy_is_authoritative() {
        let (proxy_url, server) = serve(vec![MockResponse::json(200, answer())]);
        let pool = JevClientPool::new().unwrap();
        let policy = ProxyPolicy::Explicit(proxy_url.to_string());
        let client = pool.for_policy(&policy).unwrap();
        let mut settings = config(reqwest::Url::parse("http://localhost:1/").unwrap());
        settings.proxy = policy;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (_handle, signal) = CancelHandle::new();
            assert!(
                client
                    .system_one(&request(), &settings, &ApiKey::for_test("test"), signal)
                    .await
                    .is_ok()
            );
        });
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "http://localhost:1/v1/systemone");
    }

    /// Confirms SOCKS5h sends the destination hostname to the proxy for resolution.
    #[test]
    fn socks5h_proxy_receives_destination_hostname() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut greeting = [0; 2];
            stream.read_exact(&mut greeting).unwrap();
            assert_eq!(greeting[0], 5);
            let mut methods = vec![0; greeting[1] as usize];
            stream.read_exact(&mut methods).unwrap();
            assert!(methods.contains(&0));
            stream.write_all(&[5, 0]).unwrap();
            let mut request = [0; 5];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(&request[..4], &[5, 1, 0, 3]);
            let mut destination = vec![0; request[4] as usize + 2];
            stream.read_exact(&mut destination).unwrap();
            let domain = String::from_utf8(destination[..request[4] as usize].to_vec()).unwrap();
            stream.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with("POST /v1/systemone HTTP/1.1"));
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
            }
            let body = answer().to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            domain
        });
        let policy = ProxyPolicy::Explicit(format!("socks5h://{address}"));
        let client = JevClientPool::new().unwrap().for_policy(&policy).unwrap();
        let mut settings = config(reqwest::Url::parse("http://unresolvable.invalid/").unwrap());
        settings.proxy = policy;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (_handle, signal) = CancelHandle::new();
            assert!(
                client
                    .system_one(&request(), &settings, &ApiKey::for_test("test"), signal)
                    .await
                    .is_ok()
            );
        });
        assert_eq!(server.join().unwrap(), "unresolvable.invalid");
    }

    /// Checks ordinary proxy routing and NO_PROXY bypass in isolated processes.
    #[test]
    fn auto_proxy_honors_standard_discovery_and_bypass() {
        if let Some(mode) = std::env::var_os("JEV_AUTO_PROXY_CHILD") {
            let mode = mode.to_str().unwrap();
            let local = if matches!(mode, "bypass" | "direct") {
                Some(serve(vec![MockResponse::json(200, answer())]))
            } else {
                None
            };
            let url = local
                .as_ref()
                .map(|(url, _)| url.clone())
                .unwrap_or_else(|| reqwest::Url::parse("http://localhost:1/").unwrap());
            let client = if mode == "direct" {
                JevClient::for_policy(&ProxyPolicy::Direct).unwrap()
            } else {
                JevClient::new().unwrap()
            };
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let (_handle, signal) = CancelHandle::new();
                assert!(
                    client
                        .system_one(&request(), &config(url), &ApiKey::for_test("test"), signal)
                        .await
                        .is_ok()
                );
            });
            if let Some((_, server)) = local {
                assert_eq!(server.join().unwrap().len(), 1);
            }
            return;
        }

        let binary = std::env::current_exe().unwrap();
        let bypass = Command::new(&binary)
            .args([
                "--exact",
                "api::client::tests::auto_proxy_honors_standard_discovery_and_bypass",
            ])
            .env("JEV_AUTO_PROXY_CHILD", "bypass")
            .env("HTTP_PROXY", "http://127.0.0.1:1")
            .env("http_proxy", "http://127.0.0.1:1")
            .env("HTTPS_PROXY", "http://127.0.0.1:1")
            .env("https_proxy", "http://127.0.0.1:1")
            .env("ALL_PROXY", "http://127.0.0.1:1")
            .env("all_proxy", "http://127.0.0.1:1")
            .env("NO_PROXY", "localhost,127.0.0.1")
            .env("no_proxy", "localhost,127.0.0.1")
            .status()
            .unwrap();
        assert!(bypass.success());

        let direct = Command::new(&binary)
            .args([
                "--exact",
                "api::client::tests::auto_proxy_honors_standard_discovery_and_bypass",
            ])
            .env("JEV_AUTO_PROXY_CHILD", "direct")
            .env("HTTP_PROXY", "http://127.0.0.1:1")
            .env("http_proxy", "http://127.0.0.1:1")
            .env("ALL_PROXY", "http://127.0.0.1:1")
            .env("all_proxy", "http://127.0.0.1:1")
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .status()
            .unwrap();
        assert!(direct.success());

        let (proxy_url, proxy_server) = serve(vec![MockResponse::json(200, answer())]);
        let routed = Command::new(&binary)
            .args([
                "--exact",
                "api::client::tests::auto_proxy_honors_standard_discovery_and_bypass",
            ])
            .env("JEV_AUTO_PROXY_CHILD", "route")
            .env("HTTP_PROXY", proxy_url.as_str())
            .env("http_proxy", proxy_url.as_str())
            .env("HTTPS_PROXY", proxy_url.as_str())
            .env("https_proxy", proxy_url.as_str())
            .env("ALL_PROXY", proxy_url.as_str())
            .env("all_proxy", proxy_url.as_str())
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .status()
            .unwrap();
        assert!(routed.success());
        assert_eq!(
            proxy_server.join().unwrap()[0].path,
            "http://localhost:1/v1/systemone"
        );
    }

    /// Holds one TLS fixture endpoint, trust anchor, and observed protocol result.
    struct TlsFixture {
        url: reqwest::Url,
        certificate: Vec<u8>,
        server: thread::JoinHandle<(Vec<u8>, usize)>,
    }

    /// Starts a local TLS server advertising either HTTP/2 or HTTP/1.1.
    fn tls_fixture(h2_enabled: bool, reset_stream: bool) -> TlsFixture {
        use tokio_rustls::rustls::{ServerConfig, pki_types::PrivatePkcs8KeyDer};

        let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let certificate = generated.cert.der().to_vec();
        let key = PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der());
        let mut tls_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![generated.cert.der().clone()], key.into())
            .unwrap();
        tls_config.alpn_protocols = vec![if h2_enabled {
            b"h2".to_vec()
        } else {
            b"http/1.1".to_vec()
        }];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = reqwest::Url::parse(&format!(
            "https://localhost:{}/",
            listener.local_addr().unwrap().port()
        ))
        .unwrap();
        let server = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let (stream, _) = listener.accept().await.unwrap();
                let mut tls = acceptor.accept(stream).await.unwrap();
                let alpn = tls.get_ref().1.alpn_protocol().unwrap_or_default().to_vec();
                if h2_enabled {
                    let mut connection = h2::server::handshake(tls).await.unwrap();
                    let mut count = 0;
                    while let Ok(Some(Ok((request, mut respond)))) = tokio::time::timeout(
                        if count == 0 { Duration::from_secs(3) } else { Duration::from_millis(350) },
                        connection.accept(),
                    ).await {
                        count += 1;
                        if reset_stream {
                            respond.send_reset(h2::Reason::REFUSED_STREAM);
                            continue;
                        }
                        assert_eq!(request.uri().path(), "/v1/systemone");
                        let data = answer().to_string();
                        let response = http::Response::builder()
                            .status(200)
                            .header("content-type", "application/json")
                            .body(())
                            .unwrap();
                        respond.send_response(response, false).unwrap()
                            .send_data(Bytes::from(data), true).unwrap();
                    }
                    (alpn, count)
                } else {
                    let mut reader = tokio::io::BufReader::new(&mut tls);
                    let mut line = String::new();
                    reader.read_line(&mut line).await.unwrap();
                    assert!(line.starts_with("POST /v1/systemone HTTP/1.1"));
                    let mut content_length = 0;
                    loop {
                        line.clear();
                        reader.read_line(&mut line).await.unwrap();
                        if line == "\r\n" { break; }
                        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            content_length = value.trim().parse().unwrap();
                        }
                    }
                    let mut body = vec![0; content_length];
                    reader.read_exact(&mut body).await.unwrap();
                    let data = answer().to_string();
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}", data.len());
                    tls.write_all(response.as_bytes()).await.unwrap();
                    (alpn, 1)
                }
            })
        });
        TlsFixture {
            url,
            certificate,
            server,
        }
    }

    /// Negotiates h2 on capable HTTPS routes and HTTP/1.1 otherwise.
    #[test]
    fn tls_alpn_negotiates_h2_and_http1_fallback() {
        for (h2_enabled, expected_alpn) in
            [(true, b"h2".as_slice()), (false, b"http/1.1".as_slice())]
        {
            let TlsFixture {
                url,
                certificate,
                server,
            } = tls_fixture(h2_enabled, false);
            let client = JevClient {
                http: Arc::new(
                    Client::builder()
                        .no_proxy()
                        .retry(reqwest::retry::never())
                        .add_root_certificate(reqwest::Certificate::from_der(&certificate).unwrap())
                        .build()
                        .unwrap(),
                ),
            };
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let (_handle, signal) = CancelHandle::new();
                let result = client
                    .system_one(&request(), &config(url), &ApiKey::for_test("test"), signal)
                    .await;
                assert!(result.is_ok(), "{result:?}");
            });
            let (alpn, count) = server.join().unwrap();
            assert_eq!(alpn, expected_alpn);
            assert_eq!(count, 1);
        }
    }

    /// Leaves an HTTP/2 rejected stream un-retried when application retries are zero.
    #[test]
    fn h2_protocol_nack_obeys_zero_retry_budget() {
        let TlsFixture {
            url,
            certificate,
            server,
        } = tls_fixture(true, true);
        let client = JevClient {
            http: Arc::new(
                Client::builder()
                    .no_proxy()
                    .retry(reqwest::retry::never())
                    .add_root_certificate(reqwest::Certificate::from_der(&certificate).unwrap())
                    .build()
                    .unwrap(),
            ),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (_handle, signal) = CancelHandle::new();
            let error = client
                .system_one(&request(), &config(url), &ApiKey::for_test("test"), signal)
                .await
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Transport);
        });
        let (alpn, count) = server.join().unwrap();
        assert_eq!(alpn, b"h2");
        assert_eq!(count, 1);
    }

    /// Reports explicit-proxy failures without falling back or exposing userinfo.
    #[test]
    fn failed_explicit_proxy_does_not_fall_back_or_leak_credentials() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let url =
            reqwest::Url::parse(&format!("http://{}/", target.local_addr().unwrap())).unwrap();
        let pool = JevClientPool::new().unwrap();
        let policy = ProxyPolicy::Explicit("http://proxy-user:proxy-secret@127.0.0.1:1".to_owned());
        let client = pool.for_policy(&policy).unwrap();
        let mut settings = config(url);
        settings.proxy = policy;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (_handle, signal) = CancelHandle::new();
            let error = client
                .system_one(&request(), &settings, &ApiKey::for_test("test"), signal)
                .await
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Transport);
            assert!(!error.to_string().contains("proxy-secret"));
            assert!(!format!("{settings:?}").contains("proxy-secret"));
        });
        assert!(target.accept().is_err());
    }
}
