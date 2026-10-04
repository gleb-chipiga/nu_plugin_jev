//! Exposes the offline Choice question constructor to Nushell.

use nu_plugin::{EngineInterface, EvaluatedCall, SimplePluginCommand};
use nu_protocol::{Example, LabeledError, Signature, SyntaxShape, Type, Value};

use crate::plugin::JevPlugin;

use super::{build_choice, question_to_nu};

/// Builds an ordinary Choice question record without credentials or HTTP.
pub(crate) struct JevQuestionChoice;

impl SimplePluginCommand for JevQuestionChoice {
    type Plugin = JevPlugin;

    /// Names the offline constructor.
    fn name(&self) -> &str {
        "jev question choice"
    }

    /// Declares instructions and list-or-record criteria.
    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_type(Type::Nothing, Type::record())
            .required("instructions", SyntaxShape::Any, "Decision instructions")
            .required("criteria", SyntaxShape::Any, "Option names or descriptions")
    }

    /// Describes the constructor in command listings.
    fn description(&self) -> &str {
        "Build a Choice question from a list or record"
    }

    /// Explains shorthand and limits in Nu help.
    fn extra_description(&self) -> &str {
        concat!(
            "A list of 1 to 255 distinct strings becomes option names with null descriptions. ",
            "A record supplies explicit descriptions. No HTTP request is made and pipeline ",
            "input is not accepted."
        )
    }

    /// Shows an offline list-shorthand example.
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example {
            example: "jev question choice 'Message kind?' [normal promo spam]",
            description: "Create a Choice question from option names",
            result: None,
        }]
    }

    /// Returns the schema-aligned question without using plugin state.
    fn run(
        &self,
        _plugin: &JevPlugin,
        _engine: &EngineInterface,
        call: &EvaluatedCall,
        input: &Value,
    ) -> Result<Value, LabeledError> {
        super::reject_pipeline_input(input, call.head)?;
        let instructions: Value = call.req(0).map_err(LabeledError::from)?;
        let criteria: Value = call.req(1).map_err(LabeledError::from)?;
        question_to_nu(build_choice(&instructions, &criteria)?, call.head)
    }
}
