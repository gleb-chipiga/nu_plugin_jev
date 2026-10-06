//! Shares runtime and HTTP resources across the SDK's concurrent synchronous command handlers.
//! Commands hold caller settings locally, so reusing this plugin never selects a global caller.

use std::sync::Arc;

use nu_plugin::{Plugin, PluginCommand};

use crate::api::client::JevClientPool;
use crate::commands::{
    annotate::JevAnnotate,
    ask::JevAsk,
    models::JevModels,
    question::{choice::JevQuestionChoice, noul::JevQuestionNoul, score::JevQuestionScore},
    root::Jev,
};
use crate::config::process::HttpAttemptLimit;
use crate::error::JevError;

/// Holds reusable resources shared by all plugin command invocations.
/// Nu dispatches concurrent handlers; request settings and cancellation stay with each caller.
pub(crate) struct JevPlugin {
    /// Reuses policy-specific connection pools under one immutable process attempt budget.
    pub(crate) client: JevClientPool,
    /// Serves async work for every caller without serializing synchronous command handlers.
    pub(crate) runtime: Arc<tokio::runtime::Runtime>,
}

impl JevPlugin {
    /// Builds the startup automatic pool and retains the process runtime.
    pub(crate) fn new(
        runtime: tokio::runtime::Runtime,
        limit: HttpAttemptLimit,
    ) -> Result<Self, JevError> {
        let client = JevClientPool::new(limit)?;
        Ok(Self {
            client,
            runtime: Arc::new(runtime),
        })
    }
}

impl Plugin for JevPlugin {
    /// Reports the package version to Nushell.
    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").into()
    }

    /// Registers reusable command objects whose invocation state lives only in `run`.
    /// The SDK adapts `SimplePluginCommand` constructors to this same command registry.
    fn commands(&self) -> Vec<Box<dyn PluginCommand<Plugin = Self>>> {
        // The SDK may call the same Sync command object from several handler threads.
        // These unit structs contain no current call, key, model, or output-stream state.
        vec![
            Box::new(Jev),
            Box::new(JevAsk),
            Box::new(JevAnnotate),
            Box::new(JevModels),
            Box::new(JevQuestionNoul),
            Box::new(JevQuestionChoice),
            Box::new(JevQuestionScore),
        ]
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;
    use nu_plugin::{PluginCommand, SimplePluginCommand};
    use nu_plugin_test_support::PluginTest;
    use nu_protocol::{ShellError, Span, Type};

    use super::{
        Jev, JevAnnotate, JevAsk, JevModels, JevPlugin, JevQuestionChoice, JevQuestionNoul,
        JevQuestionScore,
    };

    /// Declares stable native output shapes without claiming fixed annotation columns.
    #[test]
    fn commands_declare_nushell_input_output_types() {
        assert_eq!(
            SimplePluginCommand::signature(&Jev).get_input_type(),
            Type::Nothing
        );
        assert_eq!(
            SimplePluginCommand::signature(&Jev).get_output_type(None),
            Some(Type::String)
        );
        assert_eq!(
            JevAsk.signature().get_output_type(None),
            Some(Type::record())
        );
        assert_eq!(JevAsk.signature().get_input_type(), Type::Any);
        assert_eq!(
            JevAnnotate.signature().get_output_type(None),
            Some(Type::list(Type::Any))
        );
        assert_eq!(JevAnnotate.signature().get_input_type(), Type::Any);
        assert_eq!(JevModels.signature().get_input_type(), Type::Nothing);
        assert_eq!(
            JevModels.signature().get_output_type(None),
            Some(Type::record())
        );
        for signature in [
            SimplePluginCommand::signature(&JevQuestionNoul),
            SimplePluginCommand::signature(&JevQuestionChoice),
            SimplePluginCommand::signature(&JevQuestionScore),
        ] {
            assert_eq!(signature.get_input_type(), Type::Nothing);
            assert_eq!(signature.get_output_type(None), Some(Type::record()));
        }
    }

    /// Confirms root guidance runs in the public test engine without credentials.
    #[test]
    fn root_command_is_offline() -> Result<(), Box<ShellError>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        let plugin = JevPlugin::new(runtime, Default::default()).unwrap();
        let mut test = PluginTest::new("jev", plugin.into())?;
        let result = test.eval("jev")?.into_value(Span::test_data())?;
        assert!(result.as_str()?.contains("jev ask"));
        assert!(
            test.eval(indoc! {r#"
                'ignored'
                | jev
            "#})
                .is_err()
        );
        Ok(())
    }
}
