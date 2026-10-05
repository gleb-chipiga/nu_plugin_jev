//! Checks TypeSafe question shapes and validates service answers against submitted names.

use std::collections::BTreeMap;

use nu_protocol::{LabeledError, Span, Value};
use serde_json::Value as JsonValue;

use crate::nu::value::to_json_at;

use super::types::{Answer, NoulCriteria, Question, SystemOneResponse};

/// Parses named Nu question data into strict typed contracts before any state dispatch.
/// Handles raw records as well as constructor output; syntax shapes alone cannot validate them.
pub(crate) fn parse_questions(value: &Value) -> Result<BTreeMap<String, Question>, LabeledError> {
    let Value::Record { val: questions, .. } = value else {
        return Err(question_error("questions must be a record", value.span()));
    };
    if questions.is_empty() {
        return Err(question_error(
            "questions record must not be empty",
            value.span(),
        ));
    }
    questions
        .iter()
        .try_fold(BTreeMap::new(), |mut parsed, (name, body)| {
            if parsed.contains_key(name) {
                return Err(question_error(
                    format!("duplicate question name {name:?}"),
                    body.span(),
                ));
            }
            // Generic conversion checks nested Nu values and duplicate keys; Serde then checks
            // the fixed tagged schema and unknown fields. Neither step replaces the other.
            let body = to_json_at(body, format!("$.{name}"))?;
            let question: Question = serde_json::from_value(body).map_err(|error| {
                question_error(
                    format!("question {name:?} has invalid fields: {error}"),
                    value.span(),
                )
            })?;
            validate_question(name, &question, value.span())?;
            parsed.insert(name.clone(), question);
            Ok(parsed)
        })
}

/// Checks the root shapes and cardinality defined by the raw API schema.
/// Constructor shorthand limits are separate and do not restrict hand-authored API questions.
pub(crate) fn validate_question(
    name: &str,
    question: &Question,
    span: Span,
) -> Result<(), LabeledError> {
    let fail = |field: &str, reason: &str| {
        question_error(format!("question {name:?} {field}: {reason}"), span)
    };
    let instructions = match question {
        Question::Noul { instructions, .. }
        | Question::Choice { instructions, .. }
        | Question::Score { instructions, .. } => instructions,
    };
    if instructions.as_ref().is_some_and(invalid_root_description) {
        return Err(fail(
            "instructions",
            "expected string, record, list, or null",
        ));
    }
    match question {
        Question::Noul { criteria, .. } => {
            if let Some(criteria) = criteria {
                match criteria {
                    NoulCriteria::Null => {}
                    NoulCriteria::Descriptions(descriptions) => {
                        for (key, description) in [
                            ("true", descriptions.yes.as_ref()),
                            ("false", descriptions.no.as_ref()),
                        ] {
                            if description.is_some_and(invalid_root_description) {
                                return Err(fail(
                                    &format!("criteria.{key}"),
                                    "invalid description type",
                                ));
                            }
                        }
                    }
                }
            }
        }
        Question::Choice { criteria, .. } => {
            if criteria.is_empty() {
                return Err(fail("criteria", "at least one option is required"));
            }
            for (key, description) in criteria {
                if invalid_root_description(description) {
                    return Err(fail(&format!("criteria.{key}"), "invalid description type"));
                }
            }
        }
        Question::Score { criteria, .. } => {
            if criteria.is_empty() {
                return Err(fail("criteria", "at least one level is required"));
            }
            for (index, description) in criteria.iter().enumerate() {
                if description.is_null() || invalid_root_description(description) {
                    return Err(fail(
                        &format!("criteria[{index}]"),
                        "expected string, record, or list",
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Checks answer names, variants, numeric domains, and membership in submitted options.
pub(crate) fn validate_response(
    response: &SystemOneResponse,
    questions: &BTreeMap<String, Question>,
) -> Result<(), LabeledError> {
    if response.model.trim().is_empty() {
        return Err(response_error("response model is empty"));
    }
    if response.answers.len() != questions.len() {
        return Err(response_error(
            "response answer names do not match submitted questions",
        ));
    }
    for (name, question) in questions {
        let answer = response
            .answers
            .get(name)
            .ok_or_else(|| response_error(format!("response is missing answer {name:?}")))?;
        let fail = |reason: &str| response_error(format!("answer {name:?}: {reason}"));
        match (question, answer) {
            (Question::Noul { .. }, Answer::Noul { noul }) => {
                if !probability(*noul) {
                    return Err(fail("noul must be a probability in 0..=1"));
                }
            }
            (
                Question::Choice { criteria, .. },
                Answer::Choice {
                    choice,
                    confidence,
                    probabilities,
                },
            ) => {
                if !criteria.contains_key(choice) {
                    return Err(fail("choice is not a submitted option"));
                }
                if !probability(*confidence) {
                    return Err(fail("confidence must be in 0..=1"));
                }
                if !probabilities.contains_key(choice) || probabilities.is_empty() {
                    return Err(fail("probabilities omit the selected choice"));
                }
                for (option, probability_value) in probabilities {
                    if !criteria.contains_key(option) || !probability(*probability_value) {
                        return Err(fail("probabilities contain an invalid option or value"));
                    }
                }
            }
            (
                Question::Score { criteria, .. },
                Answer::Score {
                    score,
                    confidence,
                    legend,
                    probabilities,
                },
            ) => {
                if !score.is_finite() || *score < 0.0 || *score > (criteria.len() - 1) as f64 {
                    return Err(fail("score is outside submitted levels"));
                }
                if !probability(*confidence) {
                    return Err(fail("confidence must be in 0..=1"));
                }
                if probabilities.is_empty() {
                    return Err(fail("probabilities are empty"));
                }
                for (level, probability_value) in probabilities {
                    if !valid_level(level, criteria.len()) || !probability(*probability_value) {
                        return Err(fail("probabilities contain an invalid level or value"));
                    }
                }
                for level in legend.keys() {
                    if !valid_level(level, criteria.len()) {
                        return Err(fail("legend contains an invalid level"));
                    }
                }
            }
            _ => return Err(fail("answer type does not match question type")),
        }
    }
    Ok(())
}

/// Identifies root types disallowed for instructions and Noul/Choice descriptions.
fn invalid_root_description(value: &JsonValue) -> bool {
    matches!(value, JsonValue::Number(_) | JsonValue::Bool(_))
}

/// Tests whether a server number is a representable probability.
fn probability(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

/// Tests whether a score key names an in-range ordinal level.
fn valid_level(level: &str, level_count: usize) -> bool {
    level
        .parse::<usize>()
        .is_ok_and(|index| index < level_count && index.to_string() == level)
}

/// Produces a labeled input-contract error for a question field.
fn question_error(message: impl Into<String>, span: Span) -> LabeledError {
    LabeledError::new(message).with_label("invalid Jev question", span)
}

/// Produces a service-contract error without attaching unrelated source spans.
fn response_error(message: impl Into<String>) -> LabeledError {
    LabeledError::new(message).with_code("jev::response_contract")
}

#[cfg(test)]
mod tests {
    use nu_protocol::{Record, Span, Value};
    use serde_json::json;

    use crate::nu::value::from_json;

    use super::{parse_questions, validate_response};
    use crate::api::types::SystemOneResponse;

    /// Converts a JSON fixture to a Nu value for raw question tests.
    fn nu(value: serde_json::Value) -> nu_protocol::Value {
        from_json(value, Span::test_data()).unwrap()
    }

    /// Accepts schema-aligned raw questions with structured optional values.
    #[test]
    fn validates_raw_questions_without_constructor_maxima() {
        let questions = parse_questions(&nu(json!({
            "n": {"type": "noul", "instructions": null, "criteria": {"true": {"weight": 2}}},
            "c": {"type": "choice", "criteria": {"a": null, "b": ["text", 2]}},
            "s": {"type": "score", "criteria":
                (0..11).map(|n| json!({"level": n})).collect::<Vec<_>>()}
        })))
        .unwrap();
        assert_eq!(questions.len(), 3);
    }

    /// Rejects empty maps, root scalars, and an empty or null Score rubric.
    #[test]
    fn rejects_invalid_raw_questions() {
        for fixture in [
            json!({}),
            json!({"q": {"type": "noul", "instructions": 2}}),
            json!({"q": {"type": "noul", "instrucitons": "typo"}}),
            json!({"q": {"type": "choice", "criteria": {"x": null}, "instrucitons": "typo"}}),
            json!({"q": {"type": "score", "criteria": ["low"], "instrucitons": "typo"}}),
            json!({"q": {"type": "choice", "criteria": {"x": true}}}),
            json!({"q": {"type": "score", "criteria": []}}),
            json!({"q": {"type": "score", "criteria": [null]}}),
        ] {
            assert!(parse_questions(&nu(fixture)).is_err());
        }
    }

    /// Rejects repeated question names instead of discarding an earlier decision.
    #[test]
    fn rejects_duplicate_question_names() {
        let mut questions = Record::new();
        let question = || nu(json!({"type": "noul"}));
        questions.push("same", question());
        questions.push("same", question());
        let error = parse_questions(&Value::test_record(questions)).unwrap_err();
        assert!(error.msg.contains("duplicate question name"));
    }

    /// Rejects repeated nested keys before deserializing a raw question.
    #[test]
    fn rejects_duplicate_nested_question_keys() {
        let mut instructions = Record::new();
        instructions.push("same", Value::test_string("first"));
        instructions.push("same", Value::test_string("second"));
        let mut body = Record::new();
        body.push("type", Value::test_string("noul"));
        body.push("instructions", Value::test_record(instructions));
        let mut questions = Record::new();
        questions.push("q", Value::test_record(body));

        let error = parse_questions(&Value::test_record(questions)).unwrap_err();
        assert!(error.msg.contains("duplicate record key"));
        assert!(error.msg.contains("$.q.instructions.same"));
    }

    /// Retains the complete question path when converting one invalid field.
    #[test]
    fn reports_nested_question_conversion_path() {
        let mut body = Record::new();
        body.push("type", Value::test_string("noul"));
        body.push(
            "instructions",
            Value::test_list(vec![Value::binary(vec![1], Span::test_data())]),
        );
        let mut questions = Record::new();
        questions.push("q", Value::test_record(body));
        let error = parse_questions(&Value::test_record(questions)).unwrap_err();
        assert!(error.msg.contains("$.q.instructions[0]"));
    }

    /// Rejects absent and mistyped answers, invalid domains, and unknown choices or levels.
    #[test]
    fn rejects_invalid_responses() {
        let questions = parse_questions(&nu(json!({
            "n": {"type": "noul"},
            "c": {"type": "choice", "criteria": {"a": null}},
            "s": {"type": "score", "criteria": ["low", "high"]}
        })))
        .unwrap();
        let valid = json!({"model": "jev-latest", "usage": {"input_tokens": 1, "output_tokens": 2},
        "answers": {
            "n": {"type": "noul", "noul": 0.5},
            "c": {"type": "choice", "choice": "a", "confidence": 0.8,
                "probabilities": {"a": 0.8}},
            "s": {"type": "score", "score": 0.5, "confidence": 0.7,
                "legend": {"0": "low", "1": "high"},
                "probabilities": {"0": 0.5, "1": 0.5}}
        }});
        let response: SystemOneResponse = serde_json::from_value(valid.clone()).unwrap();
        validate_response(&response, &questions).unwrap();
        for bad in [
            json!({"answers": {"n": null}}),
            json!({"answers": {"n": {"type": "choice", "choice": "a", "confidence": 0.8,
                "probabilities": {"a": 0.8}}}}),
            json!({"answers": {"n": {"type": "noul", "noul": 1.2}}}),
            json!({"answers": {"c": {"type": "choice", "choice": "other", "confidence": 0.8,
                "probabilities": {"other": 0.8}}}}),
            json!({"answers": {"s": {"type": "score", "score": 2.0, "confidence": 0.7,
                "legend": {"0": "low", "1": "high"}, "probabilities": {"0": 0.5}}}}),
        ] {
            let mut modified = valid.clone();
            for (key, replacement) in bad["answers"].as_object().unwrap() {
                if replacement.is_null() {
                    modified["answers"].as_object_mut().unwrap().remove(key);
                } else {
                    modified["answers"][key] = replacement.clone();
                }
            }
            let response: SystemOneResponse = serde_json::from_value(modified).unwrap();
            assert!(validate_response(&response, &questions).is_err());
        }
    }
}
