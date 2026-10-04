//! Provides offline namespace guidance for installed Nushell users.

use nu_plugin::{EngineInterface, EvaluatedCall, SimplePluginCommand};
use nu_protocol::{LabeledError, Signature, Type, Value};

use crate::plugin::JevPlugin;

/// Shows the available Jev workflows without making a network request.
pub(crate) struct Jev;

impl SimplePluginCommand for Jev {
    type Plugin = JevPlugin;

    /// Returns the Nu namespace entry command.
    fn name(&self) -> &str {
        "jev"
    }

    /// Declares an argument-free offline command.
    fn signature(&self) -> Signature {
        Signature::build(self.name()).input_output_type(Type::Nothing, Type::String)
    }

    /// Summarizes the plugin in command listings.
    fn description(&self) -> &str {
        "Use TypeSafe Jev to make typed decisions about Nushell values"
    }

    /// Explains the main command workflows in Nu help.
    fn extra_description(&self) -> &str {
        concat!(
            "This guidance command accepts no pipeline input. ",
            "Build questions with `jev question`, evaluate one state with `jev ask`, ",
            "annotate rows with `jev annotate`, or list current models with `jev models`; ",
            "use native Nu commands for filtering and sorting. Settings resolve per call ",
            "from flags, Nu config, caller environment, local TOML, user TOML, then defaults. ",
            "Use --config or NU_PLUGIN_JEV_CONFIG to select a local file. ",
            "Automatic proxy discovery is captured at plugin startup; restart with ",
            "`plugin stop jev` after ordinary proxy changes. Explicit HTTP/SOCKS5h proxies ",
            "ignore global NO_PROXY and never fall back to direct routing."
        )
    }

    /// Returns usage guidance without reading credentials or sending HTTP.
    fn run(
        &self,
        _plugin: &JevPlugin,
        _engine: &EngineInterface,
        call: &EvaluatedCall,
        input: &Value,
    ) -> Result<Value, LabeledError> {
        if !input.is_nothing() {
            return Err(LabeledError::new("jev does not accept pipeline input")
                .with_label("invoke jev without an input state", call.head));
        }
        Ok(Value::string(
            concat!(
                "Use `jev ask` for one state, `jev question` to build questions, ",
                "`jev annotate` for tables, or `jev models` to list current models. ",
                "Filter and sort with native Nu commands. See `help jev`."
            ),
            call.head,
        ))
    }
}
