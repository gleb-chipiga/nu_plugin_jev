//! Provides offline namespace guidance through the SDK's materialized-value adapter.

use indoc::indoc;
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
        indoc! {"
            This guidance command accepts no pipeline input. Build questions with `jev \
            question`, evaluate one state with `jev ask`, annotate rows with `jev annotate`, \
            or list current models with `jev models`; use native Nu commands for filtering and \
            sorting. Invocation settings resolve per call from flags, Nu config, caller \
            environment, local NUON, user NUON, then defaults. Use --config or \
            NU_PLUGIN_JEV_CONFIG to select a local file. Concurrent calls share a startup-only \
            HTTP attempt limit: NU_PLUGIN_JEV_MAX_IN_FLIGHT, local NUON max_in_flight, user \
            NUON, then 128. Startup local selection uses process NU_PLUGIN_JEV_CONFIG or \
            startup Nu PWD; restart to change the limit. Later caller settings cannot resize \
            it. This does not change the annotation --jobs default of 16. Automatic proxy \
            discovery is captured at plugin startup; restart with `plugin stop jev` after \
            ordinary proxy changes. Explicit HTTP/SOCKS5h proxies ignore global NO_PROXY and \
            never fall back to direct routing.\
        "}
    }

    /// Returns usage guidance without reading credentials or sending HTTP.
    fn run(
        &self,
        _plugin: &JevPlugin,
        _engine: &EngineInterface,
        call: &EvaluatedCall,
        input: &Value,
    ) -> Result<Value, LabeledError> {
        // The simple adapter has already collected any incoming stream. Nothing represents
        // both absent input and explicit null; other supplied values must not be ignored.
        if !input.is_nothing() {
            return Err(LabeledError::new("jev does not accept pipeline input")
                .with_label("invoke jev without an input state", call.head));
        }
        Ok(Value::string(
            indoc! {"
                Use `jev ask` for one state, `jev question` to build questions, `jev annotate` \
                for tables, or `jev models` to list current models. Filter and sort with \
                native Nu commands. See `help jev`.\
            "},
            call.head,
        ))
    }
}
