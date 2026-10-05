//! Classifies plugin failures without retaining credentials or response bodies.

use nu_protocol::LabeledError;
use thiserror::Error;

/// Maximum UTF-8 byte length of a user-controlled path in an error message.
const MAX_PATH_BYTES: usize = 256;

/// A field or state path escaped and bounded for diagnostics.
#[derive(Debug)]
pub(crate) struct DiagnosticPath(String);

impl DiagnosticPath {
    /// Escapes and bounds a state path without retaining its values.
    pub(crate) fn new(path: &str) -> Self {
        Self::escaped(path, false)
    }

    /// Escapes and quotes a literal field name for an error message.
    fn quoted(name: &str) -> Self {
        Self::escaped(name, true)
    }

    /// Writes at most one bounded path, reserving space for a truncation mark.
    fn escaped(raw: &str, quoted: bool) -> Self {
        let mut safe = String::with_capacity(raw.len().min(MAX_PATH_BYTES));
        if quoted {
            safe.push('"');
        }
        let limit = MAX_PATH_BYTES - '…'.len_utf8() - usize::from(quoted);
        for character in raw.chars() {
            let escaped = character.escape_debug();
            let escaped_bytes = escaped.clone().map(char::len_utf8).sum::<usize>();
            if safe.len() + escaped_bytes > limit {
                safe.push('…');
                if quoted {
                    safe.push('"');
                }
                return Self(safe);
            }
            safe.extend(escaped);
        }
        if quoted {
            safe.push('"');
        }
        Self(safe)
    }
}

impl std::fmt::Display for DiagnosticPath {
    /// Writes the already escaped, bounded path.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A classified, credential-safe failure with status only for HTTP errors.
#[derive(Debug, Error)]
pub(crate) enum JevError {
    /// Invalid invocation arguments or questions.
    #[error("{0}")]
    Validation(&'static str),
    /// Invalid outbound state with a fixed explanation.
    #[error("{0}")]
    State(&'static str),
    /// A selected literal field is absent from a row.
    #[error("selected Jev field {field} is missing")]
    MissingField { field: DiagnosticPath },
    /// A Nu value cannot be represented in a Jev JSON state.
    #[error("cannot convert {kind} at {path} to Jev JSON")]
    StateConversion {
        kind: &'static str,
        path: DiagnosticPath,
    },
    /// Existing row fields would be overwritten.
    #[error("{0}")]
    FieldCollision(&'static str),
    /// A non-success HTTP status was returned.
    #[error("Jev returned HTTP {status}")]
    Http { status: u16 },
    /// A network operation failed without an HTTP status.
    #[error("{0}")]
    Transport(&'static str),
    /// A logical evaluation exceeded its total deadline.
    #[error("{0}")]
    Timeout(&'static str),
    /// The response was malformed or violated the typed contract.
    #[error("{0}")]
    Response(&'static str),
    /// A successful response exceeded its configured byte limit.
    #[error("Jev response exceeds {limit} byte limit")]
    ResponseTooLarge { limit: usize },
    /// The local invocation was interrupted or dropped.
    #[error("Jev invocation was cancelled")]
    Cancelled,
}

impl JevError {
    /// Constructs a missing-field error without retaining an unbounded name.
    pub(crate) fn missing_field(name: &str) -> Self {
        Self::MissingField {
            field: DiagnosticPath::quoted(name),
        }
    }

    /// Returns the stable category used by Nu error codes and row records.
    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            Self::Validation(_) => "validation",
            Self::State(_) | Self::MissingField { .. } | Self::StateConversion { .. } => "state",
            Self::FieldCollision(_) => "field_collision",
            Self::Http { .. } => "http",
            Self::Transport(_) => "transport",
            Self::Timeout(_) => "timeout",
            Self::Response(_) | Self::ResponseTooLarge { .. } => "response",
            Self::Cancelled => "cancelled",
        }
    }

    /// Returns the HTTP status only when a response failed by status.
    pub(crate) fn status(&self) -> Option<u16> {
        match self {
            Self::Http { status } => Some(*status),
            _ => None,
        }
    }

    /// Projects a redacted failure to Nu's serializable diagnostic with a stable category code.
    /// Leaves source labels to boundary code, which knows the command or input value span.
    pub(crate) fn to_labeled(&self) -> LabeledError {
        let code = match self {
            Self::Validation(_) => "jev::validation",
            Self::State(_) | Self::MissingField { .. } | Self::StateConversion { .. } => {
                "jev::state"
            }
            Self::FieldCollision(_) => "jev::field_collision",
            Self::Http { .. } => "jev::http",
            Self::Transport(_) => "jev::transport",
            Self::Timeout(_) => "jev::timeout",
            Self::Response(_) | Self::ResponseTooLarge { .. } => "jev::response",
            Self::Cancelled => "jev::cancelled",
        };
        LabeledError::new(self.to_string()).with_code(code)
    }

    /// Converts an owned failure to a native Nu error.
    pub(crate) fn into_labeled(self) -> LabeledError {
        self.to_labeled()
    }
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticPath, JevError, MAX_PATH_BYTES};

    /// Maps each typed variant to the existing public category and Nu code.
    #[test]
    fn maps_redacted_error_categories() {
        let cases = [
            (JevError::Validation("invalid"), "validation", None),
            (JevError::State("invalid"), "state", None),
            (JevError::missing_field("name"), "state", None),
            (
                JevError::StateConversion {
                    kind: "binary",
                    path: DiagnosticPath::new("$.name"),
                },
                "state",
                None,
            ),
            (
                JevError::FieldCollision("occupied"),
                "field_collision",
                None,
            ),
            (JevError::Http { status: 503 }, "http", Some(503)),
            (JevError::Transport("failed"), "transport", None),
            (JevError::Timeout("expired"), "timeout", None),
            (JevError::Response("invalid"), "response", None),
            (JevError::ResponseTooLarge { limit: 64 }, "response", None),
            (JevError::Cancelled, "cancelled", None),
        ];
        for (error, kind, status) in cases {
            assert_eq!(error.kind_name(), kind);
            assert_eq!(error.status(), status);
            assert_eq!(
                error.to_labeled().code.as_deref(),
                Some(format!("jev::{kind}").as_str())
            );
        }
        assert_eq!(
            JevError::Http { status: 503 }.to_string(),
            "Jev returned HTTP 503"
        );
    }

    /// Bounds unusual field names without splitting a UTF-8 character.
    #[test]
    fn bounds_and_escapes_paths() {
        let path = DiagnosticPath::new(&format!("{}\n{}", "é".repeat(200), "secret"));
        assert!(path.0.len() <= MAX_PATH_BYTES);
        assert!(path.0.ends_with('…'));
        assert!(!path.0.contains("secret"));
        assert_eq!(DiagnosticPath::new("$.a\nb").0, "$.a\\nb");
        let boundary = DiagnosticPath::new(&format!("{}\n", "a".repeat(252)));
        assert_eq!(boundary.0, format!("{}…", "a".repeat(252)));
        assert_eq!(
            JevError::missing_field("ab\ncd").to_string(),
            "selected Jev field \"ab\\ncd\" is missing"
        );
    }
}
