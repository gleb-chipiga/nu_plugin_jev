//! Exercises concurrent commands in one real Nushell and one plugin process.

use std::{
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use indoc::indoc;
use serde_json::{Value, json};

use super::h2_fixture::{self, CapturedRequest, Response, ResponseGate};

/// Owns an isolated Nu child and terminates it if a test exits before collection.
struct NuProcess {
    child: Option<Child>,
    config_dir: PathBuf,
}

impl NuProcess {
    /// Starts one Nu with a synthetic key, deterministic transport, and a startup limit.
    fn start(source: &str, limit: usize, environment: &[(&str, &str)]) -> Self {
        Self::with_files(source, Some(limit), environment, &[])
    }

    /// Adds synthetic startup files without reading real user credentials or mutating globals.
    /// Child-only environment overrides avoid races between parallel tests.
    fn with_files(
        source: &str,
        limit: Option<usize>,
        environment: &[(&str, &str)],
        files: &[(&str, &str)],
    ) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let config_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "nu-command-concurrency-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir(&config_dir).expect("create isolated Nu configuration directory");
        for &(name, contents) in files {
            let path = config_dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        let mut command = Command::new("nu");
        command
            .args([
                "--no-config-file",
                "--plugins",
                env!("CARGO_BIN_EXE_nu_plugin_jev"),
                "--commands",
                source,
            ])
            .current_dir(&config_dir)
            .env("PWD", &config_dir)
            .env("XDG_CONFIG_HOME", &config_dir)
            .env_remove("NU_PLUGIN_JEV_CONFIG")
            .env("TYPESAFE_API_KEY", "local-key")
            .env_remove("NU_PLUGIN_JEV_MAX_IN_FLIGHT")
            .env("NU_PLUGIN_JEV_PROXY", "direct")
            .env("NU_PLUGIN_JEV_TIMEOUT_MS", "5000")
            .env("NU_PLUGIN_JEV_RETRIES", "0")
            .env("NU_PLUGIN_JEV_JOBS", "16")
            .env("NU_PLUGIN_JEV_MODEL", "jev-test")
            .env("NU_PLUGIN_JEV_LOG", "off")
            .env_remove("NU_PLUGIN_JEV_LOG_FORMAT")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(limit) = limit {
            command.env("NU_PLUGIN_JEV_MAX_IN_FLIGHT", limit.to_string());
        }
        for &(name, value) in environment {
            command.env(name, value);
        }
        Self {
            child: Some(command.spawn().expect("start Nu concurrency test")),
            config_dir,
        }
    }

    /// Collects finite output with a watchdog so serialized or stalled calls fail clearly.
    fn output(mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(15);
        let child = self.child.as_mut().unwrap();
        while child.try_wait().expect("poll Nu child").is_none() {
            assert!(Instant::now() < deadline, "concurrent Nu commands stalled");
            thread::sleep(Duration::from_millis(10));
        }
        self.child
            .take()
            .unwrap()
            .wait_with_output()
            .expect("collect Nu output")
    }

    /// Parses a successful finite pipeline's JSON representation of native Nu results.
    fn json(self) -> Value {
        let output = self.output();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("Nu result JSON")
    }
}

impl Drop for NuProcess {
    /// Avoids orphaning a Nu process when a gate assertion or watchdog fails.
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if thread::panicking() && child.try_wait().is_ok_and(|status| status.is_some()) {
                let output = child.wait_with_output().expect("collect failed Nu child");
                eprintln!("Nu exited: {}", String::from_utf8_lossy(&output.stderr));
            } else {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        let _ = std::fs::remove_dir_all(&self.config_dir);
    }
}

/// Returns a catalog or the Noul answers required by these concurrency scenarios.
fn reply(request: &CapturedRequest) -> Response {
    if request.path.ends_with("/models") {
        return Response::json(
            200,
            json!({
                "models": [
                    {
                        "name": "jev-test",
                        "description": "Fixture",
                        "release_date": "2026-09-15",
                    }
                ]
            }),
        );
    }
    let wire: Value = serde_json::from_slice(&request.body).unwrap();
    let answers = wire["questions"]
        .as_object()
        .unwrap()
        .keys()
        .map(|name| {
            (
                name.clone(),
                json!({
                    "type": "noul",
                    "noul": 0.75
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    Response::json(
        200,
        json!({
            "model": wire["model"],
            "answers": answers,
            "usage": {
                "input_tokens": 10,
                "output_tokens": 1
            },
        }),
    )
}

/// The server cannot answer either annotation until both independent calls have arrived.
/// One Nu child means one plugin instance, unlike two subprocesses with separate budgets.
#[test]
fn parallel_single_row_annotations_really_overlap() {
    let gate = ResponseGate::default();
    let (root, server) = h2_fixture::serve(2, move |_, request| {
        gate.rendezvous(2);
        reply(request)
    });
    // collect inside each par-each branch drives its lazy annotation to completion.
    // Returning unconsumed streams would not prove that the commands execute concurrently.
    let source = indoc! {r#"
        let q = {q: (jev question noul 'Question?')}

        [0 1]
        | par-each --threads 2 { |id|
            [{id: $id}]
            | jev annotate $q --jobs 1 --metrics
            | collect
        }
        | flatten
        | sort-by id
        | to json --raw
    "#};
    let rows = NuProcess::start(source, 2, &[("NU_PLUGIN_JEV_BASE_URL", &root)]).json();
    assert_eq!(rows.as_array().unwrap().len(), 2);
    for id in 0..2 {
        assert_eq!(rows[id]["id"], id);
        assert_eq!(rows[id]["answers"]["q"]["noul"], 0.75);
        assert_eq!(rows[id]["jev_metrics"]["attempts"], 1);
    }
    let captured = server.join().unwrap();
    assert_ne!(captured[0].body, captured[1].body);
}

/// Same-process single-state commands use the shared runtime concurrently.
#[test]
fn parallel_ask_calls_really_overlap() {
    let gate = ResponseGate::default();
    let (root, server) = h2_fixture::serve(2, move |_, request| {
        gate.rendezvous(2);
        reply(request)
    });
    let source = indoc! {r#"
        let q = {q: (jev question noul 'Question?')}

        [0 1]
        | par-each --threads 2 { |id|
            {id: $id}
            | jev ask $q --metrics
        }
        | to json --raw
    "#};
    let results = NuProcess::start(source, 2, &[("NU_PLUGIN_JEV_BASE_URL", &root)]).json();
    assert_eq!(results.as_array().unwrap().len(), 2);
    assert!(
        results
            .as_array()
            .unwrap()
            .iter()
            .all(|result| result["metrics"]["attempts"] == 1)
    );
    server.join().unwrap();
}

/// Ask, annotate, and models overlap even though their Nu result shapes differ.
#[test]
fn parallel_command_families_really_overlap() {
    let gate = ResponseGate::default();
    let (root, server) = h2_fixture::serve(3, move |_, request| {
        gate.rendezvous(3);
        reply(request)
    });
    let source = indoc! {r#"
        let q = {q: (jev question noul 'Question?')}

        [ask annotate models]
        | par-each --threads 3 { |command|
            ignore
            let result = (
                match $command {
                    ask => (
                        {id: 0}
                        | jev ask $q --metrics
                    )
                    annotate => (
                        [{id: 1}]
                        | jev annotate $q --jobs 1 --metrics
                        | collect
                    )
                    models => (jev models --metrics)
                }
            )
            {command: $command, result: $result}
        }
        | to json --raw
    "#};
    let results = NuProcess::start(source, 3, &[("NU_PLUGIN_JEV_BASE_URL", &root)]).json();
    assert_eq!(results.as_array().unwrap().len(), 3);
    let captured = server.join().unwrap();
    assert_eq!(
        captured
            .iter()
            .filter(|request| request.method == "GET")
            .count(),
        1
    );
    assert_eq!(
        captured
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        2
    );
}

/// Startup capacity remains shared across dynamic callers, all command families, and clients.
#[test]
fn aggregate_native_budget_and_caller_settings_are_isolated() {
    let gates: Vec<_> = (0..4).map(|_| ResponseGate::default()).collect();
    let sequence = Arc::new(AtomicUsize::new(0));
    let make_handler = || {
        let gates = gates.clone();
        let sequence = Arc::clone(&sequence);
        move |_: usize, request: &CapturedRequest| {
            let index = sequence.fetch_add(1, Ordering::SeqCst);
            let mut response = reply(request);
            response.body_gate = Some(gates[index / 2].clone());
            response
        }
    };
    let (first_root, first_server) = h2_fixture::serve_unbounded(make_handler());
    let (second_root, second_server) = h2_fixture::serve_unbounded(make_handler());
    let source = indoc! {r#"
        jev | ignore
        $env.NU_PLUGIN_JEV_MAX_IN_FLIGHT = '9'
        $env.config.plugins.jev = {jobs: 1}

        0..7
        | par-each --threads 8 { |id|
            ignore
            let root = if ($id mod 2) == 0 {
                $env.JEV_TEST_FIRST_ROOT
            } else {
                $env.JEV_TEST_SECOND_ROOT
            }
            let proxy = match ($id mod 3) {
                0 => 'auto'
                1 => 'direct'
                _ => $env.JEV_TEST_FIRST_ROOT
            }
            with-env {
                TYPESAFE_API_KEY: $'key-($id)'
                NU_PLUGIN_JEV_BASE_URL: $root
                NU_PLUGIN_JEV_PROXY: $proxy
                NU_PLUGIN_JEV_MODEL: $'model-($id)'
                NU_PLUGIN_JEV_MAX_IN_FLIGHT: $'($id + 16)'
            } {
                let q = {q: {type: noul, instructions: $'question-($id)'}}
                let command = match ($id mod 3) {
                    0 => 'ask'
                    1 => 'annotate'
                    _ => 'models'
                }
                let result = (
                    match $command {
                        ask => (
                            {id: $id}
                            | jev ask $q --context {scope: $id} --metrics
                        )
                        annotate => (
                            [{id: $id}]
                            | jev annotate $q --jobs 1 --context {scope: $id} --metrics
                            | collect
                        )
                        models => (jev models --metrics)
                    }
                )
                {id: $id, command: $command, result: $result}
            }
        }
        | sort-by id
        | to json --raw
    "#};
    let child = NuProcess::start(
        source,
        2,
        &[
            ("JEV_TEST_FIRST_ROOT", &first_root),
            ("JEV_TEST_SECOND_ROOT", &second_root),
        ],
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for (index, gate) in gates.iter().enumerate() {
            gate.wait_for_arrivals(2).await;
            tokio::time::sleep(Duration::from_millis(30)).await;
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
    });
    let results = child.json();
    assert_eq!(results.as_array().unwrap().len(), 8);
    for id in 0..8 {
        let result = &results[id]["result"];
        let (meta, measurement) = if id % 3 == 1 {
            (&result[0]["jev_meta"], &result[0]["jev_metrics"])
        } else {
            (&result["meta"], &result["metrics"])
        };
        let root = if id % 2 == 0 {
            &first_root
        } else {
            &second_root
        };
        assert_eq!(meta["base_url"], format!("{root}/"));
        assert_eq!(measurement["attempts"], 1);
        if id % 3 != 2 {
            assert_eq!(meta["model"], format!("model-{id}"));
        }
    }
    let captured = [first_server.join().unwrap(), second_server.join().unwrap()].concat();
    assert_eq!(captured.len(), 8);
    for request in captured {
        let id: usize = request
            .authorization
            .as_deref()
            .unwrap()
            .strip_prefix("Bearer key-")
            .unwrap()
            .parse()
            .unwrap();
        if request.method == "GET" {
            assert!(request.body.is_empty());
            assert_eq!(id % 3, 2);
        } else {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["model"], format!("model-{id}"));
            assert_eq!(
                body["state"],
                json!({
                    "input": {"id": id},
                    "context": {"scope": id}
                })
            );
            assert_eq!(
                body["questions"]["q"]["instructions"],
                format!("question-{id}")
            );
        }
    }
}

/// Local, user, and explicitly selected startup files establish an immutable shared limit.
#[test]
fn startup_nuon_budget_ignores_later_file_and_caller_changes() {
    for (local, user, selector) in [
        ("{max_in_flight: 2}", "{max_in_flight: 4}", None),
        ("{jobs: 8}", "{max_in_flight: 2}", None),
        (
            "{max_in_flight: 9}",
            "{max_in_flight: 4}",
            Some("startup.nuon"),
        ),
    ] {
        let gates: Vec<_> = (0..3).map(|_| ResponseGate::default()).collect();
        let held = gates.clone();
        let (root, server) = h2_fixture::serve(6, move |index, request| {
            let mut response = reply(request);
            response.body_gate = Some(held[index / 2].clone());
            response
        });
        let source = indoc! {r#"
            jev | ignore
            for path in [.nu_plugin_jev.nuon startup.nuon nu_plugin_jev/config.nuon] {
                {max_in_flight: 9, model: edited}
                | to nuon
                | save --raw --force $path
            }
            cd calls
            $env.NU_PLUGIN_JEV_MAX_IN_FLIGHT = '9'
            $env.NU_PLUGIN_JEV_CONFIG = 'call.nuon'
            $env.config.plugins.jev = {max_in_flight: 1}
            let q = {q: {type: noul}}

            0..5
            | par-each --threads 6 { |id|
                if ($id mod 2) == 0 {
                    {id: $id}
                    | jev ask $q --config call.nuon --metrics
                } else {
                    {id: $id}
                    | jev ask $q --metrics
                }
            }
            | to json --raw
        "#};
        let mut environment = vec![("NU_PLUGIN_JEV_BASE_URL", root.as_str())];
        if let Some(selector) = selector {
            environment.push(("NU_PLUGIN_JEV_CONFIG", selector));
        }
        let child = NuProcess::with_files(
            source,
            None,
            &environment,
            &[
                (".nu_plugin_jev.nuon", local),
                ("nu_plugin_jev/config.nuon", user),
                ("startup.nuon", "{max_in_flight: 2}"),
                ("calls/.nu_plugin_jev.nuon", "{max_in_flight: 77}"),
                (
                    "calls/call.nuon",
                    "{max_in_flight: 'ignored', model: caller}",
                ),
            ],
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for (index, gate) in gates.iter().enumerate() {
                gate.wait_for_arrivals(2).await;
                tokio::time::sleep(Duration::from_millis(30)).await;
                assert_eq!(server.request_count(), 2 * (index + 1));
                assert!(
                    gates
                        .iter()
                        .skip(index + 1)
                        .all(|later| later.arrivals() == 0)
                );
                gate.release();
            }
        });
        let results = child.json();
        assert_eq!(results.as_array().unwrap().len(), 6);
        assert!(
            results
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["metrics"]["attempts"] == 1)
        );
        server.join().unwrap();
    }
}

/// Startup failures remain redacted, while an environment override masks lower file errors.
#[test]
fn startup_nuon_selection_validates_before_commands() {
    let files = [
        (".nu_plugin_jev.nuon", "{max_in_flight: 'PRIVATE_VALUE'}"),
        ("nu_plugin_jev/config.nuon", "{max_in_flight: 2}"),
    ];
    let failed = NuProcess::with_files("jev", None, &[], &files).output();
    assert!(!failed.status.success());
    let diagnostic = String::from_utf8_lossy(&failed.stderr);
    assert!(diagnostic.contains("local NUON max_in_flight"));
    assert!(!diagnostic.contains("PRIVATE_VALUE"));
    let malformed = [(".nu_plugin_jev.nuon", "{PRIVATE_VALUE trailing")];
    let passed = NuProcess::with_files(
        indoc! {r#"
            jev
            | to json --raw
        "#},
        Some(2),
        &[],
        &malformed,
    )
    .json();
    assert!(passed.as_str().unwrap().contains("jev ask"));
    let failed = NuProcess::with_files(
        "jev",
        None,
        &[("NU_PLUGIN_JEV_CONFIG", "missing.nuon")],
        &[],
    )
    .output();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("cannot read local NUON"));
}

/// Offline commands ignore later malformed files after successful startup validation.
#[test]
fn offline_commands_ignore_file_edits_after_startup() {
    let source = indoc! {r#"
        jev | ignore
        '{PRIVATE_VALUE trailing'
        | save --raw --force .nu_plugin_jev.nuon
        {
            guidance: (jev)
            noul: (jev question noul 'Q')
            choice: (jev question choice 'Q' [a b])
            score: (jev question score 'Q' [low high])
        }
        | to json --raw
    "#};
    let result = NuProcess::with_files(
        source,
        None,
        &[],
        &[(".nu_plugin_jev.nuon", "{max_in_flight: 2}")],
    )
    .json();
    assert!(result["guidance"].as_str().unwrap().contains("jev ask"));
    assert_eq!(result["noul"]["type"], "noul");
    assert_eq!(result["choice"]["type"], "choice");
    assert_eq!(result["score"]["type"], "score");
}

/// Errors caught inside one Nu branch do not turn local failure into a plugin-wide interrupt.
#[test]
fn caught_native_failures_do_not_cancel_neighbors() {
    for timeout in [false, true] {
        let arrivals = ResponseGate::default();
        let body_gate = ResponseGate::default();
        let gate = body_gate.clone();
        let (root, server) = h2_fixture::serve(2, move |_, captured| {
            arrivals.rendezvous(2);
            let wire: Value = serde_json::from_slice(&captured.body).unwrap();
            if wire["state"]["id"] == 0 {
                if timeout {
                    let mut response = reply(captured);
                    response.body_gate = Some(gate.clone());
                    response
                } else {
                    Response::json(401, json!({}))
                }
            } else {
                reply(captured)
            }
        });
        // Catch the expected failure inside its branch. Catching only the outer pipeline
        // could let Nu's global interrupt hide whether plugin-local cancellation is isolated.
        let source = indoc! {r#"
            let q = {q: {type: noul}}

            [0 1]
            | par-each --threads 2 { |id|
                try {
                    let result = (
                        {id: $id}
                        | jev ask $q --timeout 1sec
                    )
                    {
                        id: $id
                        success: true
                        probability: $result.answers.q.noul
                    }
                } catch {
                    {id: $id, success: false}
                }
            }
            | sort-by id
            | to json --raw
        "#};
        let results = NuProcess::start(source, 2, &[("NU_PLUGIN_JEV_BASE_URL", &root)]).json();
        assert_eq!(results[0]["success"], false);
        assert_eq!(results[1]["success"], true);
        assert_eq!(results[1]["probability"], 0.75);
        body_gate.release();
        server.join().unwrap();
    }
}

/// Identical concurrent annotations keep deduplication inside each invocation only.
#[test]
fn concurrent_native_annotations_do_not_share_caches() {
    let gate = ResponseGate::default();
    let (root, server) = h2_fixture::serve(2, move |_, captured| {
        gate.rendezvous(2);
        reply(captured)
    });
    let source = indoc! {r#"
        let q = {q: {type: noul}}

        [0 1]
        | par-each --threads 2 {
            [
                {message: same}
                {message: same}
            ]
            | jev annotate $q --jobs 2 --metrics
            | collect
        }
        | to json --raw
    "#};
    let groups = NuProcess::start(source, 2, &[("NU_PLUGIN_JEV_BASE_URL", &root)]).json();
    assert_eq!(groups.as_array().unwrap().len(), 2);
    for group in groups.as_array().unwrap() {
        assert_eq!(group.as_array().unwrap().len(), 2);
        assert_eq!(
            group[0]["jev_metrics"]["request_id"],
            group[1]["jev_metrics"]["request_id"]
        );
    }
    assert_ne!(
        groups[0][0]["jev_metrics"]["request_id"],
        groups[1][0]["jev_metrics"]["request_id"]
    );
    let captured = server.join().unwrap();
    assert_eq!(captured[0].body, captured[1].body);
}

/// Offline operations complete while a live attempt holds the sole process slot.
#[test]
fn native_offline_commands_bypass_a_full_budget() {
    let gate = ResponseGate::default();
    let held = gate.clone();
    let (root, server) = h2_fixture::serve(3, move |_, request| match request.path.as_str() {
        "/ready" => {
            let deadline = Instant::now() + Duration::from_secs(3);
            while held.arrivals() == 0 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            assert!(held.arrivals() > 0, "live attempt did not hold its body");
            Response::json(200, json!({"ready": true}))
        }
        "/release" => {
            held.release();
            Response::json(200, json!({"released": true}))
        }
        _ => {
            let mut response = reply(request);
            response.body_gate = Some(held.clone());
            response
        }
    });
    let source = indoc! {r#"
        let q = {q: {type: noul}}

        [live offline]
        | par-each --threads 2 { |kind|
            if $kind == live {
                'held'
                | jev ask $q --timeout 4sec
            } else {
                http get $'($env.NU_PLUGIN_JEV_BASE_URL)/ready'
                | ignore
                let result = {
                    guidance: (jev)
                    noul: (jev question noul 'Q')
                    choice: (jev question choice 'Q' [a b])
                    score: (jev question score 'Q' [low high])
                    ask: ('preview' | jev ask $q --dry-run)
                    annotate: (
                        [{id: 1}]
                        | jev annotate $q --dry-run
                        | collect
                    )
                }
                http get $'($env.NU_PLUGIN_JEV_BASE_URL)/release'
                | ignore
                $result
            }
        }
        | to json --raw
    "#};
    let results = NuProcess::start(source, 1, &[("NU_PLUGIN_JEV_BASE_URL", &root)]).json();
    assert_eq!(results.as_array().unwrap().len(), 2);
    let previews = results
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result.get("ask").is_some())
        .unwrap();
    assert_eq!(previews["noul"]["type"], "noul");
    assert_eq!(previews["choice"]["type"], "choice");
    assert_eq!(previews["score"]["type"], "score");
    assert_eq!(previews["ask"]["request"]["state"], "preview");
    assert_eq!(
        server
            .join()
            .unwrap()
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        1
    );
}
