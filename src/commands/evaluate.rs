//! Shares exact request construction and live execution for `jev ask`.

use std::collections::BTreeMap;

use nu_plugin::{EngineInterface, EvaluatedCall};
use nu_protocol::{LabeledError, Record, SignalAction, Span, Value};

use crate::{
    api::{
        cancel::CancelHandle,
        client::{HttpMeasurement, MeasuredSuccess, PreparedRequest, request_body_bytes},
        types::{Question, SystemOneRequest, SystemOneResponse},
    },
    config::{ConfigScope, capture_sources, require_api_key, resolve},
    error::JevError,
    nu::{
        cache::next_request_id,
        state::build_request,
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

/// Builds one body and either previews it or sends it with caller-scoped settings.
pub(crate) fn evaluate(
    plugin: &JevPlugin,
    engine: &EngineInterface,
    call: &EvaluatedCall,
    state: &Value,
    questions: BTreeMap<String, Question>,
) -> Result<Evaluation, LabeledError> {
    let sources = capture_sources(engine, call, ConfigScope::Single)?;
    let config = resolve(&sources, ConfigScope::Single)?;
    let context = call.get_flag_value("context");
    let request = build_request(state, context.as_ref(), config.model.clone(), questions)?;
    if call.has_flag("dry-run").map_err(LabeledError::from)? {
        return Ok(Evaluation::Preview(request));
    }
    let key = require_api_key(&sources)?;
    let prepared = PreparedRequest::new(request).map_err(JevError::into_labeled)?;
    let client = plugin
        .client
        .for_policy(&config.proxy)
        .map_err(|error| error.into_labeled())?;
    engine
        .signals()
        .check(&call.head)
        .map_err(LabeledError::from)?;
    let (cancel, signal) = CancelHandle::new();
    let handler = cancel.clone();
    let _guard = engine
        .register_signal_handler(Box::new(move |action| {
            if action == SignalAction::Interrupt {
                handler.cancel();
            }
        }))
        .map_err(LabeledError::from)?;
    let request_id = next_request_id();
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

/// Converts one successful HTTP measurement to native Nu sizes and durations.
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
