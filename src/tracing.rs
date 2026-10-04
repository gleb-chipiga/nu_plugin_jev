//! Configures non-blocking process diagnostics and traces logical API operations.

/// Serializes tracing events as newline-delimited NUON records.
#[cfg(feature = "nuon-tracing-format")]
mod nuon;

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    io,
    time::Instant,
};

use ::tracing::Instrument;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

use crate::{
    api::{
        client::MeasuredSuccess,
        types::{ModelMetadataList, SystemOneResponse},
    },
    error::JevError,
};

/// Holds an explicit process-wide target filter and the log-bridge ceiling.
struct DiagnosticFilter {
    directives: BTreeMap<String, LevelFilter>,
    max_log: log::LevelFilter,
}

/// Selects the process-wide diagnostic presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiagnosticFormat {
    Text,
    #[cfg(feature = "nuon-tracing-format")]
    Nuon,
}

impl DiagnosticFilter {
    /// Converts the validated directives to a filter with no global fallback.
    fn targets(&self) -> Targets {
        self.directives
            .iter()
            .fold(Targets::new(), |targets, (target, level)| {
                targets.with_target(target.clone(), *level)
            })
    }
}

/// Parses legacy shorthand or a strict list of target-specific levels.
fn parse_filter(value: Option<&str>) -> Result<DiagnosticFilter, io::Error> {
    let mut directives = BTreeMap::new();
    let value = value.unwrap_or("warn");
    if let Some((level, _)) = parse_level(value) {
        directives.insert("nu_plugin_jev".to_owned(), level);
    } else {
        directives.insert("nu_plugin_jev".to_owned(), LevelFilter::WARN);
        let mut seen = BTreeSet::new();
        for directive in value.split(',') {
            let (target, level) = directive.split_once('=').ok_or_else(invalid_filter)?;
            let target = target.trim();
            let level = level.trim();
            if !valid_target(target) || !seen.insert(target) {
                return Err(invalid_filter());
            }
            let (level, _) = parse_level(level).ok_or_else(invalid_filter)?;
            directives.insert(target.to_owned(), level);
        }
    }
    let max_log = directives
        .values()
        .copied()
        .map(log_level)
        .max()
        .unwrap_or(log::LevelFilter::Off);
    Ok(DiagnosticFilter {
        directives,
        max_log,
    })
}

/// Validates a Rust target path without allowing wildcard or global directives.
fn valid_target(target: &str) -> bool {
    target.split("::").all(|segment| {
        let mut bytes = segment.bytes();
        bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    })
}

/// Recognizes the six documented verbosity values.
fn parse_level(value: &str) -> Option<(LevelFilter, log::LevelFilter)> {
    match value {
        "off" => Some((LevelFilter::OFF, log::LevelFilter::Off)),
        "error" => Some((LevelFilter::ERROR, log::LevelFilter::Error)),
        "warn" => Some((LevelFilter::WARN, log::LevelFilter::Warn)),
        "info" => Some((LevelFilter::INFO, log::LevelFilter::Info)),
        "debug" => Some((LevelFilter::DEBUG, log::LevelFilter::Debug)),
        "trace" => Some((LevelFilter::TRACE, log::LevelFilter::Trace)),
        _ => None,
    }
}

/// Maps a tracing ceiling to the equivalent `log` ceiling.
fn log_level(level: LevelFilter) -> log::LevelFilter {
    match level {
        LevelFilter::OFF => log::LevelFilter::Off,
        LevelFilter::ERROR => log::LevelFilter::Error,
        LevelFilter::WARN => log::LevelFilter::Warn,
        LevelFilter::INFO => log::LevelFilter::Info,
        LevelFilter::DEBUG => log::LevelFilter::Debug,
        LevelFilter::TRACE => log::LevelFilter::Trace,
    }
}

/// Reports a malformed filter without reproducing its potentially sensitive value.
fn invalid_filter() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid NU_PLUGIN_JEV_LOG filter",
    )
}

/// Parses the explicit output format without exposing an invalid value.
fn parse_format(value: Option<&str>) -> Result<DiagnosticFormat, io::Error> {
    match value.unwrap_or("text") {
        "text" => Ok(DiagnosticFormat::Text),
        #[cfg(feature = "nuon-tracing-format")]
        "nuon" => Ok(DiagnosticFormat::Nuon),
        #[cfg(not(feature = "nuon-tracing-format"))]
        "nuon" => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUON diagnostics require the nuon-tracing-format Cargo feature",
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid NU_PLUGIN_JEV_LOG_FORMAT setting",
        )),
    }
}

/// Reads an optional process setting without echoing a non-Unicode value.
fn process_setting(name: &str) -> Result<Option<String>, io::Error> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid {name} setting"),
        )),
    }
}

/// Sends filtered diagnostics to stderr without blocking async request workers.
pub(crate) fn init() -> Result<WorkerGuard, Box<dyn std::error::Error + Send + Sync>> {
    let value = process_setting("NU_PLUGIN_JEV_LOG")?;
    let filter = parse_filter(value.as_deref())?;
    let value = process_setting("NU_PLUGIN_JEV_LOG_FORMAT")?;
    let format = parse_format(value.as_deref())?;
    let (writer, guard) = tracing_appender::non_blocking(std::io::stderr());
    let max_log = filter.max_log;
    let subscriber = tracing_subscriber::registry().with(filter.targets());
    match format {
        DiagnosticFormat::Text => subscriber
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false),
            )
            .try_init()?,
        #[cfg(feature = "nuon-tracing-format")]
        DiagnosticFormat::Nuon => subscriber
            .with(nuon::SpanFieldLayer)
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(nuon::NuonFormatter)
                    .with_writer(writer)
                    .with_ansi(false),
            )
            .try_init()?,
    }
    tracing_log::LogTracer::builder()
        .with_max_level(max_log)
        .init()?;
    Ok(guard)
}

/// Traces one catalog lookup without recording credentials or model contents.
pub(crate) async fn trace_models<F>(
    request_id: &str,
    operation: F,
) -> Result<MeasuredSuccess<ModelMetadataList>, JevError>
where
    F: Future<Output = Result<MeasuredSuccess<ModelMetadataList>, JevError>>,
{
    let span = ::tracing::warn_span!("jev_models", request_id);
    async move {
        let started = Instant::now();
        ::tracing::info!("model listing started");
        let result = operation.await;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &result {
            Ok(success) => ::tracing::info!(
                duration_ms,
                count = success.response.models.len(),
                base_url = %success.base_url,
                request_bytes = u64::try_from(success.measurement.request_bytes).unwrap_or(u64::MAX),
                response_bytes = u64::try_from(success.measurement.response_bytes).unwrap_or(u64::MAX),
                elapsed_ns = u64::try_from(success.measurement.elapsed.as_nanos()).unwrap_or(u64::MAX),
                attempt_elapsed_ns = u64::try_from(success.measurement.attempt_elapsed.as_nanos()).unwrap_or(u64::MAX),
                attempts = u64::try_from(success.measurement.attempts).unwrap_or(u64::MAX),
                http_version = success.measurement.http_version_name(),
                "model listing completed"
            ),
            Err(JevError::Cancelled) => {
                ::tracing::debug!(duration_ms, "model listing cancelled");
            }
            Err(error) => ::tracing::warn!(
                duration_ms,
                kind = error.kind_name(),
                status = ?error.status(),
                "model listing failed"
            ),
        }
        result
    }
    .instrument(span)
    .await
}

/// Traces one logical evaluation without recording credentials or request content.
pub(crate) async fn trace_evaluation<F>(
    command: &'static str,
    request_id: &str,
    operation: F,
) -> Result<MeasuredSuccess<SystemOneResponse>, JevError>
where
    F: Future<Output = Result<MeasuredSuccess<SystemOneResponse>, JevError>>,
{
    let span = ::tracing::warn_span!("jev_evaluation", command, request_id);
    async move {
        let started = Instant::now();
        ::tracing::info!("evaluation started");
        let result = operation.await;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &result {
            Ok(success) => ::tracing::info!(
                duration_ms,
                model = %success.response.model,
                input_tokens = success.response.usage.input_tokens,
                output_tokens = success.response.usage.output_tokens,
                base_url = %success.base_url,
                request_bytes = u64::try_from(success.measurement.request_bytes).unwrap_or(u64::MAX),
                response_bytes = u64::try_from(success.measurement.response_bytes).unwrap_or(u64::MAX),
                elapsed_ns = u64::try_from(success.measurement.elapsed.as_nanos()).unwrap_or(u64::MAX),
                attempt_elapsed_ns = u64::try_from(success.measurement.attempt_elapsed.as_nanos()).unwrap_or(u64::MAX),
                attempts = u64::try_from(success.measurement.attempts).unwrap_or(u64::MAX),
                http_version = success.measurement.http_version_name(),
                "evaluation completed"
            ),
            Err(JevError::Cancelled) => {
                ::tracing::debug!(duration_ms, "evaluation cancelled");
            }
            Err(error) => ::tracing::warn!(
                duration_ms,
                kind = error.kind_name(),
                status = ?error.status(),
                "evaluation failed"
            ),
        }
        result
    }
    .instrument(span)
    .await
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Output};

    use tracing_subscriber::filter::LevelFilter;

    use super::{DiagnosticFormat, parse_filter, parse_format};

    /// Runs the one-test fixture in a fresh process with isolated global logging.
    fn run_diagnostic_child(filter: &str, format: Option<&str>) -> Output {
        let mut command = Command::new(std::env::current_exe().expect("test binary path"));
        command
            .args([
                "--exact",
                "tracing::tests::diagnostic_child_fixture",
                "--nocapture",
            ])
            .env("JEV_DIAGNOSTIC_CHILD", "1")
            .env("NU_PLUGIN_JEV_LOG", filter)
            .env_remove("NU_PLUGIN_JEV_LOG_FORMAT")
            .env_remove("RUST_LOG");
        if let Some(format) = format {
            command.env("NU_PLUGIN_JEV_LOG_FORMAT", format);
        }
        command.output().expect("run diagnostic fixture")
    }

    /// Emits native and bridged records only inside its isolated subprocess.
    #[test]
    fn diagnostic_child_fixture() {
        if std::env::var_os("JEV_DIAGNOSTIC_CHILD").is_none() {
            return;
        }
        let guard = super::init().expect("initialize isolated diagnostics");
        let outer = ::tracing::info_span!(target: "nu_plugin_jev::diagnostic_fixture", "outer", command = "ask", request_id = "initial", updated = ::tracing::field::Empty);
        let _outer_guard = outer.enter();
        outer.record("updated", 42_i64);
        let inner = ::tracing::info_span!(target: "nu_plugin_jev::diagnostic_fixture", "inner", kind = "nested");
        let _inner_guard = inner.enter();
        ::tracing::info!(target: "nu_plugin_jev::diagnostic_fixture", count = 7, finite = 1.5_f64, nan = f64::NAN, large = u64::MAX, "odd key" = true, text = "quote\"\n雪", "plugin fixture");
        log::debug!(target: "reqwest", "reqwest fixture");
        log::warn!(target: "unlisted_dependency", "unlisted fixture");
        drop(guard);
    }

    /// Keeps dependency diagnostics off for defaults and legacy shorthand.
    #[test]
    fn default_and_shorthand_are_plugin_only() {
        let default = parse_filter(None).unwrap();
        assert_eq!(default.directives.len(), 1);
        assert_eq!(default.directives["nu_plugin_jev"], LevelFilter::WARN);
        assert_eq!(default.max_log, log::LevelFilter::Warn);
        let debug = parse_filter(Some("debug")).unwrap();
        assert_eq!(debug.directives.len(), 1);
        assert_eq!(debug.directives["nu_plugin_jev"], LevelFilter::DEBUG);
        assert_eq!(debug.max_log, log::LevelFilter::Debug);
        let off = parse_filter(Some("off")).unwrap();
        assert_eq!(off.directives["nu_plugin_jev"], LevelFilter::OFF);
        assert_eq!(off.max_log, log::LevelFilter::Off);
    }

    /// Allows dependency targets and more-specific Rust module overrides.
    #[test]
    fn target_directives_preserve_implicit_plugin_warn() {
        let filter = parse_filter(Some(
            "reqwest=debug,nu_plugin_jev=info,nu_plugin_jev::api=trace",
        ))
        .unwrap();
        assert_eq!(filter.directives["reqwest"], LevelFilter::DEBUG);
        assert_eq!(filter.directives["nu_plugin_jev"], LevelFilter::INFO);
        assert_eq!(filter.directives["nu_plugin_jev::api"], LevelFilter::TRACE);
        assert_eq!(filter.max_log, log::LevelFilter::Trace);
        let dependency_only = parse_filter(Some("reqwest=debug")).unwrap();
        assert_eq!(
            dependency_only.directives["nu_plugin_jev"],
            LevelFilter::WARN
        );
    }

    /// Rejects malformed or ambiguous directives without echoing their values.
    #[test]
    fn malformed_directives_are_redacted() {
        for value in [
            "",
            "reqwest",
            "reqwest=verbose",
            "reqwest=debug,",
            "=debug",
            "reqwest=debug,reqwest=info",
            "nu_plugin_jev=info,nu_plugin_jev=debug",
            "info,reqwest=debug",
            "reqwest::=trace",
            "secret-token=debug,broken",
        ] {
            let error = parse_filter(Some(value)).err().expect("invalid filter");
            assert_eq!(error.to_string(), "invalid NU_PLUGIN_JEV_LOG filter");
            assert!(!error.to_string().contains("secret-token"));
        }
    }

    /// Keeps text as the default and rejects unknown explicit formats.
    #[test]
    fn format_selection_is_strict() {
        assert_eq!(parse_format(None).unwrap(), DiagnosticFormat::Text);
        #[cfg(feature = "nuon-tracing-format")]
        assert_eq!(parse_format(Some("nuon")).unwrap(), DiagnosticFormat::Nuon);
        let error = parse_format(Some("secret-format")).unwrap_err();
        assert_eq!(
            error.to_string(),
            "invalid NU_PLUGIN_JEV_LOG_FORMAT setting"
        );
        assert!(!error.to_string().contains("secret-format"));
    }

    /// Round-trips typed event and recorded span fields through the NUON subset.
    #[test]
    #[cfg(feature = "nuon-tracing-format")]
    fn nuon_formatter_preserves_fields_and_one_line_per_event() {
        let output = run_diagnostic_child("nu_plugin_jev=info,reqwest=debug", Some("nuon"));
        assert!(output.status.success());
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostics");
        let lines = stderr.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2, "unexpected diagnostic lines: {stderr}");
        let records = lines
            .iter()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("NUON subset"))
            .collect::<Vec<_>>();
        let plugin = records
            .iter()
            .find(|record| record["target"] == "nu_plugin_jev::diagnostic_fixture")
            .expect("plugin record");
        assert_eq!(plugin["level"], "info");
        assert_eq!(plugin["message"], "plugin fixture");
        assert_eq!(plugin["fields"]["count"], 7);
        assert_eq!(plugin["fields"]["finite"], 1.5);
        assert_eq!(plugin["fields"]["nan"], "NaN");
        assert_eq!(plugin["fields"]["large"], u64::MAX.to_string());
        assert_eq!(plugin["fields"]["odd key"], true);
        assert_eq!(plugin["fields"]["text"], "quote\"\n雪");
        assert_eq!(plugin["spans"][0]["name"], "outer");
        assert_eq!(plugin["spans"][0]["fields"]["command"], "ask");
        assert_eq!(plugin["spans"][0]["fields"]["request_id"], "initial");
        assert_eq!(plugin["spans"][0]["fields"]["updated"], 42);
        assert_eq!(plugin["spans"][1]["name"], "inner");
        assert_eq!(plugin["spans"][1]["fields"]["kind"], "nested");
        assert!(
            plugin["timestamp"]
                .as_str()
                .is_some_and(|value| value.ends_with('Z'))
        );
        let dependency = records
            .iter()
            .find(|record| record["target"] == "reqwest")
            .unwrap_or_else(|| panic!("bridged log record missing: {records:?}"));
        assert_eq!(dependency["level"], "debug");
        assert_eq!(dependency["message"], "reqwest fixture");
    }

    /// Confirms Nu 0.116 parses escaped records with both supported readers.
    #[test]
    #[cfg(feature = "nuon-tracing-format")]
    fn nuon_formatter_round_trips_through_nu() {
        if Command::new("nu").arg("--version").output().is_err() {
            return;
        }
        let emitted = run_diagnostic_child("nu_plugin_jev=info,reqwest=debug", Some("nuon"));
        assert!(emitted.status.success());
        let diagnostics = String::from_utf8(emitted.stderr).expect("UTF-8 diagnostics");
        let parsed = Command::new("nu")
            .args([
                "--no-config-file",
                "--commands",
                "use std/formats *; {single: ($env.JEV_TEST_DIAGNOSTICS | lines | first | from nuon), batch: ($env.JEV_TEST_DIAGNOSTICS | from ndnuon)} | to json --raw",
            ])
            .env("JEV_TEST_DIAGNOSTICS", diagnostics)
            .output()
            .expect("parse fixture in Nu");
        assert!(
            parsed.status.success(),
            "Nu parser failed: {}",
            String::from_utf8_lossy(&parsed.stderr)
        );
        let records: serde_json::Value =
            serde_json::from_slice(&parsed.stdout).expect("Nu parser output");
        assert_eq!(records["batch"].as_array().unwrap().len(), 2);
        assert_eq!(records["single"], records["batch"][0]);
        assert_eq!(records["single"]["fields"]["odd key"], true);
        assert_eq!(records["single"]["fields"]["text"], "quote\"\n雪");
        assert_eq!(records["single"]["fields"]["nan"], "NaN");
    }

    /// Invalid format selection fails before logging or starting commands.
    #[test]
    fn invalid_process_format_fails_redacted() {
        let output = run_diagnostic_child("debug", Some("secret-format"));
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostics");
        assert!(stderr.contains("invalid NU_PLUGIN_JEV_LOG_FORMAT setting"));
        assert!(!stderr.contains("secret-format"));
    }

    /// Rejects NUON selection when the formatter is not compiled into the plugin.
    #[test]
    #[cfg(not(feature = "nuon-tracing-format"))]
    fn nuon_format_requires_feature() {
        let error = parse_format(Some("nuon")).unwrap_err();
        assert_eq!(
            error.to_string(),
            "NUON diagnostics require the nuon-tracing-format Cargo feature"
        );
        let output = run_diagnostic_child("debug", Some("nuon"));
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostics");
        assert!(stderr.contains("NUON diagnostics require the nuon-tracing-format Cargo feature"));
        assert!(!stderr.contains("plugin fixture"));
    }

    /// Filters native tracing and bridged dependency logs through one policy.
    #[test]
    fn native_and_log_events_share_target_filter() {
        let output = run_diagnostic_child("nu_plugin_jev=info,reqwest=debug", None);
        assert!(output.status.success());
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostics");
        assert_eq!(stderr.matches("plugin fixture").count(), 1);
        assert_eq!(stderr.matches("reqwest fixture").count(), 1);
        assert!(!stderr.contains("unlisted fixture"));

        let shorthand = run_diagnostic_child("debug", None);
        assert!(shorthand.status.success());
        let stderr = String::from_utf8(shorthand.stderr).expect("UTF-8 diagnostics");
        assert!(stderr.contains("plugin fixture"));
        assert!(!stderr.contains("reqwest fixture"));

        let dependency_only = run_diagnostic_child("reqwest=debug", None);
        assert!(dependency_only.status.success());
        let stderr = String::from_utf8(dependency_only.stderr).expect("UTF-8 diagnostics");
        assert!(!stderr.contains("plugin fixture"));
        assert!(stderr.contains("reqwest fixture"));
    }

    /// Rejects invalid startup filters without echoing their raw contents.
    #[test]
    fn invalid_process_filter_fails_redacted() {
        let output = run_diagnostic_child("reqwest=debug,secret-token=invalid", None);
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostics");
        assert!(stderr.contains("invalid NU_PLUGIN_JEV_LOG filter"));
        assert!(!stderr.contains("secret-token"));
    }
}
