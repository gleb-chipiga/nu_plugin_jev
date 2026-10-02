//! Supplies a bounded local HTTP fixture to command-level integration tests.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

use serde_json::Value as JsonValue;

/// Captures one authenticated request from a local plugin command.
pub(crate) struct CapturedRequest {
    /// HTTP method sent by the command.
    pub(crate) method: String,
    /// Fixed API path sent by the command.
    pub(crate) path: String,
    /// Caller authorization header, if one was sent.
    pub(crate) authorization: Option<String>,
    /// Parsed JSON request body, or null for bodyless requests.
    pub(crate) body: JsonValue,
}

/// Serves one JSON body per expected request on a dedicated blocking thread.
pub(crate) fn serve(
    responses: Vec<JsonValue>,
) -> (String, thread::JoinHandle<Vec<CapturedRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local mock server");
    listener
        .set_nonblocking(true)
        .expect("configure local mock server");
    let base_url = format!("http://{}", listener.local_addr().expect("local address"));
    let handle = thread::spawn(move || {
        responses
            .into_iter()
            .map(|response| {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("mock server did not receive request: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("configure request timeout");
                let mut reader = BufReader::new(stream.try_clone().expect("clone request stream"));
                let mut line = String::new();
                reader.read_line(&mut line).expect("read request line");
                let parts: Vec<_> = line.split_whitespace().collect();
                let method = parts[0].to_owned();
                let path = parts[1].to_owned();
                let mut authorization = None;
                let mut content_length = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).expect("read request header");
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        let value = value.trim();
                        if name.eq_ignore_ascii_case("authorization") {
                            authorization = Some(value.to_owned());
                        }
                        if name.eq_ignore_ascii_case("content-length") {
                            content_length = value.parse().expect("parse body length");
                        }
                    }
                }
                let mut body = vec![0; content_length];
                reader.read_exact(&mut body).expect("read complete body");
                let response = response.to_string();
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                );
                stream
                    .write_all(headers.as_bytes())
                    .expect("write response headers");
                stream
                    .write_all(response.as_bytes())
                    .expect("write response body");
                CapturedRequest {
                    method,
                    path,
                    authorization,
                    body: if body.is_empty() {
                        JsonValue::Null
                    } else {
                        serde_json::from_slice(&body).expect("parse request JSON")
                    },
                }
            })
            .collect()
    });
    (base_url, handle)
}
