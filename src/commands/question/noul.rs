//! Exposes the offline Noul question constructor to Nushell.

use indoc::indoc;
use nu_plugin::{EngineInterface, EvaluatedCall, SimplePluginCommand};
use nu_protocol::{Example, LabeledError, Signature, SyntaxShape, Type, Value};

use crate::plugin::JevPlugin;

use super::{build_noul, question_to_nu};

/// Builds an ordinary Noul question record without credentials or HTTP.
pub(crate) struct JevQuestionNoul;

impl SimplePluginCommand for JevQuestionNoul {
    type Plugin = JevPlugin;

    /// Names the offline constructor.
    fn name(&self) -> &str {
        "jev question noul"
    }

    /// Declares structured instructions and optional true/false descriptions.
    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_type(Type::Nothing, Type::record())
            .required(
                "instructions",
                SyntaxShape::Any,
                "The condition or decision instructions",
            )
            .named(
                "yes",
                SyntaxShape::Any,
                "Description of the true case",
                None,
            )
            .named(
                "no",
                SyntaxShape::Any,
                "Description of the false case",
                None,
            )
    }

    /// Describes the constructor in command listings.
    fn description(&self) -> &str {
        "Build a Noul probability question as a Nu record"
    }

    /// Clarifies optional-field behavior in Nu help.
    fn extra_description(&self) -> &str {
        indoc! {"
            No network request is made and pipeline input is not accepted. Missing --yes/--no \
            fields are omitted; an explicitly supplied null remains null.\
        "}
    }

    /// Shows an offline policy-building example.
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example {
            example: "jev question noul 'Is this spam?' --yes 'Unsolicited message'",
            description: "Create a Noul question with only a true-case description",
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
        let yes = call.get_flag_value("yes");
        let no = call.get_flag_value("no");
        question_to_nu(
            build_noul(&instructions, yes.as_ref(), no.as_ref())?,
            call.head,
        )
    }
}
