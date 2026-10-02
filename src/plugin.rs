//! Owns the reusable runtime and policy-specific HTTP client pools.

use std::sync::Arc;

use nu_plugin::{Plugin, PluginCommand};

use crate::api::client::JevClientPool;
use crate::commands::{
    annotate::JevAnnotate,
    ask::JevAsk,
    question::{choice::JevQuestionChoice, noul::JevQuestionNoul, score::JevQuestionScore},
    root::Jev,
};

/// Holds reusable resources shared by all plugin command invocations.
pub(crate) struct JevPlugin {
    pub(crate) client: JevClientPool,
    pub(crate) runtime: Arc<tokio::runtime::Runtime>,
}

impl JevPlugin {
    /// Builds the startup automatic pool and retains the process runtime.
    pub(crate) fn new(runtime: tokio::runtime::Runtime) -> Self {
        let client = JevClientPool::new().expect("build HTTP client");
        Self {
            client,
            runtime: Arc::new(runtime),
        }
    }
}

impl Plugin for JevPlugin {
    /// Reports the package version to Nushell.
    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").into()
    }

    /// Registers the currently implemented commands.
    fn commands(&self) -> Vec<Box<dyn PluginCommand<Plugin = Self>>> {
        vec![
            Box::new(Jev),
            Box::new(JevAsk),
            Box::new(JevAnnotate),
            Box::new(JevQuestionNoul),
            Box::new(JevQuestionChoice),
            Box::new(JevQuestionScore),
        ]
    }
}

#[cfg(test)]
mod tests {
    use nu_plugin_test_support::PluginTest;
    use nu_protocol::{ShellError, Span};

    use super::JevPlugin;

    /// Confirms root guidance runs in the public test engine without credentials.
    #[test]
    fn root_command_is_offline() -> Result<(), Box<ShellError>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        let mut test = PluginTest::new("jev", JevPlugin::new(runtime).into())?;
        let result = test.eval("jev")?.into_value(Span::test_data())?;
        assert!(result.as_str()?.contains("jev ask"));
        Ok(())
    }
}
