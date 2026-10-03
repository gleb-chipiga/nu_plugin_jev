//! Configures non-blocking process diagnostics and traces logical API operations.

use std::{future::Future, time::Instant};

use ::tracing::Instrument;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::{
    api::types::{ModelMetadataList, SystemOneResponse},
    error::{ErrorKind, JevError},
};

/// Sends diagnostics to stderr without blocking async request workers.
pub(crate) fn init() -> Result<WorkerGuard, Box<dyn std::error::Error + Send + Sync>> {
    let (writer, guard) = tracing_appender::non_blocking(std::io::stderr());
    let level = std::env::var("NU_PLUGIN_JEV_LOG")
        .ok()
        .filter(|value| {
            matches!(
                value.as_str(),
                "off" | "error" | "warn" | "info" | "debug" | "trace"
            )
        })
        .unwrap_or_else(|| "warn".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(format!("nu_plugin_jev={level}")))
        .with_writer(writer)
        .with_ansi(false)
        .try_init()?;
    Ok(guard)
}

/// Traces one catalog lookup without recording credentials or model contents.
pub(crate) async fn trace_models<F>(
    request_id: &str,
    operation: F,
) -> Result<ModelMetadataList, JevError>
where
    F: Future<Output = Result<ModelMetadataList, JevError>>,
{
    let span = ::tracing::warn_span!("jev_models", request_id);
    async move {
        let started = Instant::now();
        ::tracing::info!("model listing started");
        let result = operation.await;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &result {
            Ok(list) => ::tracing::info!(
                duration_ms,
                count = list.models.len(),
                "model listing completed"
            ),
            Err(error) if error.kind == ErrorKind::Cancelled => {
                ::tracing::debug!(duration_ms, "model listing cancelled");
            }
            Err(error) => ::tracing::warn!(
                duration_ms,
                kind = error.kind.as_str(),
                status = ?error.status,
                "model listing failed"
            ),
        }
        result
    }
    .instrument(span)
    .await
}

/// Traces one logical evaluation without recording credentials or request content.
pub(crate) async fn trace_evaluation<F>(
    command: &'static str,
    request_id: &str,
    operation: F,
) -> Result<SystemOneResponse, JevError>
where
    F: Future<Output = Result<SystemOneResponse, JevError>>,
{
    let span = ::tracing::warn_span!("jev_evaluation", command, request_id);
    async move {
        let started = Instant::now();
        ::tracing::info!("evaluation started");
        let result = operation.await;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &result {
            Ok(response) => ::tracing::info!(
                duration_ms,
                model = %response.model,
                input_tokens = response.usage.input_tokens,
                output_tokens = response.usage.output_tokens,
                "evaluation completed"
            ),
            Err(error) if error.kind == ErrorKind::Cancelled => {
                ::tracing::debug!(duration_ms, "evaluation cancelled");
            }
            Err(error) => ::tracing::warn!(
                duration_ms,
                kind = error.kind.as_str(),
                status = ?error.status,
                "evaluation failed"
            ),
        }
        result
    }
    .instrument(span)
    .await
}
