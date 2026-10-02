//! Exposes the offline Score question constructor to Nushell.

use nu_plugin::{EngineInterface, EvaluatedCall, SimplePluginCommand};
use nu_protocol::{Example, LabeledError, Signature, SyntaxShape, Value};

use crate::plugin::JevPlugin;

use super::{build_score, question_to_nu};

/// Builds an ordinary Score question record without credentials or HTTP.
pub(crate) struct JevQuestionScore;

impl SimplePluginCommand for JevQuestionScore {
    type Plugin = JevPlugin;

    /// Names the offline constructor.
    fn name(&self) -> &str {
        "jev question score"
    }

    /// Declares instructions and ordered level descriptions.
    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .required("instructions", SyntaxShape::Any, "Scoring instructions")
            .required(
                "criteria",
                SyntaxShape::List(Box::new(SyntaxShape::Any)),
                "Ordered level descriptions",
            )
    }

    /// Describes the constructor in command listings.
    fn description(&self) -> &str {
        "Build a Score question from ordered levels"
    }

    /// Clarifies ordinal positions and limits in Nu help.
    fn extra_description(&self) -> &str {
        "One to ten levels are allowed; positions correspond to 0..N-1. At least two levels are usually useful. No HTTP request is made."
    }

    /// Shows an offline rubric example.
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example {
            example: "jev question score 'How urgent?' ['later' 'today' 'now']",
            description: "Create a three-level ordered Score question",
            result: None,
        }]
    }

    /// Returns the schema-aligned question without using plugin state.
    fn run(
        &self,
        _plugin: &JevPlugin,
        _engine: &EngineInterface,
        call: &EvaluatedCall,
        _input: &Value,
    ) -> Result<Value, LabeledError> {
        let instructions: Value = call.req(0).map_err(LabeledError::from)?;
        let criteria: Value = call.req(1).map_err(LabeledError::from)?;
        question_to_nu(build_score(&instructions, &criteria)?, call.head)
    }
}
