//! Composes a selected pipeline value and optional explicit context into one Jev state.

use std::collections::BTreeMap;

use nu_protocol::{LabeledError, Value};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::api::types::{Question, SystemOneRequest};

use super::value::to_json;

/// Builds the exact shared System One body from a selected Nu input.
pub(crate) fn build_request(
    input: &Value,
    context: Option<&Value>,
    model: String,
    questions: BTreeMap<String, Question>,
) -> Result<SystemOneRequest, LabeledError> {
    Ok(SystemOneRequest {
        state: compose_state(input, context)?,
        model,
        questions,
    })
}

/// Converts input and optional context, then validates the final top-level state.
pub(crate) fn compose_state(
    input: &Value,
    context: Option<&Value>,
) -> Result<JsonValue, LabeledError> {
    let input_json = to_json(input)?;
    let state = if let Some(context) = context {
        let mut wrapper = JsonMap::with_capacity(2);
        wrapper.insert("input".to_owned(), input_json);
        wrapper.insert("context".to_owned(), to_json(context)?);
        JsonValue::Object(wrapper)
    } else {
        input_json
    };
    validate_state(&state, input.span())?;
    Ok(state)
}

/// Rejects top-level JSON scalars that the TypeSafe state contract excludes.
pub(crate) fn validate_state(
    state: &JsonValue,
    span: nu_protocol::Span,
) -> Result<(), LabeledError> {
    if matches!(
        state,
        JsonValue::String(_) | JsonValue::Object(_) | JsonValue::Array(_)
    ) {
        Ok(())
    } else {
        Err(
            LabeledError::new("Jev state must be a string, record, or list")
                .with_label("unsupported top-level state", span),
        )
    }
}

#[cfg(test)]
mod tests {
    use nu_protocol::{Record, Value};
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
