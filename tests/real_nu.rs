//! Exercises streaming annotation through the actual Nushell plugin protocol.

use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use indoc::indoc;

/// Runs ready-made HTTP/2 fixtures for the actual plugin binary.
#[path = "support/h2_fixture.rs"]
mod h2_fixture;

/// Proves same-process command overlap, aggregate admission, and caller isolation.
#[path = "real_nu/concurrency.rs"]
mod concurrency;

/// Prevents native test children from selecting real user settings, files, or credentials.
fn isolated_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("real-nu-environment-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create isolated native-test environment");
    let mut command = Command::new(program);
    command
        .current_dir(&directory)
        .env("PWD", &directory)
        .env("XDG_CONFIG_HOME", &directory)
        .env("NU_PLUGIN_JEV_MAX_IN_FLIGHT", "128")
        .env("NU_PLUGIN_JEV_LOG", "off")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost");
    for name in [
        "TYPESAFE_API_KEY",
        "NU_PLUGIN_JEV_CONFIG",
        "NU_PLUGIN_JEV_MODEL",
        "NU_PLUGIN_JEV_BASE_URL",
        "NU_PLUGIN_JEV_TIMEOUT_MS",
        "NU_PLUGIN_JEV_JOBS",
        "NU_PLUGIN_JEV_RETRIES",
        "NU_PLUGIN_JEV_PROXY",
        "NU_PLUGIN_JEV_LOG_FORMAT",
    ] {
        command.env_remove(name);
    }
    command
}

/// Rejects invalid process limits before entering the plugin protocol without echoing values.
#[test]
fn invalid_startup_limit_has_a_redacted_diagnostic() {
    for value in [
        "",
        "0",
        "-1",
        "2.5",
        "private-value",
        "99999999999999999999999999999",
    ] {
        let output = isolated_command(env!("CARGO_BIN_EXE_nu_plugin_jev"))
            .arg("--help")
            .env("NU_PLUGIN_JEV_MAX_IN_FLIGHT", value)
            .env("NU_PLUGIN_JEV_LOG", "off")
            .output()
            .expect("start plugin with invalid limit");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(diagnostic.contains("NU_PLUGIN_JEV_MAX_IN_FLIGHT"));
        assert!(!diagnostic.contains("panicked"));
        assert!(!diagnostic.contains("private-value"));
    }
}

/// Accepts absent and positive process limits without needing a key or network.
#[test]
fn valid_startup_limit_serves_offline_help() {
    for value in [None, Some("2")] {
        let mut command = isolated_command(env!("CARGO_BIN_EXE_nu_plugin_jev"));
        command
            .arg("--help")
            .env("NU_PLUGIN_JEV_LOG", "off")
            .env_remove("NU_PLUGIN_JEV_CONFIG")
            .env(
                "XDG_CONFIG_HOME",
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/target/isolated-nu-test-config"
                ),
            );
        match value {
            Some(value) => command.env("NU_PLUGIN_JEV_MAX_IN_FLIGHT", value),
            None => command.env_remove("NU_PLUGIN_JEV_MAX_IN_FLIGHT"),
        };
        let output = command.output().expect("start plugin with valid limit");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Non-Unicode process values fail startup without lossy diagnostic contents.
#[cfg(unix)]
#[test]
fn non_unicode_startup_limit_fails_safely() {
    use std::os::unix::ffi::OsStrExt;

    let output = isolated_command(env!("CARGO_BIN_EXE_nu_plugin_jev"))
        .arg("--help")
        .env(
            "NU_PLUGIN_JEV_MAX_IN_FLIGHT",
            std::ffi::OsStr::from_bytes(b"private-\xff-value"),
        )
        .env("NU_PLUGIN_JEV_LOG", "off")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("NU_PLUGIN_JEV_MAX_IN_FLIGHT"));
    assert!(!diagnostic.contains("private"));
    assert!(!diagnostic.contains("panicked"));
}

/// Parses plugin diagnostics with Nu's single-record and newline-delimited NUON readers.
#[cfg(feature = "nuon-tracing-format")]
fn parse_nuon_diagnostics_in_nu(stderr: &[u8]) -> Vec<serde_json::Value> {
    let diagnostics = String::from_utf8(stderr.to_vec()).expect("UTF-8 diagnostics");
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--commands",
            concat!(
                "use std/formats *; ",
                "{single: ($env.JEV_TEST_DIAGNOSTICS | lines | first | from nuon), ",
                "batch: ($env.JEV_TEST_DIAGNOSTICS | from ndnuon)} | to json --raw"
            ),
        ])
        .env("JEV_TEST_DIAGNOSTICS", &diagnostics)
        .output()
        .expect("parse diagnostics in Nu");
    assert!(
        output.status.success(),
        "Nu could not parse diagnostics: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parsed Nu diagnostics");
    let batch = parsed["batch"].as_array().expect("NDNUON records");
    assert_eq!(batch.len(), diagnostics.lines().count());
    assert_eq!(parsed["single"], batch[0]);
    batch.clone()
}

/// Returns the local evaluation identity recorded on a diagnostic span.
#[cfg(feature = "nuon-tracing-format")]
fn diagnostic_request_id(record: &serde_json::Value) -> Option<&str> {
    record["spans"]
        .as_array()?
        .iter()
        .find_map(|span| span["fields"]["request_id"].as_str())
}

/// Responds to local System One requests until the subprocess completes.
fn serve_until_stopped(selective: bool) -> (String, Arc<AtomicUsize>, h2_fixture::TestServer) {
    let calls = Arc::new(AtomicUsize::new(0));
    let server_calls = Arc::clone(&calls);
    let (root, server) = h2_fixture::serve_unbounded(move |_, request| {
        server_calls.fetch_add(1, Ordering::SeqCst);
        let state: serde_json::Value = serde_json::from_slice(&request.body).expect("request body");
        let id = state["state"]["message"].as_i64().expect("selected row id");
        let probability = if !selective || id % 5 == 0 { 0.9 } else { 0.1 };
        h2_fixture::Response::json(
            200,
            serde_json::json!({
                "model": "jev-fixed",
                "answers": {"match": {"type": "noul", "noul": probability}},
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }),
        )
    });
    (root, calls, server)
}

/// Runs the same native Nu pipeline against an isolated local mock service.
fn run_pipeline(selective: bool) -> Option<(Vec<serde_json::Value>, usize)> {
    if isolated_command("nu").arg("--version").output().is_err() {
        return None;
    }
    let (base_url, calls, server) = serve_until_stopped(selective);
    let source = if selective {
        concat!(
            "let q = {match: (jev question noul 'Is this a match?')}; ",
            "1..1000 | each { |id| {id: $id, message: ($id mod 5)} } ",
            "| jev annotate $q --fields [message] --jobs 4 ",
            "| where answers.match.noul > 0.5 | first 10 | to json --raw"
        )
    } else {
        concat!(
            "let q = {match: (jev question noul 'Is this a match?')}; ",
            "1..1000 | each { |id| {id: $id, message: $id} } ",
            "| jev annotate $q --fields [message] -j 4 ",
            "| where answers.match.noul > 0.5 | first 10 | to json --raw"
        )
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut child = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .env("TYPESAFE_API_KEY", "local-key")
        .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start isolated Nu");
    loop {
        if child.try_wait().expect("poll Nu").is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            server.join().expect("join local mock after timeout");
            panic!("Nu pipeline did not stop after first 10 rows");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().expect("read Nu output");
    server.join().expect("join local mock");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu JSON table");
    let rows = rows.as_array().expect("row list").clone();
    Some((rows, calls.load(Ordering::SeqCst)))
}

/// Verifies ordinary early termination with ten accepted rows.
#[test]
fn annotate_where_first_ten_stays_bounded() {
    let Some((rows, calls)) = run_pipeline(false) else {
        return;
    };
    assert_eq!(rows.len(), 10);
    assert_eq!(rows[0]["id"], 1);
    // Nu may buffer beyond `first 10`; plugin admission has a separate exact bound.
    assert!(
        calls <= 64,
        "too many requests after first 10 rows: {calls}"
    );
}

/// Verifies rejected rows do not defeat backpressure before ten matching decisions.
#[test]
fn annotate_where_rejects_many_rows_before_first_ten() {
    let Some((rows, calls)) = run_pipeline(true) else {
        return;
    };
    assert_eq!(rows.len(), 10);
    assert_eq!(
        rows.iter()
            .map(|row| row["id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        (1..=10).map(|value| value * 5).collect::<Vec<_>>()
    );
    assert_eq!(
        calls, 5,
        "five projected states should each be evaluated once"
    );
}

/// Serves one complete mixed-answer envelope and returns the captured request body.
fn serve_mixed_once() -> (String, h2_fixture::TestServer) {
    h2_fixture::serve(1, |_, _| {
        h2_fixture::Response::json(
            200,
            serde_json::json!({"model": "jev-fixed", "answers": {
            "spam": {"type": "noul", "noul": 0.982},
            "kind": {"type": "choice", "choice": "spam", "confidence": 0.91,
                "probabilities": {"normal": 0.09, "spam": 0.91}},
            "urgency": {"type": "score", "score": 1.4, "confidence": 0.81,
                "legend": {"0": "later", "1": "today", "2": "now"},
                "probabilities": {"0": 0.1, "1": 0.4, "2": 0.5}}
        }, "usage": {"input_tokens": 42, "output_tokens": 6}}),
        )
    })
}

/// Verifies native projections and caller thresholds over all three answer variants.
#[test]
fn ask_native_get_keeps_mixed_answer_details() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, server) = serve_mixed_once();
    let source = concat!(
        "let q = {spam: {type: noul}, kind: {type: choice, ",
        "criteria: {normal: null, spam: null}}, urgency: {type: score, ",
        "criteria: ['later' 'today' 'now']}}; ",
        "let result = ('hello' | jev ask $q); ",
        "{probability: ($result | get answers.spam.noul), ",
        "passes: (($result | get answers.spam.noul) > 0.98), ",
        "kind: ($result | get answers.kind.choice), ",
        "confidence: ($result | get answers.kind.confidence), ",
        "distribution: ($result | get answers.kind.probabilities), ",
        "score: ($result | get answers.urgency.score), ",
        "legend: ($result | get answers.urgency.legend), ",
        "model: ($result | get meta.model)} | to json --raw"
    );
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .env("TYPESAFE_API_KEY", "local-key")
        .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
        .env("NU_PLUGIN_JEV_LOG", "debug")
        .output()
        .expect("run isolated Nu");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let wire: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu JSON result");
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostics.contains("evaluation started"));
    assert!(diagnostics.contains("evaluation completed"));
    assert!(diagnostics.contains("command=\"ask\""));
    assert!(diagnostics.contains(&format!("base_url={base_url}/")));
    for field in [
        "request_bytes=",
        "response_bytes=",
        "elapsed_ns=",
        "attempt_elapsed_ns=",
        "attempts=1",
        if cfg!(feature = "http2-prior-knowledge") {
            "http_version=\"HTTP/2\""
        } else {
            "http_version=\"HTTP/1.1\""
        },
    ] {
        assert!(diagnostics.contains(field), "missing {field}");
    }
    for secret in ["local-key", "hello", "Authorization"] {
        assert!(!diagnostics.contains(secret));
    }
    assert_eq!(wire["probability"], 0.982);
    assert_eq!(wire["passes"], true);
    assert_eq!(wire["kind"], "spam");
    assert_eq!(wire["confidence"], 0.91);
    assert_eq!(
        wire["distribution"],
        serde_json::json!({"normal": 0.09, "spam": 0.91})
    );
    assert_eq!(wire["score"], 1.4);
    assert_eq!(wire["legend"]["1"], "today");
    assert_eq!(wire["model"], "jev-fixed");
    let captured = server.join().expect("join mixed mock");
    let request = &captured[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/systemone");
    assert_eq!(request.authorization.as_deref(), Some("Bearer local-key"));
    assert_eq!(request.content_type.as_deref(), Some("application/json"));
    let expected_version = if cfg!(feature = "http2-prior-knowledge") {
        reqwest::Version::HTTP_2
    } else {
        reqwest::Version::HTTP_11
    };
    assert_eq!(request.version, expected_version);
    if expected_version == reqwest::Version::HTTP_2 {
        assert_eq!(
            request.authority.as_deref(),
            base_url.strip_prefix("http://")
        );
    }
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("request JSON");
    assert_eq!(body["state"], "hello");
    assert_eq!(body["questions"].as_object().unwrap().len(), 3);
}

/// Keeps retry attempts under one NUON evaluation span without logging secrets.
#[test]
#[cfg(feature = "nuon-tracing-format")]
fn ask_nuon_correlates_retry_attempts() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, server) = h2_fixture::serve(3, |attempt, _| {
        let request_id = ["req_retry_1", "req_local-key", "req_success_3"][attempt];
        let mut response = if attempt < 2 {
            let mut response = h2_fixture::Response::json(503, serde_json::json!({}));
            response.headers.push(("retry-after".into(), "0".into()));
            response
        } else {
            h2_fixture::Response::json(
                200,
                serde_json::json!({"model": "jev-fixed", "answers": {
                "match": {"type": "noul", "noul": 0.9}}, "usage": {
                "input_tokens": 2, "output_tokens": 1}}),
            )
        };
        response
            .headers
            .push(("x-typesafe-request-id".into(), request_id.into()));
        response
    });
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            concat!(
                "let result = ('hello' | jev ask {match: {type: noul}} ",
                "--timeout 5sec --metrics); ",
                "{answer: $result.answers.match.noul, meta: $result.meta, ",
                "metrics: $result.metrics, ",
                "elapsed_ns: ($result.metrics.elapsed | into int), ",
                "attempt_elapsed_ns: ($result.metrics.attempt_elapsed | into int)} ",
                "| to json --raw"
            ),
        ])
        .env("TYPESAFE_API_KEY", "local-key")
        .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
        .env("NU_PLUGIN_JEV_RETRIES", "2")
        .env("NU_PLUGIN_JEV_LOG", "debug")
        .env("NU_PLUGIN_JEV_LOG_FORMAT", "nuon")
        .output()
        .expect("run isolated Nu");
    server.join().expect("join retry mock");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu JSON result");
    assert_eq!(result["answer"], 0.9);
    assert_eq!(result["meta"]["base_url"], format!("{base_url}/"));
    assert_eq!(result["metrics"]["attempts"], 3);
    let records = parse_nuon_diagnostics_in_nu(&output.stderr);
    let attempts = records
        .iter()
        .filter(|record| record["message"] == "Jev HTTP attempt started")
        .collect::<Vec<_>>();
    assert_eq!(
        attempts
            .iter()
            .map(|record| record["fields"]["attempt"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    let responses = records
        .iter()
        .filter(|record| record["message"] == "Jev HTTP response received")
        .collect::<Vec<_>>();
    assert_eq!(
        responses
            .iter()
            .map(|record| record["fields"]["status"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [503, 503, 200]
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record["message"] == "retrying Jev request")
            .count(),
        2
    );
    let ids = records
        .iter()
        .filter(|record| {
            record["message"] == "Jev HTTP attempt started"
                || record["message"] == "Jev HTTP response received"
                || record["message"] == "retrying Jev request"
        })
        .filter_map(|record| diagnostic_request_id(record))
        .collect::<Vec<_>>();
    assert!(ids.len() >= 4);
    assert!(ids.iter().all(|id| *id == ids[0]));
    for (attempt, server_id) in [(1, "req_retry_1"), (3, "req_success_3")] {
        let record = responses
            .iter()
            .find(|record| record["fields"]["attempt"] == attempt)
            .expect("response diagnostic");
        assert_eq!(record["fields"]["server_request_id"], server_id);
        assert_eq!(diagnostic_request_id(record), Some(ids[0]));
    }
    let invalid_record = responses
        .iter()
        .find(|record| record["fields"]["attempt"] == 2)
        .expect("invalid-ID response diagnostic");
    assert!(invalid_record["fields"].get("server_request_id").is_none());
    let completed = records
        .iter()
        .find(|record| record["message"] == "evaluation completed")
        .expect("completed evaluation");
    assert_eq!(completed["fields"]["input_tokens"], 2);
    assert_eq!(completed["fields"]["output_tokens"], 1);
    assert_eq!(completed["fields"]["base_url"], result["meta"]["base_url"]);
    for name in [
        "request_bytes",
        "response_bytes",
        "attempts",
        "http_version",
    ] {
        assert_eq!(completed["fields"][name], result["metrics"][name]);
    }
    assert_eq!(completed["fields"]["elapsed_ns"], result["elapsed_ns"]);
    assert_eq!(
        completed["fields"]["attempt_elapsed_ns"],
        result["attempt_elapsed_ns"]
    );
    assert!(completed["fields"]["duration_ms"].as_u64().is_some());
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    for secret in ["local-key", "hello", "Authorization", "req_local-key"] {
        assert!(!diagnostics.contains(secret));
    }
}

/// Keeps offline previews free of fabricated successful HTTP diagnostics.
#[test]
fn dry_run_has_no_success_measurement_event() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let source = concat!(
        "let ask = ('hello' | jev ask {q: {type: noul}} --dry-run); ",
        "let annotate = ([{message: 'hello'}] ",
        "| jev annotate {q: {type: noul}} --dry-run); ",
        "{ask: $ask, annotate: $annotate} | to json --raw"
    );
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .env("NU_PLUGIN_JEV_LOG", "info")
        .output()
        .expect("run isolated Nu dry-run");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu JSON result");
    assert!(result["ask"]["request_bytes"].as_u64().is_some());
    assert!(result["ask"].get("meta").is_none());
    assert!(result["ask"].get("metrics").is_none());
    assert!(result["annotate"][0]["request_bytes"].as_u64().is_some());
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(!diagnostics.contains("evaluation completed"));
    assert!(!diagnostics.contains("model listing completed"));
}

/// Responds to three distinct projected states with typed, state-dependent decisions.
fn serve_mixed_table() -> (String, h2_fixture::TestServer) {
    h2_fixture::serve(3, |_, request| {
        let captured: serde_json::Value =
            serde_json::from_slice(&request.body).expect("request JSON");
        let message = captured["state"]["input"]["message"]
            .as_str()
            .expect("projected message");
        let (probability, choice, score, levels) = match message {
            "urgent" => (0.99, "spam", 1.8, [0.0, 0.2, 0.8]),
            "normal" => (0.1, "normal", 0.1, [0.9, 0.1, 0.0]),
            "later" => (0.95, "spam", 1.4, [0.1, 0.4, 0.5]),
            _ => panic!("unexpected mock state"),
        };
        let (normal, spam) = if choice == "spam" {
            (0.05, 0.95)
        } else {
            (0.95, 0.05)
        };
        h2_fixture::Response::json(
            200,
            serde_json::json!({"model": "jev-fixed", "answers": {
                "spam": {"type": "noul", "noul": probability},
                "kind": {"type": "choice", "choice": choice, "confidence": 0.95,
                    "probabilities": {"normal": normal, "spam": spam}},
                "urgency": {"type": "score", "score": score, "confidence": 0.8,
                    "legend": {"0": "later", "1": "today", "2": "now"},
                    "probabilities": {"0": levels[0], "1": levels[1], "2": levels[2]}}
            }, "usage": {"input_tokens": 10, "output_tokens": 3}}),
        )
    })
}

/// Verifies native table transforms over typed answers and projected outbound state.
#[test]
fn annotate_composes_with_native_table_commands() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, server) = serve_mixed_table();
    let source = indoc! {r#"
        let q = {
            spam: {type: noul}
            kind: {type: choice, criteria: {normal: null, spam: null}}
            urgency: {type: score, criteria: ['later' 'today' 'now']}
        }

        let rows = ([
            {id: 1, message: 'urgent', sender: 'a', secret: 'local-1'}
            {id: 2, message: 'normal', sender: 'b', secret: 'local-2'}
            {id: 3, message: 'later', sender: 'c', secret: 'local-3'}
        ]
            | jev annotate $q --fields [message sender] --context {
                policy: 'rules'
            } --metrics --jobs 2
            | select id message sender secret answers jev_meta jev_metrics)
        {
            all: $rows
            filtered: (
                $rows | where answers.spam.noul > 0.9
                | sort-by answers.urgency.score --reverse
                | reject jev_meta jev_metrics
            )
        } | to json --raw
    "#};
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .env("TYPESAFE_API_KEY", "local-key")
        .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
        .env("NU_PLUGIN_JEV_LOG", "debug")
        .output()
        .expect("run isolated Nu");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let wire: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu JSON table");
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "evaluation started",
        "evaluation completed",
        "Jev HTTP attempt started",
    ] {
        assert!(
            diagnostics.contains(expected),
            "missing diagnostic: {expected}"
        );
    }
    assert!(
        diagnostics.contains(
            wire["all"][0]["jev_metrics"]["request_id"]
                .as_str()
                .unwrap()
        )
    );
    for secret in ["local-key", "local-1", "urgent"] {
        assert!(!diagnostics.contains(secret));
    }
    assert_eq!(wire["all"].as_array().unwrap().len(), 3);
    assert_eq!(wire["all"][0]["secret"], "local-1");
    assert_eq!(wire["all"][0]["jev_meta"]["model"], "jev-fixed");
    assert_eq!(wire["all"][0]["jev_meta"]["usage"]["input_tokens"], 10);
    assert_eq!(wire["filtered"].as_array().unwrap().len(), 2);
    assert_eq!(wire["filtered"][0]["id"], 1);
    assert_eq!(wire["filtered"][1]["id"], 3);
    assert_eq!(wire["filtered"][0]["answers"]["kind"]["choice"], "spam");
    assert!(wire["filtered"][0].get("jev_meta").is_none());
    let captured = server.join().expect("join table mock");
    assert_eq!(captured.len(), 3);
    for request in captured {
        let request: serde_json::Value =
            serde_json::from_slice(&request.body).expect("request JSON");
        assert_eq!(request["questions"].as_object().unwrap().len(), 3);
        assert_eq!(
            request["state"]["context"],
            serde_json::json!({"policy": "rules"})
        );
        assert!(request["state"]["input"].get("secret").is_none());
        assert!(request["state"]["input"].get("id").is_none());
        assert!(request["state"]["input"].get("message").is_some());
        assert!(request["state"]["input"].get("sender").is_some());
    }
}

/// Keeps the default answer path aligned with ask while preserving legacy and metadata fields.
#[test]
fn annotate_default_answers_and_explicit_legacy_path_in_real_nu() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, calls, server) = serve_until_stopped(false);
    let source = indoc! {r#"
        let q = {match: {type: noul}}
        let default = ([{id: 1, message: 7}] | jev annotate $q --fields [message] --metrics | first)
        let legacy = ([{id: 2, message: 7}]
            | jev annotate $q --fields [message] --into jev --metrics | first)
        let shared = ([{id: 3, message: 7} {id: 4, message: 7}]
            | jev annotate $q --fields [message] --metrics --jobs 1)
        let kept = ([{id: 5}] | jev annotate $q --fields [message] --on-error keep | first)
        let recorded = ([{id: 6}] | jev annotate $q --fields [message] --on-error record | first)
        let empty = ([] | jev annotate $q --fields [message] | length)
        let ordered = ([{id: 7, message: 1} {id: 8, message: 2}]
            | jev annotate $q --fields [message] --jobs 2 | get id)
        let unordered = ([{id: 9, message: 3} {id: 10, message: 4}]
            | jev annotate $q --fields [message] --jobs 2 --unordered | get id)
        {
            default: $default
            legacy: $legacy
            shared: $shared
            kept: $kept
            recorded: $recorded
            empty: $empty
            ordered: $ordered
            unordered: $unordered
        } | to json --raw
    "#};
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .env("TYPESAFE_API_KEY", "local-key")
        .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
        .output()
        .expect("run isolated Nu");
    server.join().expect("join local mock");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu result");
    assert_eq!(result["default"]["answers"]["match"]["noul"], 0.9);
    assert!(result["default"].get("jev").is_none());
    assert_eq!(result["default"]["jev_meta"]["model"], "jev-fixed");
    assert_eq!(result["default"]["jev_metrics"]["attempts"], 1);
    assert_eq!(result["legacy"]["jev"]["match"]["noul"], 0.9);
    assert!(result["legacy"].get("answers").is_none());
    assert_eq!(result["legacy"]["jev_meta"], result["default"]["jev_meta"]);
    assert!(result["legacy"]["jev_metrics"]["request_id"].is_string());
    let shared = result["shared"].as_array().expect("shared rows");
    assert_eq!(shared.len(), 2);
    assert!(
        shared
            .iter()
            .all(|row| row["answers"]["match"]["noul"] == 0.9)
    );
    assert_eq!(shared[0]["jev_meta"], shared[1]["jev_meta"]);
    assert_eq!(shared[0]["jev_metrics"], shared[1]["jev_metrics"]);
    assert_eq!(result["kept"], serde_json::json!({"id": 5}));
    assert_eq!(result["recorded"]["jev_error"]["kind"], "state");
    assert!(result["recorded"].get("answers").is_none());
    assert!(result["recorded"].get("jev_meta").is_none());
    assert_eq!(result["empty"], 0);
    assert_eq!(result["ordered"], serde_json::json!([7, 8]));
    let mut unordered = result["unordered"]
        .as_array()
        .expect("unordered rows")
        .iter()
        .map(|value| value.as_i64().expect("row ID"))
        .collect::<Vec<_>>();
    unordered.sort_unstable();
    assert_eq!(unordered, [9, 10]);
    assert_eq!(calls.load(Ordering::SeqCst), 7);
}

/// Rejects new-default source collisions and removed destination flags before HTTP dispatch.
#[test]
fn annotate_default_destination_collisions_are_terminal_in_real_nu() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, calls, server) = serve_until_stopped(false);
    for source in [
        concat!(
            "[{id: 1, message: 7, answers: 'existing'}] ",
            "| jev annotate {match: {type: noul}} --fields [message] ",
            "--on-error keep | to json --raw"
        ),
        concat!(
            "[{id: 1, message: 7, answers: 'existing'}] ",
            "| jev annotate {match: {type: noul}} --fields [message] ",
            "--on-error record | to json --raw"
        ),
        "[] | jev annotate {match: {type: noul}} --meta-into ai | to json --raw",
        "[] | jev annotate {match: {type: noul}} --metrics-into ai | to json --raw",
    ] {
        let output = isolated_command("nu")
            .args([
                "--no-config-file",
                "--plugins",
                env!("CARGO_BIN_EXE_nu_plugin_jev"),
                "--commands",
                source,
            ])
            .env("TYPESAFE_API_KEY", "local-key")
            .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
            .output()
            .expect("run isolated Nu");
        assert!(
            !output.status.success(),
            "conflicting annotation unexpectedly succeeded: {source}"
        );
        assert!(output.stdout.is_empty(), "conflicting row reached output");
    }
    server.join().expect("join local mock");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// Confirms NUON cache-hit records correlate with row metadata in real Nu.
#[test]
#[cfg(feature = "nuon-tracing-format")]
fn annotate_nuon_correlates_cached_rows() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, calls, server) = serve_until_stopped(false);
    let source = concat!(
        "let rows = ([{id: 1, message: 7} {id: 2, message: 7} {id: 3, message: 7}] ",
        "| jev annotate {match: {type: noul}} --fields [message] ",
        "--jobs 1 --metrics | select id jev_meta jev_metrics); ",
        "{rows: $rows, elapsed_ns: ($rows | get 0.jev_metrics.elapsed | into int), ",
        "attempt_elapsed_ns: ($rows | get 0.jev_metrics.attempt_elapsed | into int)} ",
        "| to json --raw"
    );
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .env("TYPESAFE_API_KEY", "local-key")
        .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
        .env("NU_PLUGIN_JEV_LOG", "debug")
        .env("NU_PLUGIN_JEV_LOG_FORMAT", "nuon")
        .output()
        .expect("run isolated Nu");
    server.join().expect("join local mock");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu JSON table");
    let rows = result["rows"].as_array().expect("annotated rows");
    assert_eq!(rows.len(), 3);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let request_id = rows[0]["jev_metrics"]["request_id"]
        .as_str()
        .expect("request identity");
    assert!(
        rows.iter()
            .all(|row| row["jev_metrics"]["request_id"] == request_id)
    );
    let records = parse_nuon_diagnostics_in_nu(&output.stderr);
    let hits = records
        .iter()
        .filter(|record| record["message"] == "completed Jev result cache hit")
        .collect::<Vec<_>>();
    assert!(!hits.is_empty(), "expected a completed-result cache hit");
    assert!(
        hits.iter()
            .all(|record| record["fields"]["request_id"] == request_id)
    );
    let completed = records
        .iter()
        .filter(|record| record["message"] == "evaluation completed")
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 1);
    assert_eq!(
        completed[0]["fields"]["base_url"],
        rows[0]["jev_meta"]["base_url"]
    );
    assert_eq!(
        completed[0]["fields"]["input_tokens"],
        rows[0]["jev_meta"]["usage"]["input_tokens"]
    );
    for name in [
        "request_bytes",
        "response_bytes",
        "attempts",
        "http_version",
    ] {
        assert_eq!(completed[0]["fields"][name], rows[0]["jev_metrics"][name]);
    }
    assert_eq!(completed[0]["fields"]["elapsed_ns"], result["elapsed_ns"]);
    assert_eq!(
        completed[0]["fields"]["attempt_elapsed_ns"],
        result["attempt_elapsed_ns"]
    );
    assert_eq!(diagnostic_request_id(completed[0]), Some(request_id));
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(!diagnostics.contains("local-key"));
}

/// Applies a changed logging level only after the plugin process restarts.
#[test]
fn tracing_level_changes_after_plugin_restart() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, calls, server) = serve_until_stopped(false);
    let source = indoc! {r#"
        let q = {match: {type: noul}}
        let first = ({message: 1} | jev ask $q | get answers.match.noul)
        $env.NU_PLUGIN_JEV_LOG = "info"
        let second = ({message: 2} | jev ask $q | get answers.match.noul)
        plugin stop jev
        let third = ({message: 3} | jev ask $q | get answers.match.noul)
        [$first $second $third] | to json --raw
    "#};
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .env("TYPESAFE_API_KEY", "local-key")
        .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
        .env("NU_PLUGIN_JEV_LOG", "off")
        .env("JEV_LOG", "debug")
        .output()
        .expect("run isolated Nu");
    server.join().expect("join local mock");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let answers: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu JSON list");
    assert_eq!(answers, serde_json::json!([0.9, 0.9, 0.9]));
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert_eq!(diagnostics.matches("evaluation started").count(), 1);
    assert_eq!(diagnostics.matches("evaluation completed").count(), 1);
    for secret in ["local-key", "\"message\"", "Authorization"] {
        assert!(!diagnostics.contains(secret));
    }
}

/// Keeps the installed Nu namespace limited to the seven declared commands.
#[test]
fn real_nu_registers_only_the_seven_core_commands() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            "scope commands | where name =~ '^jev' | get name | to json --raw",
        ])
        .output()
        .expect("run isolated Nu");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let names: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("Nu command inventory");
    assert_eq!(
        names,
        serde_json::json!([
            "jev",
            "jev annotate",
            "jev ask",
            "jev models",
            "jev question choice",
            "jev question noul",
            "jev question score"
        ])
    );
}

/// Shows only the selected short aliases in real Nushell command help.
#[test]
fn real_nu_help_lists_focused_short_options() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            concat!(
                "{ask: (help jev ask), annotate: (help jev annotate), ",
                "models: (help jev models)} | to json --raw"
            ),
        ])
        .output()
        .expect("run isolated Nu");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu help text");
    let ask = help["ask"].as_str().expect("ask help");
    let annotate = help["annotate"].as_str().expect("annotate help");
    let models = help["models"].as_str().expect("models help");
    for line in ["-c, --context", "-m, --model"] {
        assert!(ask.contains(line));
        assert!(annotate.contains(line));
    }
    for line in ["-j, --jobs", "-s, --state", "-f, --fields", "-i, --into"] {
        assert!(annotate.contains(line));
        assert!(!ask.contains(line));
    }
    for help in [ask, annotate] {
        for long_only in ["--base-url", "--timeout", "--dry-run"] {
            assert!(help.contains(long_only));
        }
        assert!(help.contains("NU_PLUGIN_JEV_CONFIG"));
        assert!(help.contains(".nu_plugin_jev.nuon"));
        assert!(!help.contains("-b, --base-url"));
        assert!(!help.contains("-t, --timeout"));
    }
    for long_only in ["--base-url", "--timeout", "--config"] {
        assert!(models.contains(long_only));
    }
    for absent in ["--model", "--jobs", "--dry-run"] {
        assert!(!models.contains(absent));
    }
}

/// Serves two independent model catalogs while recording bodyless GET paths.
fn serve_model_catalogs(names: &[&str]) -> (String, h2_fixture::TestServer) {
    let names = names
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    h2_fixture::serve(names.len(), move |index, request| {
        assert!(request.body.is_empty(), "model request must be bodyless");
        h2_fixture::Response::json(
            200,
            serde_json::json!({"models": [{
                "name": names[index], "description": "Mock model", "release_date": "opaque"
            }]}),
        )
    })
}

/// Composes model rows with native Nu commands and observes fresh service data.
#[test]
fn real_nu_models_are_fresh_native_records() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let (url, server) = serve_model_catalogs(&["first", "second"]);
    let source = concat!(
        "{first: (jev models | get models | sort-by name ",
        "| select name description release_date), ",
        "second: (jev models | get models | where name == 'second' ",
        "| select name release_date)} | to json --raw"
    );
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .env("TYPESAFE_API_KEY", "local-model-key")
        .env("NU_PLUGIN_JEV_BASE_URL", url)
        .output()
        .expect("run isolated Nu model pipeline");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu model JSON");
    assert_eq!(
        value,
        serde_json::json!({
        "first": [{"name": "first", "description": "Mock model", "release_date": "opaque"}],
        "second": [{"name": "second", "release_date": "opaque"}]
        })
    );
    assert_eq!(
        server
            .join()
            .unwrap()
            .into_iter()
            .map(|request| (request.method, request.path))
            .collect::<Vec<_>>(),
        vec![
            ("GET".to_owned(), "/v1/models".to_owned()),
            ("GET".to_owned(), "/v1/models".to_owned()),
        ]
    );
}

/// Correlates unflagged and measured catalog lookups with NUON completion events.
#[test]
#[cfg(feature = "nuon-tracing-format")]
fn models_nuon_completion_matches_optional_metrics() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, server) = serve_model_catalogs(&["first", "second"]);
    let source = concat!(
        "let first = (jev models); let second = (jev models --metrics); ",
        "{first: $first, second: $second, ",
        "elapsed_ns: ($second.metrics.elapsed | into int), ",
        "attempt_elapsed_ns: ($second.metrics.attempt_elapsed | into int)} ",
        "| to json --raw"
    );
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .env("TYPESAFE_API_KEY", "local-model-key")
        .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
        .env("NU_PLUGIN_JEV_LOG", "info")
        .env("NU_PLUGIN_JEV_LOG_FORMAT", "nuon")
        .output()
        .expect("run isolated Nu model lookup");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu JSON result");
    assert_eq!(result["first"]["models"][0]["name"], "first");
    assert!(result["first"].get("metrics").is_none());
    assert_eq!(result["second"]["models"][0]["name"], "second");
    assert_eq!(result["second"]["metrics"]["request_bytes"], 0);
    let records = parse_nuon_diagnostics_in_nu(&output.stderr);
    let completed = records
        .iter()
        .filter(|record| record["message"] == "model listing completed")
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 2);
    assert_eq!(
        completed[0]["fields"]["base_url"],
        result["first"]["meta"]["base_url"]
    );
    assert_eq!(completed[0]["fields"]["request_bytes"], 0);
    assert_eq!(completed[0]["fields"]["count"], 1);
    assert!(completed[0]["fields"].get("input_tokens").is_none());
    assert_eq!(
        completed[1]["fields"]["base_url"],
        result["second"]["meta"]["base_url"]
    );
    for name in [
        "request_bytes",
        "response_bytes",
        "attempts",
        "http_version",
    ] {
        assert_eq!(
            completed[1]["fields"][name],
            result["second"]["metrics"][name]
        );
    }
    assert_eq!(completed[1]["fields"]["elapsed_ns"], result["elapsed_ns"]);
    assert_eq!(
        completed[1]["fields"]["attempt_elapsed_ns"],
        result["attempt_elapsed_ns"]
    );
    assert_eq!(server.join().unwrap().len(), 2);
}

/// Reloads user NUON and honors explicit local selection during one models session.
#[test]
fn real_nu_models_reload_nuon_and_honor_explicit_config() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let root = std::env::temp_dir().join(format!("jev-model-nuon-reload-{}", std::process::id()));
    let user = root.join("user");
    std::fs::create_dir_all(user.join("nu_plugin_jev")).expect("user config directory");
    let config = user.join("nu_plugin_jev/config.nuon");
    std::fs::write(&config, "{timeout_ms: 5000}").expect("initial user NUON");
    let (url, server) = serve_model_catalogs(&["first", "explicit"]);
    let explicit = root.join("selected.nuon");
    std::fs::write(
        &explicit,
        format!("{{base_url: '{url}', timeout_ms: 5000}}"),
    )
    .expect("explicit local NUON");
    let source = indoc! {r#"
        let first = (jev models --base-url '{URL}' | get models.0.name)
        {timeout_ms: 0} | save --force '{CONFIG}'
        let second_error = (try { jev models --base-url '{URL}' } catch {|err| $err.msg })
        $env.NU_PLUGIN_JEV_CONFIG = 'missing-env-selected.nuon'
        let explicit = (jev models --config '{EXPLICIT}' | get models.0.name)
        {first: $first, second_error: $second_error, explicit: $explicit} | to json --raw
    "#}
    .replace("{URL}", &url)
    .replace("{CONFIG}", &config.to_string_lossy())
    .replace("{EXPLICIT}", &explicit.to_string_lossy());
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            &source,
        ])
        .current_dir(&root)
        .env("XDG_CONFIG_HOME", &user)
        .env("TYPESAFE_API_KEY", "local-model-key")
        .env_remove("NU_PLUGIN_JEV_CONFIG")
        .env_remove("NU_PLUGIN_JEV_TIMEOUT_MS")
        .output()
        .expect("run isolated Nu model reload pipeline");
    std::fs::remove_dir_all(&root).expect("remove isolated fixture");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu JSON");
    assert_eq!(result["first"], "first");
    assert!(
        result["second_error"]
            .as_str()
            .is_some_and(|error| error.contains("user NUON timeout_ms")),
        "expected updated NUON to fail validation: {result}"
    );
    assert_eq!(result["explicit"], "explicit");
    assert_eq!(
        server
            .join()
            .expect("join model mock")
            .into_iter()
            .map(|request| (request.method, request.path))
            .collect::<Vec<_>>(),
        vec![("GET".to_owned(), "/v1/models".to_owned()); 2]
    );
}

/// Rejects an actual lazy Nu stream without requiring an API key or HTTP server.
#[test]
fn real_nu_models_reject_stream_input() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            "1..10 | each { |n| $n } | jev models",
        ])
        .env("TYPESAFE_API_KEY", "")
        .output()
        .expect("run isolated Nu stream rejection");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not accept pipeline input"));
}

/// Accepts and retains requests without replying until the test stops the server.
#[cfg(unix)]
fn serve_stalled() -> (String, Arc<AtomicUsize>, h2_fixture::TestServer) {
    let calls = Arc::new(AtomicUsize::new(0));
    let server_calls = Arc::clone(&calls);
    let (root, server) = h2_fixture::serve_unbounded(move |_, _| {
        server_calls.fetch_add(1, Ordering::SeqCst);
        let mut response = h2_fixture::Response::json(200, serde_json::json!({}));
        response.delay = Duration::from_secs(60);
        response
    });
    (root, calls, server)
}

/// Interrupts a stalled real Nu stream and confirms bounded, prompt teardown.
#[cfg(unix)]
#[test]
fn interrupt_stalled_annotation_stops_local_work() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, calls, server) = serve_stalled();
    let source = concat!(
        "let q = {match: (jev question noul 'Match?')}; ",
        "1..1000 | each { |id| {id: $id, message: $id} } ",
        "| jev annotate $q --fields [message] --jobs 4 ",
        "| first 10 | to json --raw"
    );
    let mut child = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .env("TYPESAFE_API_KEY", "local-key")
        .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start isolated Nu");
    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while calls.load(Ordering::SeqCst) == 0 && Instant::now() < ready_deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        calls.load(Ordering::SeqCst) > 0,
        "Nu never started its HTTP work"
    );
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("send Nu interrupt");
    assert!(status.success());
    let shutdown_deadline = Instant::now() + Duration::from_secs(5);
    let exited = loop {
        if child.try_wait().expect("poll interrupted Nu").is_some() {
            break true;
        }
        if Instant::now() >= shutdown_deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !exited {
        let _ = child.kill();
    }
    let _ = child.wait_with_output();
    thread::sleep(Duration::from_millis(100));
    let observed = calls.load(Ordering::SeqCst);
    thread::sleep(Duration::from_millis(100));
    let stable = calls.load(Ordering::SeqCst);
    server.join().expect("join stalled mock");
    assert!(exited, "Nu did not stop promptly after SIGINT");
    assert_eq!(observed, stable, "requests continued after interruption");
    assert!(
        stable <= 4,
        "more requests than the configured jobs limit: {stable}"
    );
}

/// Resolves and reloads caller NUON, ignores legacy TOML, and converts it without key disclosure.
#[test]
fn nuon_defaults_follow_caller_directory_and_reload_between_calls() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let root = std::env::temp_dir().join(format!("jev-real-config-{}", std::process::id()));
    let first = root.join("first");
    let second = root.join("second");
    let broken = root.join("broken");
    let user = root.join("user");
    let legacy_user = root.join("legacy_user");
    std::fs::create_dir_all(&first).expect("first caller directory");
    std::fs::create_dir_all(&second).expect("second caller directory");
    std::fs::create_dir_all(&broken).expect("broken caller directory");
    std::fs::create_dir_all(user.join("nu_plugin_jev")).expect("user config directory");
    std::fs::create_dir_all(legacy_user.join("nu_plugin_jev")).expect("legacy user directory");
    std::fs::write(first.join(".nu_plugin_jev.nuon"), "{model: 'first-model'}")
        .expect("first local NUON");
    std::fs::write(
        user.join("nu_plugin_jev/config.nuon"),
        "{model: 'user-model'}",
    )
    .expect("user NUON");
    std::fs::write(second.join(".jev.toml"), "model = 'legacy-local'\n")
        .expect("legacy local TOML");
    std::fs::write(
        second.join(".nu_plugin_jev.toml"),
        "model = 'legacy-local'\napi_key = 'fixture-migration-key'\n[cache]\nmax_entries = 7",
    )
    .expect("former plugin-scoped local TOML");
    std::fs::write(
        legacy_user.join("nu_plugin_jev/config.toml"),
        "model = 'legacy-user'\n",
    )
    .expect("legacy user TOML");
    std::fs::write(root.join("explicit.data"), "{model: 'explicit-model'}")
        .expect("explicit NUON with nonstandard extension");
    std::fs::write(root.join(".nu_plugin_jev.nuon"), "{model: 'parent-model'}")
        .expect("parent NUON");
    std::fs::write(
        broken.join(".nu_plugin_jev.nuon"),
        "{api_key: 'secret' invalid NUON}",
    )
    .expect("broken NUON");
    std::fs::write(
        broken.join(".nu_plugin_jev.toml"),
        "api_key = 'secret' invalid TOML",
    )
    .expect("broken legacy TOML");
    let source = indoc! {r#"
        let q = {match: (jev question noul 'Match?')};
        cd '{FIRST}';
        let a = ('hello' | jev ask $q --dry-run | get request.model);
        let pending = ([{message: 1}] | jev annotate $q --dry-run);
        {model: 'updated-model'} | save --force .nu_plugin_jev.nuon;
        let frozen = ($pending | get 0.request.model);
        let b = ('hello' | jev ask $q --dry-run | get request.model);
        cd '{SECOND}';
        let c = ('hello' | jev ask $q --dry-run | get request.model);
        {model: 'changed-user'} | save --force '{USER}/nu_plugin_jev/config.nuon';
        let changed_user = ('hello' | jev ask $q --dry-run | get request.model);
        $env.NU_PLUGIN_JEV_MODEL = 'env-model';
        let changed_env = ('hello' | jev ask $q --dry-run | get request.model);
        hide-env NU_PLUGIN_JEV_MODEL;
        $env.NU_PLUGIN_JEV_CONFIG = '{FIRST}/.nu_plugin_jev.nuon';
        let d = ('hello' | jev ask $q --dry-run | get request.model);
        let e = ('hello' | jev ask $q --config '{EXPLICIT}' --dry-run | get request.model);
        hide-env NU_PLUGIN_JEV_CONFIG;
        let parallel = (['{FIRST}' '{SECOND}'] | par-each { |dir|
            cd $dir
            'hello' | jev ask $q --dry-run | get request.model
        } | sort);
        cd '{SECOND}';
        $env.XDG_CONFIG_HOME = '{LEGACY_USER}';
        let ignored_legacy_files = ('hello' | jev ask $q --dry-run | get request.model);
        let legacy_error = (try {
            'hello' | jev ask $q --config .jev.toml --dry-run
        } catch {|err| $err.msg});
        open .nu_plugin_jev.toml | save converted.nuon;
        let converted = ('hello' | jev ask $q --config converted.nuon --dry-run
            | get request.model);
        let retained = (open converted.nuon | {key: ($in.api_key == 'fixture-migration-key'),
            cache: $in.cache.max_entries});
        cd '{BROKEN}';
        let offline = ((jev question noul 'Still offline?') | get type);
        let guidance = (jev | str contains 'jev ask');
        [
            $a $frozen $b $c $changed_user $changed_env $d $e
            $parallel $ignored_legacy_files $legacy_error $converted $retained $offline $guidance
        ] | to json --raw
    "#}
    .replace("{FIRST}", &first.to_string_lossy())
    .replace("{SECOND}", &second.to_string_lossy())
    .replace("{BROKEN}", &broken.to_string_lossy())
    .replace("{USER}", &user.to_string_lossy())
    .replace("{LEGACY_USER}", &legacy_user.to_string_lossy())
    .replace("{EXPLICIT}", &root.join("explicit.data").to_string_lossy());
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            &source,
        ])
        .env("XDG_CONFIG_HOME", &user)
        .output()
        .expect("run isolated Nu");
    std::fs::remove_dir_all(&root).expect("remove isolated fixture");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON models");
    assert_eq!(
        values,
        serde_json::json!([
            "first-model",
            "first-model",
            "updated-model",
            "user-model",
            "changed-user",
            "env-model",
            "updated-model",
            "explicit-model",
            ["changed-user", "updated-model"],
            "jev-latest",
            "malformed local NUON configuration",
            "legacy-local",
            {"key": true, "cache": 7},
            "noul",
            true
        ])
    );
}

/// Ignores former setting names in the calling Nu environment.
#[test]
fn replaced_environment_names_are_not_selected() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let root = std::env::temp_dir().join(format!("jev-old-env-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("isolated config directory");
    let source = concat!(
        "let q = {match: {type: noul}}; ",
        "let a = ('hello' | jev ask $q --dry-run | get request.model); ",
        "let b = ([{message: 'hello'}] | jev annotate $q --dry-run ",
        "| get 0.request.model); [$a $b] | to json --raw"
    );
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .current_dir(&root)
        .env("XDG_CONFIG_HOME", &root)
        .env("TYPESAFE_MODEL", "old-model")
        .env("TYPESAFE_BASE_URL", "invalid-url")
        .env("TYPESAFE_TIMEOUT_MS", "0")
        .env("TYPESAFE_JOBS", "0")
        .env("TYPESAFE_RETRIES", "-1")
        .env("JEV_PROXY", "invalid-proxy")
        .env("JEV_CONFIG", "missing-legacy.toml")
        .output()
        .expect("run isolated Nu");
    std::fs::remove_dir_all(&root).expect("remove isolated fixture");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let models: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON models");
    assert_eq!(models, serde_json::json!(["jev-latest", "jev-latest"]));
}

/// Uses a private user-file key for live HTTP and blocks an insecure key file.
#[cfg(unix)]
#[test]
fn live_nu_uses_private_nuon_key_and_rejects_open_permissions() {
    use std::os::unix::fs::PermissionsExt;

    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let root = std::env::temp_dir().join(format!("jev-real-key-{}", std::process::id()));
    let user = root.join("user");
    std::fs::create_dir_all(user.join("nu_plugin_jev")).expect("user config directory");
    let key_file = user.join("nu_plugin_jev/config.nuon");
    let old_file = user.join("nu_plugin_jev/config.toml");
    std::fs::write(&old_file, "api_key = 'fixture-secret-do-not-echo'").expect("old user key file");
    std::fs::set_permissions(&old_file, std::fs::Permissions::from_mode(0o600))
        .expect("private file permissions");
    let migration = concat!(
        "touch '{NEW}'; ^chmod 600 '{NEW}'; ",
        "open '{OLD}' | save --force '{NEW}'"
    )
    .replace("{NEW}", &key_file.to_string_lossy())
    .replace("{OLD}", &old_file.to_string_lossy());
    let converted = isolated_command("nu")
        .args(["--no-config-file", "--commands", &migration])
        .output()
        .expect("convert private TOML using documented Nu commands");
    assert!(converted.status.success());
    assert!(converted.stdout.is_empty());
    assert!(converted.stderr.is_empty());
    assert_eq!(
        std::fs::metadata(&key_file).unwrap().permissions().mode() & 0o077,
        0
    );
    let (base_url, calls, server) = serve_until_stopped(false);
    let source =
        "{message: 1} | jev ask {match: (jev question noul 'Match?')} | get answers.match.noul";
    let run = || {
        isolated_command("nu")
            .args([
                "--no-config-file",
                "--plugins",
                env!("CARGO_BIN_EXE_nu_plugin_jev"),
                "--commands",
                source,
            ])
            .current_dir(&root)
            .env("XDG_CONFIG_HOME", &user)
            .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
            .env_remove("TYPESAFE_API_KEY")
            .output()
            .expect("run isolated Nu")
    };
    let accepted = run();
    assert!(
        accepted.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o644))
        .expect("open file permissions");
    let rejected = run();
    server.join().expect("join mock server");
    std::fs::remove_dir_all(&root).expect("remove isolated fixture");
    assert!(!rejected.status.success());
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(stderr.contains("owner-only"));
    assert!(!stderr.contains("fixture-secret-do-not-echo"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// Confirms a reused plugin process keeps one live HTTP connection for compatible calls.
#[test]
fn live_nu_reuses_compatible_http_connection() {
    if isolated_command("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, server) = h2_fixture::serve(2, |_, _| {
        h2_fixture::Response::json(
            200,
            serde_json::json!({
                "model": "jev-fixed", "answers": {"match": {"type": "noul", "noul": 0.9}},
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }),
        )
    });
    let source = concat!(
        "let q = {match: (jev question noul 'Match?')}; ",
        "let a = ({message: 1} | jev ask $q | get answers.match.noul); ",
        "let b = ({message: 2} | jev ask $q | get answers.match.noul); ",
        "[$a $b] | to json --raw"
    );
    let output = isolated_command("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            source,
        ])
        .env("TYPESAFE_API_KEY", "mock-only")
        .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
        .output()
        .expect("run isolated Nu");
    let (requests, connections) = server
        .join_with_connections()
        .expect("join keep-alive mock");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(requests.len(), 2);
    assert_eq!(
        connections, 1,
        "compatible calls opened separate HTTP connections"
    );
}
