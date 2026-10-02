//! Exercises streaming annotation through the actual Nushell plugin protocol.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Responds to local System One requests until the subprocess completes.
fn serve_until_stopped(
    selective: bool,
) -> (
    String,
    Arc<AtomicBool>,
    Arc<AtomicUsize>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local mock");
    listener
        .set_nonblocking(true)
        .expect("configure local mock");
    let root = format!("http://{}", listener.local_addr().expect("local address"));
    let stop = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let server_stop = Arc::clone(&stop);
    let server_calls = Arc::clone(&calls);
    let handle = thread::spawn(move || {
        while !server_stop.load(Ordering::SeqCst) {
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("local mock accept failed: {error}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("request timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone socket"));
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
                    length = value.trim().parse().expect("body length");
                }
            }
            let mut body = vec![0; length];
            if reader.read_exact(&mut body).is_err() {
                continue;
            }
            server_calls.fetch_add(1, Ordering::SeqCst);
            let state: serde_json::Value = serde_json::from_slice(&body).expect("request body");
            let id = state["state"]["message"].as_i64().expect("selected row id");
            let probability = if !selective || id % 5 == 0 { 0.9 } else { 0.1 };
            let response = serde_json::json!({"model": "jev-fixed", "answers": {"match": {"type": "noul", "noul": probability}}, "usage": {"input_tokens": 1, "output_tokens": 1}}).to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                response.len()
            );
        }
    });
    (root, stop, calls, handle)
}

/// Runs the same native Nu pipeline against an isolated local mock service.
fn run_pipeline(selective: bool) -> Option<(Vec<serde_json::Value>, usize)> {
    if Command::new("nu").arg("--version").output().is_err() {
        return None;
    }
    let (base_url, stop, calls, server) = serve_until_stopped(selective);
    let source = if selective {
        "let q = {match: (jev question noul 'Is this a match?')}; 1..1000 | each { |id| {id: $id, message: ($id mod 5)} } | jev annotate $q --fields [message] --jobs 4 | where jev.match.noul > 0.5 | first 10 | to json --raw"
    } else {
        "let q = {match: (jev question noul 'Is this a match?')}; 1..1000 | each { |id| {id: $id, message: $id} } | jev annotate $q --fields [message] -j 4 | where jev.match.noul > 0.5 | first 10 | to json --raw"
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut child = Command::new("nu")
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
            stop.store(true, Ordering::SeqCst);
            server.join().expect("join local mock after timeout");
            panic!("Nu pipeline did not stop after first 10 rows");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().expect("read Nu output");
    stop.store(true, Ordering::SeqCst);
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
    assert!(calls <= 18, "too many requests after first 10 rows");
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
fn serve_mixed_once() -> (String, thread::JoinHandle<serde_json::Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mixed mock");
    let root = format!("http://{}", listener.local_addr().expect("local address"));
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept mixed request");
        let mut reader = BufReader::new(stream.try_clone().expect("clone socket"));
        let mut line = String::new();
        reader.read_line(&mut line).expect("request line");
        let mut length = 0;
        loop {
            line.clear();
            reader.read_line(&mut line).expect("request header");
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().expect("body length");
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).expect("request body");
        let captured = serde_json::from_slice(&body).expect("request JSON");
        let response = serde_json::json!({"model": "jev-fixed", "answers": {
            "spam": {"type": "noul", "noul": 0.982},
            "kind": {"type": "choice", "choice": "spam", "confidence": 0.91,
                "probabilities": {"normal": 0.09, "spam": 0.91}},
            "urgency": {"type": "score", "score": 1.4, "confidence": 0.81,
                "legend": {"0": "later", "1": "today", "2": "now"},
                "probabilities": {"0": 0.1, "1": 0.4, "2": 0.5}}
        }, "usage": {"input_tokens": 42, "output_tokens": 6}})
        .to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).expect("mixed response");
        captured
    });
    (root, handle)
}

/// Verifies native projections and caller thresholds over all three answer variants.
#[test]
fn ask_native_get_keeps_mixed_answer_details() {
    if Command::new("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, server) = serve_mixed_once();
    let source = "let q = {spam: {type: noul}, kind: {type: choice, criteria: {normal: null, spam: null}}, urgency: {type: score, criteria: ['later' 'today' 'now']}}; let result = ('hello' | jev ask $q); {probability: ($result | get answers.spam.noul), passes: (($result | get answers.spam.noul) > 0.98), kind: ($result | get answers.kind.choice), confidence: ($result | get answers.kind.confidence), distribution: ($result | get answers.kind.probabilities), score: ($result | get answers.urgency.score), legend: ($result | get answers.urgency.legend), model: ($result | get model)} | to json --raw";
    let output = Command::new("nu")
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
    assert_eq!(captured["state"], "hello");
    assert_eq!(captured["questions"].as_object().unwrap().len(), 3);
}

/// Keeps retry attempts under one traced evaluation without logging request content.
#[test]
fn ask_tracing_correlates_retry_attempts() {
    if Command::new("nu").arg("--version").output().is_err() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind retry mock");
    let base_url = format!("http://{}", listener.local_addr().expect("local address"));
    let server = thread::spawn(move || {
        for (attempt, status) in [(1, 503), (2, 503), (3, 200)] {
            let (mut stream, _) = listener.accept().expect("accept retry request");
            let mut reader = BufReader::new(stream.try_clone().expect("clone socket"));
            let mut line = String::new();
            reader.read_line(&mut line).expect("request line");
            let mut length = 0;
            loop {
                line.clear();
                reader.read_line(&mut line).expect("request header");
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().expect("body length");
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).expect("request body");
            if status == 503 {
                let request_id = if attempt == 1 {
                    "req_retry_1"
                } else {
                    "req_local-key"
                };
                write!(stream, "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nX-TypeSafe-Request-Id: {request_id}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").expect("retry response");
            } else {
                let response = serde_json::json!({"model": "jev-fixed", "answers": {
                    "match": {"type": "noul", "noul": 0.9}}, "usage": {
                    "input_tokens": 2, "output_tokens": 1}})
                .to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-TypeSafe-Request-Id: req_success_3\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).expect("success response");
            }
        }
    });
    let output = Command::new("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            "'hello' | jev ask {match: {type: noul}} --timeout 5sec | get answers.match.noul | to json --raw",
        ])
        .env("TYPESAFE_API_KEY", "local-key")
        .env("NU_PLUGIN_JEV_BASE_URL", &base_url)
        .env("NU_PLUGIN_JEV_RETRIES", "2")
        .env("NU_PLUGIN_JEV_LOG", "debug")
        .output()
        .expect("run isolated Nu");
    server.join().expect("join retry mock");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.trim_ascii(), b"0.9");
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "attempt=1",
        "attempt=2",
        "status=503",
        "status=200",
        "retrying Jev request",
    ] {
        assert!(
            diagnostics.contains(expected),
            "missing diagnostic: {expected}"
        );
    }
    let ids: Vec<_> = diagnostics
        .lines()
        .filter(|line| line.contains("Jev HTTP") || line.contains("retrying Jev request"))
        .filter_map(|line| line.split("request_id=\"").nth(1))
        .filter_map(|suffix| suffix.split('"').next())
        .collect();
    assert!(ids.len() >= 4);
    assert!(ids.iter().all(|id| *id == ids[0]));
    for (attempt, server_id) in [(1, "req_retry_1"), (3, "req_success_3")] {
        let line = diagnostics
            .lines()
            .find(|line| {
                line.contains("Jev HTTP response received")
                    && line.contains(&format!("attempt={attempt}"))
            })
            .expect("response diagnostic");
        assert!(line.contains(&format!("server_request_id=\"{server_id}\"")));
        assert!(line.contains(&format!("request_id=\"{}\"", ids[0])));
    }
    let invalid_line = diagnostics
        .lines()
        .find(|line| line.contains("Jev HTTP response received") && line.contains("attempt=2"))
        .expect("invalid-ID response diagnostic");
    assert!(!invalid_line.contains("server_request_id"));
    for secret in ["local-key", "hello", "Authorization", "req_local-key"] {
        assert!(!diagnostics.contains(secret));
    }
}

/// Responds to three distinct projected states with typed, state-dependent decisions.
fn serve_mixed_table() -> (String, thread::JoinHandle<Vec<serde_json::Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind table mock");
    let root = format!("http://{}", listener.local_addr().expect("local address"));
    let handle = thread::spawn(move || {
        (0..3).map(|_| {
            let (mut stream, _) = listener.accept().expect("accept table request");
            let mut reader = BufReader::new(stream.try_clone().expect("clone socket"));
            let mut line = String::new();
            reader.read_line(&mut line).expect("request line");
            let mut length = 0;
            loop {
                line.clear();
                reader.read_line(&mut line).expect("request header");
                if line == "\r\n" { break; }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().expect("body length");
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).expect("request body");
            let captured: serde_json::Value = serde_json::from_slice(&body).expect("request JSON");
            let message = captured["state"]["input"]["message"].as_str().expect("projected message");
            let (probability, choice, score, levels) = match message {
                "urgent" => (0.99, "spam", 1.8, [0.0, 0.2, 0.8]),
                "normal" => (0.1, "normal", 0.1, [0.9, 0.1, 0.0]),
                "later" => (0.95, "spam", 1.4, [0.1, 0.4, 0.5]),
                _ => panic!("unexpected mock state"),
            };
            let (normal, spam) = if choice == "spam" { (0.05, 0.95) } else { (0.95, 0.05) };
            let response = serde_json::json!({"model": "jev-fixed", "answers": {
                "spam": {"type": "noul", "noul": probability},
                "kind": {"type": "choice", "choice": choice, "confidence": 0.95,
                    "probabilities": {"normal": normal, "spam": spam}},
                "urgency": {"type": "score", "score": score, "confidence": 0.8,
                    "legend": {"0": "later", "1": "today", "2": "now"},
                    "probabilities": {"0": levels[0], "1": levels[1], "2": levels[2]}}
            }, "usage": {"input_tokens": 10, "output_tokens": 3}}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).expect("table response");
            captured
        }).collect()
    });
    (root, handle)
}

/// Verifies native table transforms over typed answers and projected outbound state.
#[test]
fn annotate_composes_with_native_table_commands() {
    if Command::new("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, server) = serve_mixed_table();
    let source = r#"
let q = {
    spam: {type: noul}
    kind: {type: choice, criteria: {normal: null, spam: null}}
    urgency: {type: score, criteria: ['later' 'today' 'now']}
}
let rows = ([{id: 1, message: 'urgent', sender: 'a', secret: 'local-1'} {id: 2, message: 'normal', sender: 'b', secret: 'local-2'} {id: 3, message: 'later', sender: 'c', secret: 'local-3'}]
    | jev annotate $q --fields [message sender] --context {policy: 'rules'} --into ai --meta ai_meta --jobs 2
    | select id message sender secret ai ai_meta)
{all: $rows, filtered: ($rows | where ai.spam.noul > 0.9 | sort-by ai.urgency.score --reverse | reject ai_meta)} | to json --raw
"#;
    let output = Command::new("nu")
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
    assert!(diagnostics.contains(wire["all"][0]["ai_meta"]["request_id"].as_str().unwrap()));
    for secret in ["local-key", "local-1", "urgent"] {
        assert!(!diagnostics.contains(secret));
    }
    assert_eq!(wire["all"].as_array().unwrap().len(), 3);
    assert_eq!(wire["all"][0]["secret"], "local-1");
    assert_eq!(wire["all"][0]["ai_meta"]["model"], "jev-fixed");
    assert_eq!(wire["all"][0]["ai_meta"]["usage"]["input_tokens"], 10);
    assert_eq!(wire["filtered"].as_array().unwrap().len(), 2);
    assert_eq!(wire["filtered"][0]["id"], 1);
    assert_eq!(wire["filtered"][1]["id"], 3);
    assert_eq!(wire["filtered"][0]["ai"]["kind"]["choice"], "spam");
    assert!(wire["filtered"][0].get("ai_meta").is_none());
    let captured = server.join().expect("join table mock");
    assert_eq!(captured.len(), 3);
    for request in captured {
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

/// Confirms cache reuse logs correlate with row metadata in a real Nu process.
#[test]
fn annotate_tracing_correlates_cached_rows() {
    if Command::new("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, stop, calls, server) = serve_until_stopped(false);
    let source = "[{id: 1, message: 7} {id: 2, message: 7} {id: 3, message: 7}] | jev annotate {match: {type: noul}} --fields [message] --jobs 1 --meta ai_meta | select id ai_meta | to json --raw";
    let output = Command::new("nu")
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
    stop.store(true, Ordering::SeqCst);
    server.join().expect("join local mock");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Nu JSON table");
    let rows = rows.as_array().expect("annotated rows");
    assert_eq!(rows.len(), 3);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let request_id = rows[0]["ai_meta"]["request_id"]
        .as_str()
        .expect("request identity");
    assert!(
        rows.iter()
            .all(|row| row["ai_meta"]["request_id"] == request_id)
    );
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostics.contains("completed Jev result cache hit"));
    assert!(diagnostics.contains(request_id));
    assert!(!diagnostics.contains("local-key"));
}

/// Applies a changed logging level only after the plugin process restarts.
#[test]
fn tracing_level_changes_after_plugin_restart() {
    if Command::new("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, stop, calls, server) = serve_until_stopped(false);
    let source = r#"
let q = {match: {type: noul}}
let first = ({message: 1} | jev ask $q | get answers.match.noul)
$env.NU_PLUGIN_JEV_LOG = "info"
let second = ({message: 2} | jev ask $q | get answers.match.noul)
plugin stop jev
let third = ({message: 3} | jev ask $q | get answers.match.noul)
[$first $second $third] | to json --raw
"#;
    let output = Command::new("nu")
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
    stop.store(true, Ordering::SeqCst);
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

/// Keeps the installed Nu namespace limited to the six declared commands.
#[test]
fn real_nu_registers_only_the_six_core_commands() {
    if Command::new("nu").arg("--version").output().is_err() {
        return;
    }
    let output = Command::new("nu")
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
            "jev question choice",
            "jev question noul",
            "jev question score"
        ])
    );
}

/// Shows only the selected short aliases in real Nushell command help.
#[test]
fn real_nu_help_lists_focused_short_options() {
    if Command::new("nu").arg("--version").output().is_err() {
        return;
    }
    let output = Command::new("nu")
        .args([
            "--no-config-file",
            "--plugins",
            env!("CARGO_BIN_EXE_nu_plugin_jev"),
            "--commands",
            "{ask: (help jev ask), annotate: (help jev annotate)} | to json --raw",
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
        assert!(help.contains(".nu_plugin_jev.toml"));
        assert!(!help.contains("-b, --base-url"));
        assert!(!help.contains("-t, --timeout"));
    }
}

/// Accepts and retains requests without replying until the test stops the server.
fn serve_stalled() -> (
    String,
    Arc<AtomicBool>,
    Arc<AtomicUsize>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalled mock");
    listener
        .set_nonblocking(true)
        .expect("configure stalled mock");
    let root = format!("http://{}", listener.local_addr().expect("local address"));
    let stop = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let server_stop = Arc::clone(&stop);
    let server_calls = Arc::clone(&calls);
    let handle = thread::spawn(move || {
        let mut pending = Vec::new();
        while !server_stop.load(Ordering::SeqCst) {
            let (stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("stalled mock accept failed: {error}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("request timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone socket"));
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
                    length = value.trim().parse().expect("body length");
                }
            }
            let mut body = vec![0; length];
            if reader.read_exact(&mut body).is_err() {
                continue;
            }
            server_calls.fetch_add(1, Ordering::SeqCst);
            pending.push(stream);
        }
        drop(pending);
    });
    (root, stop, calls, handle)
}

/// Interrupts a stalled real Nu stream and confirms bounded, prompt teardown.
#[cfg(unix)]
#[test]
fn interrupt_stalled_annotation_stops_local_work() {
    if Command::new("nu").arg("--version").output().is_err() {
        return;
    }
    let (base_url, stop, calls, server) = serve_stalled();
    let source = "let q = {match: (jev question noul 'Match?')}; 1..1000 | each { |id| {id: $id, message: $id} } | jev annotate $q --fields [message] --jobs 4 | first 10 | to json --raw";
    let mut child = Command::new("nu")
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
    stop.store(true, Ordering::SeqCst);
    server.join().expect("join stalled mock");
    assert!(exited, "Nu did not stop promptly after SIGINT");
    assert_eq!(observed, stable, "requests continued after interruption");
    assert!(
        stable <= 4,
        "more requests than the configured jobs limit: {stable}"
    );
}

/// Resolves TOML from each caller directory while reusing one Nu plugin session.
#[test]
fn toml_defaults_follow_caller_directory_and_reload_between_calls() {
    if Command::new("nu").arg("--version").output().is_err() {
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
    std::fs::create_dir_all(legacy_user.join("jev")).expect("legacy user directory");
    std::fs::write(first.join(".nu_plugin_jev.toml"), "model = 'first-model'\n")
        .expect("first local TOML");
    std::fs::write(
        user.join("nu_plugin_jev/config.toml"),
        "model = 'user-model'\n",
    )
    .expect("user TOML");
    std::fs::write(second.join(".jev.toml"), "model = 'legacy-local'\n")
        .expect("legacy local TOML");
    std::fs::write(
        legacy_user.join("jev/config.toml"),
        "model = 'legacy-user'\n",
    )
    .expect("legacy user TOML");
    std::fs::write(root.join("explicit.toml"), "model = 'explicit-model'\n")
        .expect("explicit TOML");
    std::fs::write(root.join(".nu_plugin_jev.toml"), "model = 'parent-model'\n")
        .expect("parent TOML");
    std::fs::write(
        broken.join(".nu_plugin_jev.toml"),
        "api_key = 'secret' invalid TOML",
    )
    .expect("broken TOML");
    let source = r#"
let q = {match: (jev question noul 'Match?')};
cd '{FIRST}';
let a = ('hello' | jev ask $q --dry-run | get model);
let pending = ([{message: 1}] | jev annotate $q --dry-run);
"model = 'updated-model'" | save --force .nu_plugin_jev.toml;
let frozen = ($pending | get 0.model);
let b = ('hello' | jev ask $q --dry-run | get model);
cd '{SECOND}';
let c = ('hello' | jev ask $q --dry-run | get model);
"model = 'changed-user'" | save --force '{USER}/nu_plugin_jev/config.toml';
let changed_user = ('hello' | jev ask $q --dry-run | get model);
$env.NU_PLUGIN_JEV_MODEL = 'env-model';
let changed_env = ('hello' | jev ask $q --dry-run | get model);
hide-env NU_PLUGIN_JEV_MODEL;
$env.NU_PLUGIN_JEV_CONFIG = '{FIRST}/.nu_plugin_jev.toml';
let d = ('hello' | jev ask $q --dry-run | get model);
let e = ('hello' | jev ask $q --config '{EXPLICIT}' --dry-run | get model);
hide-env NU_PLUGIN_JEV_CONFIG;
let parallel = (['{FIRST}' '{SECOND}'] | par-each { |dir| cd $dir; 'hello' | jev ask $q --dry-run | get model } | sort);
cd '{SECOND}';
$env.XDG_CONFIG_HOME = '{LEGACY_USER}';
let ignored_legacy_files = ('hello' | jev ask $q --dry-run | get model);
let explicit_legacy = ('hello' | jev ask $q --config .jev.toml --dry-run | get model);
cd '{BROKEN}';
let offline = ((jev question noul 'Still offline?') | get type);
let guidance = (jev | str contains 'jev ask');
[$a $frozen $b $c $changed_user $changed_env $d $e $parallel $ignored_legacy_files $explicit_legacy $offline $guidance] | to json --raw
"#
    .replace("{FIRST}", &first.to_string_lossy())
    .replace("{SECOND}", &second.to_string_lossy())
    .replace("{BROKEN}", &broken.to_string_lossy())
    .replace("{USER}", &user.to_string_lossy())
    .replace("{LEGACY_USER}", &legacy_user.to_string_lossy())
    .replace("{EXPLICIT}", &root.join("explicit.toml").to_string_lossy());
    let output = Command::new("nu")
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
            "legacy-local",
            "noul",
            true
        ])
    );
}

/// Ignores former setting names in the calling Nu environment.
#[test]
fn replaced_environment_names_are_not_selected() {
    if Command::new("nu").arg("--version").output().is_err() {
        return;
    }
    let root = std::env::temp_dir().join(format!("jev-old-env-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("isolated config directory");
    let source = "let q = {match: {type: noul}}; let a = ('hello' | jev ask $q --dry-run | get model); let b = ([{message: 'hello'}] | jev annotate $q --dry-run | get 0.model); [$a $b] | to json --raw";
    let output = Command::new("nu")
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
fn live_nu_uses_private_toml_key_and_rejects_open_permissions() {
    use std::os::unix::fs::PermissionsExt;

    if Command::new("nu").arg("--version").output().is_err() {
        return;
    }
    let root = std::env::temp_dir().join(format!("jev-real-key-{}", std::process::id()));
    let user = root.join("user");
    std::fs::create_dir_all(user.join("nu_plugin_jev")).expect("user config directory");
    let key_file = user.join("nu_plugin_jev/config.toml");
    std::fs::write(&key_file, "api_key = 'fixture-secret-do-not-echo'\n").expect("user key file");
    std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600))
        .expect("private file permissions");
    let (base_url, stop, calls, server) = serve_until_stopped(false);
    let source =
        "{message: 1} | jev ask {match: (jev question noul 'Match?')} | get answers.match.noul";
    let run = || {
        Command::new("nu")
            .args([
                "--no-config-file",
                "--plugins",
                env!("CARGO_BIN_EXE_nu_plugin_jev"),
                "--commands",
                source,
            ])
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
    stop.store(true, Ordering::SeqCst);
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
    if Command::new("nu").arg("--version").output().is_err() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local keep-alive mock");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local address"));
    let connections = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&connections);
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut requests = 0;
        while requests < 2 && Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("keep-alive mock accept failed: {error}"),
            };
            observed.fetch_add(1, Ordering::SeqCst);
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("read timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone socket"));
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
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
                        length = value.trim().parse().expect("body length");
                    }
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    break;
                }
                let response = "{\"model\":\"jev-fixed\",\"answers\":{\"match\":{\"type\":\"noul\",\"noul\":0.9}},\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}";
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{response}", response.len()).expect("write response");
                stream.flush().expect("flush response");
                requests += 1;
                if requests == 2 {
                    break;
                }
            }
        }
        requests
    });
    let source = "let q = {match: (jev question noul 'Match?')}; let a = ({message: 1} | jev ask $q | get answers.match.noul); let b = ({message: 2} | jev ask $q | get answers.match.noul); [$a $b] | to json --raw";
    let output = Command::new("nu")
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
    let requests = server.join().expect("join keep-alive mock");
    assert!(
        output.status.success(),
        "Nu failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(requests, 2);
    assert_eq!(
        connections.load(Ordering::SeqCst),
        1,
        "compatible calls opened separate HTTP connections"
    );
}
