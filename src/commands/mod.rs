//! Exposes SDK command adapters; argument evaluation and transport remain Nushell's job.
//! Simple commands receive materialized values; live commands inspect `PipelineData` directly.

/// Streams independent table-row evaluations and typed annotations.
pub(crate) mod annotate;
/// Evaluates one finite state against named questions.
pub(crate) mod ask;
/// Shares request construction and live evaluation for `jev ask`.
pub(crate) mod evaluate;
/// Lists currently available service models without accepting pipeline input.
pub(crate) mod models;
/// Builds API question records without contacting the service.
pub(crate) mod question;
/// Provides offline guidance from the `jev` root command.
pub(crate) mod root;
#[cfg(test)]
/// Supplies an isolated local HTTP fixture for command-level tests.
pub(crate) mod tests;
