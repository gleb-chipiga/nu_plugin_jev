//! Adapts Nushell values to the structured JSON expected by Jev.

/// Retains successful per-invocation evaluations under bounded LRU limits.
pub(crate) mod cache;
/// Registers and retains race-free cancellation for Nushell operations and streams.
pub(crate) mod signals;
/// Builds and validates the JSON state sent to Jev.
pub(crate) mod state;
/// Schedules bounded row evaluations and streams outcomes back to Nushell.
pub(crate) mod stream;
/// Projects typed API requests and answers into ordinary Nushell values.
pub(crate) mod typed;
/// Converts supported Nu values without stringifying structured data.
pub(crate) mod value;
