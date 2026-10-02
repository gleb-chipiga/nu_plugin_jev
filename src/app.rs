use nu_plugin::{MsgPackSerializer, serve_plugin};

use crate::plugin::JevPlugin;

/// Builds the shared plugin state and serves commands over the plugin protocol.
pub(crate) fn serve(runtime: tokio::runtime::Runtime) {
    let plugin = JevPlugin::new(runtime);
    ::tracing::info!("plugin started");
    serve_plugin(&plugin, MsgPackSerializer);
}
