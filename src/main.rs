//! Entry point for the TypeSafe Jev Nushell plugin.

/// Defines the TypeSafe wire request and response contracts.
#[allow(
    dead_code,
    reason = "contract layer is integrated by later command stages"
)]
mod api;
/// Constructs the runtime and serves the Nushell plugin.
mod app;
/// Groups the commands exposed to Nushell.
mod commands;
/// Resolves per-invocation settings from flags, plugin config, and caller environment.
#[allow(
    dead_code,
    reason = "configuration is consumed by later command stages"
)]
mod config;
/// Classifies redacted transport and command errors.
#[allow(
    dead_code,
    reason = "transport errors are consumed by later command stages"
)]
mod error;
/// Converts Nushell values and composes structured Jev state.
#[allow(
    dead_code,
    reason = "conversion layer is integrated by later command stages"
)]
mod nu;
/// Owns shared plugin state and command registration.
mod plugin;
/// Configures non-blocking diagnostics for the plugin process.
mod tracing;

/// Uses mimalloc when the default feature is enabled.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Initializes diagnostics before starting the async runtime and plugin.
fn main() {
    let _tracing_guard = tracing::init().expect("initialize tracing");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build Tokio runtime");
    app::serve(runtime);
}
