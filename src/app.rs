//! Initializes process-wide resources and serves the Nushell plugin protocol.

use nu_plugin::{MsgPackSerializer, serve_plugin};
use nu_protocol::LabeledError;

use crate::{config::process::HttpAttemptLimit, error::JevError, plugin::JevPlugin};

/// Resolves immutable startup policy, builds shared state, and serves the plugin protocol.
/// Startup NUON reads stay outside async tasks and fail before any command is served.
pub(crate) fn serve(runtime: tokio::runtime::Runtime) -> Result<(), LabeledError> {
    // Resolve once here, not in the first caller: otherwise concurrent invocation order
    // would determine a shared budget intended to apply to the whole process.
    let limit = HttpAttemptLimit::from_startup()?;
    let plugin = JevPlugin::new(runtime, limit).map_err(JevError::into_labeled)?;
    ::tracing::info!("plugin started");
    serve_plugin(&plugin, MsgPackSerializer);
    Ok(())
}
