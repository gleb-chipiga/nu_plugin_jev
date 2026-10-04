//! Converts fixed TypeSafe API shapes directly to Nushell records.

use std::collections::BTreeMap;

use nu_protocol::{LabeledError, Record, Span, Value};
use serde_json::Value as JsonValue;

use crate::api::types::{
    Answer, NoulCriteria, NoulDescriptions, Question, SystemOneRequest, Usage,
};

use super::value::{from_json, from_json_ref};

/// Projects a typed outbound request while preserving arbitrary JSON state.
pub(crate) fn request_to_nu(request: SystemOneRequest, span: Span) -> Result<Value, LabeledError> {
    let SystemOneRequest {
        state,
        model,
        questions,
    } = request;
    let mut record = Record::with_capacity(3);
    record.push("model", Value::string(model, span));
    let mut question_record = Record::with_capacity(questions.len());
    for (name, question) in questions.iter() {
        question_record.push(name.clone(), question_to_nu(question.clone(), span)?);
    }
    record.push("questions", Value::record(question_record, span));
    record.push("state", from_json(state, span)?);
    Ok(Value::record(record, span))
}

/// Projects one typed question without serializing its fixed fields to JSON.
pub(crate) fn question_to_nu(question: Question, span: Span) -> Result<Value, LabeledError> {
    let record = match question {
        Question::Noul {
            instructions,
            criteria,
        } => {
            let mut record = Record::with_capacity(
                1 + usize::from(criteria.is_some()) + usize::from(instructions.is_some()),
            );
            if let Some(criteria) = criteria {
                record.push("criteria", noul_criteria_to_nu(criteria, span)?);
            }
            if let Some(instructions) = instructions {
                record.push("instructions", from_json(instructions, span)?);
            }
            record.push("type", Value::string("noul", span));
            record
        }
        Question::Choice {
            instructions,
            criteria,
        } => {
            let mut record = Record::with_capacity(2 + usize::from(instructions.is_some()));
            record.push("criteria", structured_map_to_nu(criteria, span)?);
            if let Some(instructions) = instructions {
                record.push("instructions", from_json(instructions, span)?);
            }
            record.push("type", Value::string("choice", span));
            record
        }
        Question::Score {
            instructions,
            criteria,
        } => {
            let mut record = Record::with_capacity(2 + usize::from(instructions.is_some()));
            let criteria = criteria
                .into_iter()
                .map(|value| from_json(value, span))
                .collect::<Result<Vec<_>, _>>()?;
            record.push("criteria", Value::list(criteria, span));
            if let Some(instructions) = instructions {
                record.push("instructions", from_json(instructions, span)?);
            }
            record.push("type", Value::string("score", span));
            record
        }
    };
    Ok(Value::record(record, span))
}

/// Projects explicit Noul criteria null or its independently optional descriptions.
fn noul_criteria_to_nu(criteria: NoulCriteria, span: Span) -> Result<Value, LabeledError> {
    match criteria {
        NoulCriteria::Null => Ok(Value::nothing(span)),
        NoulCriteria::Descriptions(NoulDescriptions { yes, no }) => {
            let mut record =
                Record::with_capacity(usize::from(yes.is_some()) + usize::from(no.is_some()));
            if let Some(no) = no {
                record.push("false", from_json(no, span)?);
            }
            if let Some(yes) = yes {
                record.push("true", from_json(yes, span)?);
            }
            Ok(Value::record(record, span))
        }
    }
}

/// Projects ordered JSON descriptions to a native Nu record.
fn structured_map_to_nu(
    values: BTreeMap<String, JsonValue>,
    span: Span,
) -> Result<Value, LabeledError> {
    let mut record = Record::with_capacity(values.len());
    for (key, value) in values {
        record.push(key, from_json(value, span)?);
    }
    Ok(Value::record(record, span))
}

/// Projects all validated answer variants without a whole-map JSON copy.
pub(crate) fn answers_to_nu(
    answers: &BTreeMap<String, Answer>,
    span: Span,
) -> Result<Value, LabeledError> {
    let mut record = Record::with_capacity(answers.len());
    for (name, answer) in answers {
        record.push(name.clone(), answer_to_nu(answer, span)?);
    }
    Ok(Value::record(record, span))
}

/// Projects one validated answer and preserves arbitrary Score legend values.
fn answer_to_nu(answer: &Answer, span: Span) -> Result<Value, LabeledError> {
    let record = match answer {
        Answer::Noul { noul } => {
            let mut record = Record::with_capacity(2);
            record.push("noul", Value::float(*noul, span));
            record.push("type", Value::string("noul", span));
            record
        }
        Answer::Choice {
            choice,
            confidence,
            probabilities,
        } => {
            let mut record = Record::with_capacity(4);
            record.push("choice", Value::string(choice, span));
            record.push("confidence", Value::float(*confidence, span));
            record.push("probabilities", probabilities_to_nu(probabilities, span));
            record.push("type", Value::string("choice", span));
            record
        }
        Answer::Score {
            score,
            confidence,
            legend,
            probabilities,
        } => {
            let mut record = Record::with_capacity(5);
            record.push("confidence", Value::float(*confidence, span));
            let mut levels = Record::with_capacity(legend.len());
            for (level, description) in legend {
                levels.push(level.clone(), from_json_ref(description, span)?);
            }
            record.push("legend", Value::record(levels, span));
            record.push("probabilities", probabilities_to_nu(probabilities, span));
            record.push("score", Value::float(*score, span));
            record.push("type", Value::string("score", span));
            record
        }
    };
    Ok(Value::record(record, span))
}

/// Projects a validated probability map without JSON serialization.
fn probabilities_to_nu(probabilities: &BTreeMap<String, f64>, span: Span) -> Value {
    let mut record = Record::with_capacity(probabilities.len());
    for (name, probability) in probabilities {
        record.push(name.clone(), Value::float(*probability, span));
    }
    Value::record(record, span)
}

/// Projects token counts while rejecting values outside Nu's integer domain.
pub(crate) fn usage_to_nu(usage: &Usage, span: Span) -> Result<Value, LabeledError> {
    let Usage {
        input_tokens,
        output_tokens,
    } = usage;
    let mut record = Record::with_capacity(2);
    record.push("input_tokens", unsigned_to_nu(*input_tokens, span)?);
    record.push("output_tokens", unsigned_to_nu(*output_tokens, span)?);
    Ok(Value::record(record, span))
}

/// Converts one API unsigned integer with the same range check as JSON conversion.
fn unsigned_to_nu(value: u64, span: Span) -> Result<Value, LabeledError> {
    i64::try_from(value).map_or_else(
        |_| {
            Err(LabeledError::new(format!(
                "JSON integer {value} exceeds Nushell's signed 64-bit range"
            ))
            .with_label("out-of-range API number", span))
        },
        |value| Ok(Value::int(value, span)),
    )
}

#[cfg(test)]
mod tests {
    use nu_protocol::Span;
    use serde_json::{Value as JsonValue, json};

    use crate::{
        api::types::{Question, SystemOneRequest, SystemOneResponse},
        nu::value::to_json,
    };

    use super::{answers_to_nu, question_to_nu, request_to_nu, usage_to_nu};

    /// Keeps omitted fields, explicit null, and nested descriptions identical to Serde.
    #[test]
    fn questions_match_json_contract() {
        for fixture in [
            json!({"type": "noul", "instructions": null, "criteria": {"true": null}}),
            json!({"type": "noul", "criteria": null}),
            json!({"type": "choice", "criteria": {"normal": null, "spam": {"tags": [1, true]}}}),
            json!({"type": "score", "instructions": ["task"], "criteria": ["low", {"high": null}]}),
        ] {
            let question: Question = serde_json::from_value(fixture.clone()).unwrap();
            let actual = to_json(&question_to_nu(question, Span::test_data()).unwrap()).unwrap();
            assert_eq!(actual, fixture);
        }
    }

    /// Keeps previews equal to the exact typed request payload.
    #[test]
    fn request_matches_json_contract() {
        let fixture = json!({"model": "jev-latest", "state": {"items": [1, null]},
            "questions": {"q": {"type": "noul", "instructions": null}}});
        let request: SystemOneRequest = serde_json::from_value(fixture.clone()).unwrap();
        let actual = to_json(&request_to_nu(request, Span::test_data()).unwrap()).unwrap();
        assert_eq!(actual, fixture);
    }

    /// Keeps all answer variants and usage equal to the validated API response.
    #[test]
    fn answers_and_usage_match_json_contract() {
        let fixture = json!({"model": "jev-fixed", "answers": {
            "n": {"type": "noul", "noul": 0.8},
            "c": {"type": "choice", "choice": "spam", "confidence": 0.9,
                "probabilities": {"spam": 0.9, "normal": 0.1}},
            "s": {"type": "score", "score": 0.6, "confidence": 0.7,
                "legend": {"0": {"name": "later"}, "1": ["now", null]},
                "probabilities": {"0": 0.4, "1": 0.6}}
        }, "usage": {"input_tokens": 12, "output_tokens": 3}});
        let response: SystemOneResponse = serde_json::from_value(fixture.clone()).unwrap();
        let answers =
            to_json(&answers_to_nu(&response.answers, Span::test_data()).unwrap()).unwrap();
        let usage = to_json(&usage_to_nu(&response.usage, Span::test_data()).unwrap()).unwrap();
        assert_eq!(answers, fixture["answers"]);
        assert_eq!(usage, fixture["usage"]);
    }

    /// Rejects token counts that JSON can hold but Nushell cannot represent.
    #[test]
    fn usage_rejects_out_of_range_integer() {
        let usage =
            serde_json::from_value(json!({"input_tokens": u64::MAX, "output_tokens": 1})).unwrap();
        assert!(usage_to_nu(&usage, Span::test_data()).is_err());
        let nested: JsonValue = json!({"value": u64::MAX});
        assert!(super::from_json_ref(&nested, Span::test_data()).is_err());
    }
}
