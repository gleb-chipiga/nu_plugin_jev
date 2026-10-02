//! Defines the TypeSafe System One and model-discovery wire contracts.

/// Supplies cancellation signals shared by HTTP and stream lifetimes.
pub(crate) mod cancel;
/// Sends authenticated System One and model-discovery requests.
pub(crate) mod client;
/// Contains serializable request and response types for the HTTP API.
pub(crate) mod types;
/// Validates submitted questions and returned answer contracts.
pub(crate) mod validate;
