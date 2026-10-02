//! Adapts Nushell values to the structured JSON expected by Jev.

/// Retains successful per-invocation evaluations under bounded LRU limits.
pub(crate) mod cache;
/// Builds and validates the JSON state sent to Jev.
pub(crate) mod state;
/// Schedules bounded row evaluations and streams outcomes back to Nushell.
pub(crate) mod stream;
/// Converts supported Nu values without stringifying structured data.
pub(crate) mod value;
