//! Supplies a local HTTP/2 fixture to command-level integration tests.

use serde_json::Value as JsonValue;

use crate::h2_fixture;

/// Captures one authenticated request from a local plugin command.
pub(crate) struct CapturedRequest {
    /// HTTP method sent by the command.
    pub(crate) method: String,
    /// Fixed API path sent by the command.
    pub(crate) path: String,
    /// Original target authority when routed through an HTTP proxy.
    pub(crate) authority: Option<String>,
    /// Caller authorization header, if one was sent.
    pub(crate) authorization: Option<String>,
    /// Parsed JSON request body, or null for bodyless requests.
    pub(crate) body: JsonValue,
}

/// Keeps a command fixture running until the caller collects its requests.
pub(crate) struct CommandTestServer(h2_fixture::TestServer);

impl CommandTestServer {
    /// Waits for the expected number of complete request bodies to reach the server.
    pub(crate) fn wait_for_requests(&self, expected: usize) {
        self.0.wait_for_requests(expected);
    }

    /// Stops the fixture and parses captured JSON request bodies.
    pub(crate) fn join(self) -> std::thread::Result<Vec<CapturedRequest>> {
        self.0.join().map(|requests| {
            requests
                .into_iter()
                .map(|request| CapturedRequest {
                    method: request.method,
                    path: request.path,
                    authority: request.authority,
                    authorization: request.authorization,
                    body: if request.body.is_empty() {
                        JsonValue::Null
                    } else {
                        serde_json::from_slice(&request.body).expect("parse request JSON")
                    },
                })
                .collect()
        })
    }
}

/// Serves one JSON body per expected HTTP/2 request.
pub(crate) fn serve(responses: Vec<JsonValue>) -> (String, CommandTestServer) {
    let expected = responses.len();
    let (root, requests) = h2_fixture::serve(expected, move |index, _| {
        h2_fixture::Response::json(200, responses[index].clone())
    });
    (root, CommandTestServer(requests))
}
