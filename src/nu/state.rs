//! Composes a selected pipeline value and optional explicit context into one Jev state.

use std::{collections::BTreeMap, sync::Arc};

use nu_protocol::{LabeledError, Value};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::{
    api::types::{Question, SystemOneRequest},
    error::JevError,
};

use super::value::{ValueConversionError, to_json_checked};

/// Keeps state-construction failures typed until their output policy is known.
#[derive(Debug)]
pub(crate) enum StateBuildError {
    /// A selected value cannot be represented in JSON.
    Conversion(ValueConversionError),
    /// The final top-level state is not a string, object, or array.
    UnsupportedTopLevel(nu_protocol::Span),
}

impl StateBuildError {
    /// Preserves precise Nu diagnostics for a single-state command.
    pub(crate) fn into_labeled(self) -> LabeledError {
        match self {
            Self::Conversion(error) => error.into_labeled(),
            Self::UnsupportedTopLevel(span) => {
                LabeledError::new("Jev state must be a string, record, or list")
                    .with_label("unsupported top-level state", span)
            }
        }
    }

    /// Produces a bounded row error without copying upstream Nu error text.
    pub(crate) fn into_jev(self) -> JevError {
        // Single-state callers can propagate a native upstream diagnostic; table error records
        // must not persist arbitrary upstream text, which may contain private row data.
        match self {
            Self::Conversion(ValueConversionError::Invalid { kind, path, .. }) => {
                JevError::StateConversion { kind, path }
            }
            Self::Conversion(ValueConversionError::Upstream(_)) => {
                JevError::State("upstream Nushell value error")
            }
            Self::UnsupportedTopLevel(_) => {
                JevError::State("Jev state must be a string, record, or list")
            }
        }
    }
}

/// Builds the exact shared System One body from a selected Nu input.
pub(crate) fn build_request(
    input: &Value,
    context: Option<&Value>,
    model: String,
    questions: BTreeMap<String, Question>,
) -> Result<SystemOneRequest, StateBuildError> {
    Ok(SystemOneRequest {
        state: compose_state(input, context)?,
        model,
        questions: Arc::new(questions),
    })
}

/// Builds a row request while sharing questions and a preconverted context.
/// Each request still owns its state JSON; shared context is copied into that outbound state.
pub(crate) fn build_shared_request(
    input: &Value,
    context: Option<&JsonValue>,
    model: &str,
    questions: Arc<BTreeMap<String, Question>>,
) -> Result<SystemOneRequest, StateBuildError> {
    Ok(SystemOneRequest {
        state: compose_state_with_json_context(input, context)?,
        model: model.to_owned(),
        questions,
    })
}

/// Converts input and optional context, then validates the final top-level state.
pub(crate) fn compose_state(
    input: &Value,
    context: Option<&Value>,
) -> Result<JsonValue, StateBuildError> {
    let input_json = to_json_checked(input).map_err(StateBuildError::Conversion)?;
    let context_json = context
        .map(to_json_checked)
        .transpose()
        .map_err(StateBuildError::Conversion)?;
    finalize_state(input_json, context_json, input.span())
}

/// Converts one row while cloning already-converted static context only.
fn compose_state_with_json_context(
    input: &Value,
    context: Option<&JsonValue>,
) -> Result<JsonValue, StateBuildError> {
    let input_json = to_json_checked(input).map_err(StateBuildError::Conversion)?;
    finalize_state(input_json, context.cloned(), input.span())
}

/// Wraps optional context after the selected input has passed conversion.
fn finalize_state(
    input_json: JsonValue,
    context: Option<JsonValue>,
    span: nu_protocol::Span,
) -> Result<JsonValue, StateBuildError> {
    // Presence, not nullness, activates wrapping: --context null still creates an object.
    // A merge could overwrite input fields; this explicit wrapper keeps both namespaces intact.
    let state = if let Some(context) = context {
        let mut wrapper = JsonMap::with_capacity(2);
        wrapper.insert("input".to_owned(), input_json);
        wrapper.insert("context".to_owned(), context);
        JsonValue::Object(wrapper)
    } else {
        input_json
    };
    // Check the composed REST state, not the original Nu input. A scalar is valid when nested
    // inside the context wrapper, but cannot be sent as an unwrapped top-level state.
    validate_state(&state, span)?;
    Ok(state)
}

/// Rejects top-level JSON scalars that the TypeSafe state contract excludes.
pub(crate) fn validate_state(
    state: &JsonValue,
    span: nu_protocol::Span,
) -> Result<(), StateBuildError> {
    if matches!(
        state,
        JsonValue::String(_) | JsonValue::Object(_) | JsonValue::Array(_)
    ) {
        Ok(())
    } else {
        Err(StateBuildError::UnsupportedTopLevel(span))
    }
}

#[cfg(test)]
mod tests {
    use nu_protocol::{Record, ShellError, Span, Value};
    use serde_json::json;

    use crate::api::validate::parse_questions;

    use super::{build_request, compose_state};

    /// Distinguishes absent context from explicit null and prevents field collisions.
    #[test]
    fn explicit_context_wraps_without_merging() {
        let mut record = Record::new();
        record.push("input", Value::test_string("original"));
        let input = Value::test_record(record);
        assert_eq!(
            compose_state(&input, None).unwrap(),
            json!({"input": "original"})
        );
        assert_eq!(
            compose_state(&input, Some(&Value::test_nothing())).unwrap(),
            json!({"input": {"input": "original"}, "context": null})
        );
    }

    /// Rejects duplicate keys in either outbound state or explicit context.
    #[test]
    fn duplicate_keys_in_state_or_context_fail() {
        let mut duplicate = Record::new();
        duplicate.push("same", Value::test_int(1));
        duplicate.push("same", Value::test_int(2));
        let duplicate = Value::test_record(duplicate);

        let state_error = compose_state(&duplicate, None).unwrap_err().into_labeled();
        assert!(state_error.msg.contains("duplicate record key"));
        assert!(state_error.msg.contains("$.same"));

        let context_error = compose_state(&Value::test_string("hello"), Some(&duplicate))
            .unwrap_err()
            .into_labeled();
        assert!(context_error.msg.contains("duplicate record key"));
        assert!(context_error.msg.contains("$.same"));
    }

    /// Retains native upstream errors for ask without copying them into row records.
    #[test]
    fn nested_upstream_error_is_redacted_for_table_rows() {
        let secret = "private upstream diagnostic";
        let nested = Value::error(
            ShellError::from(nu_protocol::LabeledError::new(secret)),
            Span::test_data(),
        );
        let mut row = Record::new();
        row.push("nested", Value::test_list(vec![nested]));
        let row = Value::test_record(row);
        let labeled = compose_state(&row, None).unwrap_err().into_labeled();
        assert_eq!(labeled.msg, secret);
        let classified = compose_state(&row, None).unwrap_err().into_jev();
        assert_eq!(classified.kind_name(), "state");
        assert_eq!(classified.status(), None);
        assert_eq!(classified.to_string(), "upstream Nushell value error");
        assert!(!classified.to_labeled().msg.contains(secret));
    }

    /// Allows nested JSON scalars while excluding bare scalar states.
    #[test]
    fn checks_only_top_level_type() {
        let mut record = Record::new();
        record.push("number", Value::test_int(2));
        record.push("empty", Value::test_nothing());
        assert!(compose_state(&Value::test_record(record), None).is_ok());
        assert!(compose_state(&Value::test_list(vec![Value::test_bool(true)]), None).is_ok());
        assert!(compose_state(&Value::test_string("hello"), None).is_ok());
        assert!(compose_state(&Value::test_int(2), None).is_err());
        assert!(compose_state(&Value::test_nothing(), None).is_err());
        assert_eq!(
            compose_state(&Value::test_int(2), Some(&Value::test_nothing())).unwrap(),
            json!({"input": 2, "context": null})
        );
    }

    /// Matches the documented mixed-question request without flattening state.
    #[test]
    fn builds_exact_mixed_question_body() {
        let mut input = Record::new();
        input.push("message", Value::test_string("Hello"));
        input.push("sender", Value::test_string("Ada"));
        let questions_wire = json!({
            "spam": {"type": "noul", "instructions": "Is this unsolicited?",
                "criteria": {"true": "spam", "false": "expected"}},
            "kind": {"type": "choice", "instructions": {"task": "categorize", "priority": 1},
                "criteria": {"normal": null, "promo": "advertising"}},
            "urgency": {"type": "score", "criteria": ["later", "today", "now"]}
        });
        let questions = parse_questions(
            &crate::nu::value::from_json(questions_wire.clone(), nu_protocol::Span::test_data())
                .unwrap(),
        )
        .unwrap();
        let body = build_request(
            &Value::test_record(input),
            None,
            "jev-latest".to_owned(),
            questions,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(body).unwrap(),
            json!({"state": {"message": "Hello", "sender": "Ada"},
                "model": "jev-latest", "questions": questions_wire})
        );
    }
}
