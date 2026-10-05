//! Shares exact request construction and live execution for `jev ask`.

use std::collections::BTreeMap;

use nu_plugin::{EngineInterface, EvaluatedCall};
use nu_protocol::{LabeledError, Record, Span, Value};

use crate::{
    api::{
        client::{HttpMeasurement, MeasuredSuccess, PreparedRequest, request_body_bytes},
        types::{Question, SystemOneRequest, SystemOneResponse},
    },
    config::{ConfigScope, capture_sources, require_api_key, resolve},
    error::JevError,
    nu::{
        cache::next_request_id,
        signals::{InterruptRegistration, register_interrupt},
        state::{StateBuildError, build_request},
        typed::{request_to_nu, usage_to_nu},
    },
    plugin::JevPlugin,
    tracing::trace_evaluation,
};

/// Distinguishes a request preview from a completed, validated evaluation.
pub(crate) enum Evaluation {
    /// The exact body that would be sent, without authentication data.
    Preview(SystemOneRequest),
    /// The complete typed response from the service.
    Response(MeasuredSuccess<SystemOneResponse>),
}

/// Builds one body and runs async HTTP from Nu's synchronous handler with caller-owned settings.
/// Engine callbacks and file reads finish before entering Tokio; previews skip live key checks.
pub(crate) fn evaluate(
    plugin: &JevPlugin,
    engine: &EngineInterface,
    call: &EvaluatedCall,
    state: &Value,
    questions: BTreeMap<String, Question>,
) -> Result<Evaluation, LabeledError> {
    // EngineInterface getters synchronously wait for Nu replies. Capture them here rather
    // than retaining engine/call references inside HTTP tasks or blocking Tokio workers.
    let sources = capture_sources(engine, call, ConfigScope::Single)?;
    let config = resolve(&sources, ConfigScope::Single)?;
    let context = call.get_flag_value("context");
    let request = build_request(state, context.as_ref(), config.model.clone(), questions)
        .map_err(StateBuildError::into_labeled)?;
    // Preview and live execution use the same typed body; authentication is never body data.
    if call.has_flag("dry-run").map_err(LabeledError::from)? {
        return Ok(Evaluation::Preview(request));
    }
    let key = require_api_key(&sources)?;
    let prepared = PreparedRequest::new(request).map_err(JevError::into_labeled)?;
    let client = plugin
        .client
        .for_policy(&config.proxy)
        .map_err(|error| error.into_labeled())?;
    // Retain both owners through block_on: the guard handles interrupts, and dropping
    // every cancellation sender would itself end the async signal's wait as cancelled.
    let InterruptRegistration {
        guard: _guard,
        cancel: _cancel,
        signal,
    } = register_interrupt(engine, call.head)?;
    let request_id = next_request_id();
    // This blocks Nu's synchronous handler, not a Tokio worker or another command handler.
    let response = plugin
        .runtime
        .block_on(trace_evaluation(
            "ask",
            &request_id,
            client.system_one_prepared_measured(&prepared, &config, &key, signal),
        ))
        .map_err(|error| error.into_labeled())?;
    Ok(Evaluation::Response(response))
}

/// Wraps the exact outbound body with its compact UTF-8 JSON byte length.
/// Projects its JSON meaning, not original Nu-only types such as Date or Filesize.
pub(crate) fn preview_value(request: SystemOneRequest, span: Span) -> Result<Value, LabeledError> {
    let request_bytes = request_body_bytes(&request).map_err(JevError::into_labeled)?;
    let mut preview = Record::with_capacity(2);
    preview.push("request", request_to_nu(request, span)?);
    preview.push("request_bytes", Value::int(request_bytes, span));
    Ok(Value::record(preview, span))
}

/// Returns validated evaluation provenance under a fixed command-owned field.
pub(crate) fn evaluation_meta(
    base_url: &str,
    response: &SystemOneResponse,
    span: Span,
) -> Result<Value, LabeledError> {
    let mut meta = Record::with_capacity(3);
    meta.push("base_url", Value::string(base_url, span));
    meta.push("model", Value::string(response.model.clone(), span));
    meta.push("usage", usage_to_nu(&response.usage, span)?);
    Ok(Value::record(meta, span))
}

/// Converts successful HTTP measurements to Nu integers and nanosecond Durations.
/// Checks representation bounds instead of silently truncating counters or elapsed time.
pub(crate) fn measurement_value(
    measurement: &HttpMeasurement,
    request_id: Option<&str>,
    span: Span,
) -> Result<Value, LabeledError> {
    let int = |value: usize| {
        i64::try_from(value)
            .map_err(|_| LabeledError::new("Jev measurement exceeds Nu integer range"))
    };
    let duration = |value: std::time::Duration| {
        i64::try_from(value.as_nanos())
            .map_err(|_| LabeledError::new("Jev measurement exceeds Nu duration range"))
    };
    let mut fields = Record::with_capacity(if request_id.is_some() { 7 } else { 6 });
    // Row reuse needs an identity to avoid counting one HTTP result for every duplicate row.
    // Single-state and model-list envelopes contain one result and omit this extra field.
    if let Some(request_id) = request_id {
        fields.push("request_id", Value::string(request_id, span));
    }
    fields.push(
        "request_bytes",
        Value::int(int(measurement.request_bytes)?, span),
    );
    fields.push(
        "response_bytes",
        Value::int(int(measurement.response_bytes)?, span),
    );
    fields.push(
        "elapsed",
        Value::duration(duration(measurement.elapsed)?, span),
    );
    fields.push(
        "attempt_elapsed",
        Value::duration(duration(measurement.attempt_elapsed)?, span),
    );
    fields.push("attempts", Value::int(int(measurement.attempts)?, span));
    fields.push(
        "http_version",
        Value::string(measurement.http_version_name(), span),
    );
    Ok(Value::record(fields, span))
}
