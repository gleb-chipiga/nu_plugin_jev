//! Shares exact request construction and live execution for `jev ask`.

use std::collections::BTreeMap;

use nu_plugin::{EngineInterface, EvaluatedCall};
use nu_protocol::{LabeledError, SignalAction, Value};

use crate::{
    api::{
        cancel::CancelHandle,
        types::{Question, SystemOneRequest, SystemOneResponse},
    },
    config::{ConfigScope, capture_sources, require_api_key, resolve},
    nu::{cache::next_request_id, state::build_request, value::from_json},
    plugin::JevPlugin,
    tracing::trace_evaluation,
};

/// Distinguishes a request preview from a completed, validated evaluation.
pub(crate) enum Evaluation {
    /// The exact body that would be sent, without authentication data.
    Preview(SystemOneRequest),
    /// The complete typed response from the service.
    Response(SystemOneResponse),
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
            client.system_one(&request, &config, &key, signal),
        ))
        .map_err(|error| error.into_labeled())?;
    Ok(Evaluation::Response(response))
}

/// Projects any serializable typed API value back into an ordinary Nu value.
pub(crate) fn to_nu<T: serde::Serialize>(
    value: T,
    span: nu_protocol::Span,
) -> Result<Value, LabeledError> {
    let json =
        serde_json::to_value(value).map_err(|_| LabeledError::new("cannot encode Jev result"))?;
    from_json(json, span)
}
