//! Classifies plugin failures without retaining credentials or request bodies.

use nu_protocol::LabeledError;
use thiserror::Error;

/// Names the stable user-facing category of a Jev failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    /// Invalid invocation arguments or questions.
    Validation,
    /// Invalid outbound state.
    State,
    /// Existing row fields would be overwritten.
    FieldCollision,
    /// A non-success HTTP status was returned.
    Http,
    /// A network operation failed without an HTTP status.
    Transport,
    /// A logical evaluation exceeded its total deadline.
    Timeout,
    /// The response was malformed or violated the typed contract.
    Response,
    /// The local invocation was interrupted or dropped.
    Cancelled,
}

impl ErrorKind {
    /// Returns the stable code fragment used by Nu errors and row records.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::State => "state",
            Self::FieldCollision => "field_collision",
            Self::Http => "http",
            Self::Transport => "transport",
            Self::Timeout => "timeout",
            Self::Response => "response",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Carries only bounded, credential-safe diagnostics and an optional status.
#[derive(Debug, Error)]
#[error("{message}")]
pub(crate) struct JevError {
    /// Stable category for downstream row-error records.
    pub(crate) kind: ErrorKind,
    /// Short redacted explanation of the failure.
    pub(crate) message: String,
    /// HTTP status when one exists.
    pub(crate) status: Option<u16>,
}

impl JevError {
    /// Constructs a redacted error without an HTTP status.
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            status: None,
        }
    }

    /// Constructs an HTTP failure while retaining its numeric status.
    pub(crate) fn http(status: u16) -> Self {
        Self {
            kind: ErrorKind::Http,
            message: format!("Jev returned HTTP {status}"),
            status: Some(status),
        }
    }

    /// Maps the classified failure to a standard plugin error.
    pub(crate) fn into_labeled(self) -> LabeledError {
        LabeledError::new(self.message).with_code(format!("jev::{}", self.kind.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::{ErrorKind, JevError};

    /// Keeps stable Nu codes and statuses without incorporating server bodies.
    #[test]
    fn maps_redacted_error_categories() {
        let http = JevError::http(503);
        assert_eq!(http.kind, ErrorKind::Http);
        assert_eq!(http.status, Some(503));
        assert_eq!(http.into_labeled().code.as_deref(), Some("jev::http"));
        let timeout = JevError::new(ErrorKind::Timeout, "deadline expired");
        assert_eq!(timeout.status, None);
        assert_eq!(timeout.into_labeled().code.as_deref(), Some("jev::timeout"));
    }
}
