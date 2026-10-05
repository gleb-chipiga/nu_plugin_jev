//! Builds schema-aligned questions as ordinary Nu records without network access.
//! Constructors use the simple SDK adapter; the records remain data, not custom plugin values.

use std::collections::BTreeMap;

use nu_protocol::{LabeledError, Span, Value};
use serde_json::Value as JsonValue;

use crate::{
    api::{
        types::{NoulCriteria, NoulDescriptions, Question},
        validate::validate_question,
    },
    nu::{typed::question_to_nu, value::to_json},
};

/// Constructs Choice questions from lists or named descriptions.
pub(crate) mod choice;
/// Constructs Noul questions from arbitrary supported structured descriptions.
pub(crate) mod noul;
/// Constructs Score questions from ordered level descriptions.
pub(crate) mod score;

/// Converts a description while leaving its root-shape validation to the contract layer.
fn description(value: &Value) -> Result<JsonValue, LabeledError> {
    to_json(value)
}

/// Rejects materialized pipeline data the constructor would otherwise silently ignore.
/// The simple SDK adapter maps empty input to Nothing, indistinguishable from explicit null.
fn reject_pipeline_input(input: &Value, span: Span) -> Result<(), LabeledError> {
    if input.is_nothing() {
        Ok(())
    } else {
        Err(
            LabeledError::new("jev question does not accept pipeline input")
                .with_label("pass instructions as an argument instead", span),
        )
    }
}

/// Builds and validates a Noul question, preserving omitted and null criteria.
pub(crate) fn build_noul(
    instructions: &Value,
    yes: Option<&Value>,
    no: Option<&Value>,
) -> Result<Question, LabeledError> {
    // Missing flags are None; a flag carrying Nu Nothing is Some(JSON null).
    // Preserve that distinction so constructor output matches a hand-authored API record.
    let criteria = if yes.is_some() || no.is_some() {
        Some(NoulCriteria::Descriptions(NoulDescriptions {
            yes: yes.map(description).transpose()?,
            no: no.map(description).transpose()?,
        }))
    } else {
        None
    };
    let question = Question::Noul {
        instructions: Some(description(instructions)?),
        criteria,
    };
    validate_question("match", &question, instructions.span())?;
    Ok(question)
}

/// Builds a Choice question from a list of names or a description record.
pub(crate) fn build_choice(
    instructions: &Value,
    criteria: &Value,
) -> Result<Question, LabeledError> {
    let choices = match criteria {
        Value::List { vals, .. } => vals
            .iter()
            .map(|value| match value {
                Value::String { val, .. } => Ok((val.clone(), JsonValue::Null)),
                _ => Err(
                    LabeledError::new("Choice option list must contain only strings")
                        .with_label("invalid option name", value.span()),
                ),
            })
            .collect::<Result<Vec<_>, _>>()?,
        Value::Record { val, .. } => val
            .iter()
            .map(|(name, value)| description(value).map(|description| (name.clone(), description)))
            .collect::<Result<Vec<_>, _>>()?,
        _ => {
            return Err(
                LabeledError::new("Choice criteria must be a list or record")
                    .with_label("invalid criteria", criteria.span()),
            );
        }
    };
    if !(1..=255).contains(&choices.len()) {
        return Err(
            LabeledError::new("Choice requires 1 to 255 distinct options")
                .with_label("invalid option count", criteria.span()),
        );
    }
    let mut distinct = BTreeMap::new();
    for (name, description) in choices {
        // Nu records can contain duplicate columns. Detect them before the map would collapse
        // two definitions into one option and silently change the policy sent to the service.
        if distinct.insert(name.clone(), description).is_some() {
            return Err(
                LabeledError::new(format!("duplicate Choice option {name:?}"))
                    .with_label("duplicate option", criteria.span()),
            );
        }
    }
    let question = Question::Choice {
        instructions: Some(description(instructions)?),
        criteria: distinct,
    };
    validate_question("choice", &question, instructions.span())?;
    Ok(question)
}

/// Builds a Score question from one to ten ordered level descriptions.
pub(crate) fn build_score(
    instructions: &Value,
    criteria: &Value,
) -> Result<Question, LabeledError> {
    let Value::List { vals, .. } = criteria else {
        return Err(LabeledError::new("Score criteria must be a list")
            .with_label("invalid criteria", criteria.span()));
    };
    if !(1..=10).contains(&vals.len()) {
        return Err(LabeledError::new("Score requires 1 to 10 ordered levels")
            .with_label("invalid level count", criteria.span()));
    }
    let criteria = vals
        .iter()
        .map(description)
        .collect::<Result<Vec<_>, _>>()?;
    let question = Question::Score {
        instructions: Some(description(instructions)?),
        criteria,
    };
    validate_question("score", &question, instructions.span())?;
    Ok(question)
}

#[cfg(test)]
mod tests {
    use nu_plugin_test_support::PluginTest;
    use nu_protocol::{Record, ShellError, Span, Value};
    use serde_json::json;

    use crate::{api::validate::parse_questions, nu::value::to_json, plugin::JevPlugin};

    use super::{
        build_choice, build_noul, build_score, choice::JevQuestionChoice, noul::JevQuestionNoul,
        score::JevQuestionScore,
    };

    /// Starts an isolated public plugin test engine without a caller credential.
    fn plugin_test() -> Result<PluginTest, Box<ShellError>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        let plugin = JevPlugin::new(runtime, Default::default()).unwrap();
        PluginTest::new("jev", plugin.into()).map_err(Box::new)
    }

    /// Preserves one-sided and explicitly null Noul descriptions.
    #[test]
    fn noul_optional_criteria() {
        let instructions = Value::test_string("spam?");
        let omitted = serde_json::to_value(build_noul(&instructions, None, None).unwrap()).unwrap();
        assert!(omitted.get("criteria").is_none());
        let yes = Value::test_nothing();
        let one_sided =
            serde_json::to_value(build_noul(&instructions, Some(&yes), None).unwrap()).unwrap();
        assert_eq!(one_sided["criteria"], json!({"true": null}));
    }

    /// Enforces distinct Choice names and the constructor-only maximum.
    #[test]
    fn choice_shorthand_and_boundaries() {
        let instructions = Value::test_string("kind?");
        let list = Value::test_list(vec![
            Value::test_string("normal"),
            Value::test_string("spam"),
        ]);
        let question = build_choice(&instructions, &list).unwrap();
        assert_eq!(
            serde_json::to_value(question).unwrap()["criteria"],
            json!({"normal": null, "spam": null})
        );
        let duplicate =
            Value::test_list(vec![Value::test_string("spam"), Value::test_string("spam")]);
        assert!(build_choice(&instructions, &duplicate).is_err());
        assert!(build_choice(&instructions, &Value::test_list(vec![])).is_err());
        let names = (0..255)
            .map(|index| Value::test_string(format!("c{index}")))
            .collect();
        assert!(build_choice(&instructions, &Value::test_list(names)).is_ok());
        let names = (0..256)
            .map(|index| Value::test_string(format!("c{index}")))
            .collect();
        assert!(build_choice(&instructions, &Value::test_list(names)).is_err());
        let mut record = Record::new();
        record.push("structured", Value::test_list(vec![Value::test_int(2)]));
        assert!(build_choice(&instructions, &Value::test_record(record)).is_ok());
    }

    /// Preserves Score ordering and rejects null or excessive levels.
    #[test]
    fn score_levels_and_boundaries() {
        let instructions = Value::test_string("urgency?");
        let levels = Value::test_list(vec![Value::test_string("later"), Value::test_string("now")]);
        let question = build_score(&instructions, &levels).unwrap();
        assert_eq!(
            serde_json::to_value(question).unwrap()["criteria"],
            json!(["later", "now"])
        );
        assert!(build_score(&instructions, &Value::test_list(vec![])).is_err());
        assert!(
            build_score(
                &instructions,
                &Value::test_list(vec![Value::test_nothing()])
            )
            .is_err()
        );
        let ten = Value::test_list(
            (0..10)
                .map(|index| Value::test_string(index.to_string()))
                .collect(),
        );
        assert!(build_score(&instructions, &ten).is_ok());
        let eleven = Value::test_list(
            (0..11)
                .map(|index| Value::test_string(index.to_string()))
                .collect(),
        );
        assert!(build_score(&instructions, &eleven).is_err());
    }

    /// Exercises all offline constructors through the public plugin command interface.
    #[test]
    fn constructors_work_without_credentials() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let noul = test
            .eval("jev question noul 'spam?' --yes null")?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&noul).unwrap()["criteria"], json!({"true": null}));
        let choice = test
            .eval("jev question choice 'kind?' [normal spam]")?
            .into_value(Span::test_data())?;
        assert_eq!(
            to_json(&choice).unwrap()["criteria"],
            json!({"normal": null, "spam": null})
        );
        let score = test
            .eval("jev question score 'urgency?' ['later' 'now']")?
            .into_value(Span::test_data())?;
        assert_eq!(
            to_json(&score).unwrap()["criteria"],
            json!(["later", "now"])
        );
        Ok(())
    }

    /// Checks structured arguments and null-preserving one-sided Noul output.
    #[test]
    fn constructors_preserve_structured_descriptions() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let noul = test
            .eval("jev question noul {task: 'spam', weight: 2} --no null")?
            .into_value(Span::test_data())?;
        let noul = to_json(&noul).unwrap();
        assert_eq!(noul["instructions"], json!({"task": "spam", "weight": 2}));
        assert_eq!(noul["criteria"], json!({"false": null}));

        let choice = test
            .eval("jev question choice 'kind?' {normal: null, phishing: {risk: true}}")?
            .into_value(Span::test_data())?;
        assert_eq!(
            to_json(&choice).unwrap()["criteria"],
            json!({"normal": null, "phishing": {"risk": true}})
        );

        let score = test
            .eval("jev question score {task: 'urgency'} ['later' {hours: 24}]")?
            .into_value(Span::test_data())?;
        assert_eq!(
            to_json(&score).unwrap()["criteria"],
            json!(["later", {"hours": 24}])
        );
        Ok(())
    }

    /// Ensures invalid constructor inputs become command failures, not records.
    #[test]
    fn constructors_reject_invalid_inputs() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        for source in [
            "jev question noul 42",
            "jev question choice 'kind?' [spam spam]",
            "jev question choice 'kind?' []",
            "jev question score 'urgency?' []",
            "jev question score 'urgency?' [null]",
            "'ignored' | jev question noul 'spam?'",
            "'ignored' | jev question choice 'kind?' [spam]",
            "'ignored' | jev question score 'urgency?' ['low' 'high']",
        ] {
            let failed = match test.eval(source) {
                Ok(value) => value.into_value(Span::test_data()).is_err(),
                Err(_) => true,
            };
            assert!(failed, "expected failure for {source}");
        }
        Ok(())
    }

    /// Checks constructor cardinality through Nushell argument parsing.
    #[test]
    fn constructor_command_boundaries() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let choice_255 = (0..255)
            .map(|index| format!("c{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let choice_256 = format!("{choice_255} c255");
        assert!(test.eval("jev question choice 'kind?' [only]").is_ok());
        assert!(
            test.eval(&format!("jev question choice 'kind?' [{choice_255}]"))
                .is_ok()
        );
        assert!(
            test.eval(&format!("jev question choice 'kind?' [{choice_256}]"))
                .is_err()
        );
        let score_10 = (0..10)
            .map(|index| format!("'level {index}'"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(test.eval("jev question score 'urgency?' ['only']").is_ok());
        assert!(
            test.eval(&format!("jev question score 'urgency?' [{score_10}]"))
                .is_ok()
        );
        assert!(
            test.eval(&format!(
                "jev question score 'urgency?' [{score_10} 'level 10']"
            ))
            .is_err()
        );
        Ok(())
    }

    /// Confirms constructor examples run offline and compose into a raw policy.
    #[test]
    fn composed_policy_and_help_examples_are_valid() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let spam = test
            .eval("jev question noul 'Is this spam?'")?
            .into_value(Span::test_data())?;
        let kind = test
            .eval("jev question choice 'Message kind?' [normal promo spam]")?
            .into_value(Span::test_data())?;
        let mut policy = Record::new();
        policy.push("spam", spam);
        policy.push("kind", kind);
        assert_eq!(
            parse_questions(&Value::test_record(policy)).unwrap().len(),
            2
        );
        test.test_command_examples(&JevQuestionNoul)?;
        test.test_command_examples(&JevQuestionChoice)?;
        test.test_command_examples(&JevQuestionScore)?;
        Ok(())
    }
}
