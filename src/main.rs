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
/// Resolves invocation settings and immutable startup resource policy.
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
/// Hosts the ready-made HTTP/2 fixture shared by network tests.
#[cfg(test)]
#[path = "../tests/support/h2_fixture.rs"]
mod h2_fixture;
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
fn main() -> std::process::ExitCode {
    let _tracing_guard = tracing::init().expect("initialize tracing");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build Tokio runtime");
    match app::serve(runtime) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
