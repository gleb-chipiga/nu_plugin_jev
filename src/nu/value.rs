//! Converts arbitrary request data between native Nu values and the REST JSON domain.
//! Conversion is not a lossless Nu round trip: special outbound types have fixed representations.
//! The plugin's MsgPack transport serializes the resulting Nu values, not this JSON tree.

use nu_protocol::{LabeledError, Record, Span, Value};
use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};

use crate::error::DiagnosticPath;

/// Distinguishes known conversion failures from arbitrary upstream Nu errors.
#[derive(Debug)]
pub(crate) enum ValueConversionError {
    /// A value or record key cannot be represented as JSON.
    Invalid {
        kind: &'static str,
        path: DiagnosticPath,
        span: Span,
    },
    /// A nested Nu error is propagated unchanged outside table row records.
    Upstream(LabeledError),
}

impl ValueConversionError {
    /// Produces the existing labeled Nu conversion diagnostic.
    pub(crate) fn into_labeled(self) -> LabeledError {
        match self {
            Self::Invalid { kind, path, span } => {
                LabeledError::new(format!("cannot convert {kind} at {path} to Jev JSON"))
                    .with_label("unsupported Jev state value", span)
            }
            Self::Upstream(error) => error,
        }
    }
}

/// Converts one Nu value recursively without parsing strings or stringifying containers.
/// Rejects unsupported values with their source span and propagates embedded Nu errors.
pub(crate) fn to_json(value: &Value) -> Result<JsonValue, LabeledError> {
    to_json_checked(value).map_err(ValueConversionError::into_labeled)
}

/// Converts a value while retaining a typed failure for safe table diagnostics.
pub(crate) fn to_json_checked(value: &Value) -> Result<JsonValue, ValueConversionError> {
    to_json_at_checked(value, "$".to_owned())
}

/// Converts one value while retaining a precise path for nested failures.
pub(crate) fn to_json_at(value: &Value, path: String) -> Result<JsonValue, LabeledError> {
    to_json_at_checked(value, path).map_err(ValueConversionError::into_labeled)
}

/// Recurses with structured failures that can be safely classified by callers.
fn to_json_at_checked(value: &Value, path: String) -> Result<JsonValue, ValueConversionError> {
    match value {
        // JSON-looking shell strings remain literal strings; decoding them is the caller's job.
        Value::String { val, .. } => Ok(JsonValue::String(val.clone())),
        Value::Int { val, .. } => Ok(JsonValue::Number((*val).into())),
        Value::Float { val, .. } => JsonNumber::from_f64(*val)
            .map(JsonValue::Number)
            .ok_or_else(|| conversion_error(&path, "non-finite float", value.span())),
        Value::Bool { val, .. } => Ok(JsonValue::Bool(*val)),
        Value::Nothing { .. } => Ok(JsonValue::Null),
        // JSON has no Date, Duration, or Filesize tags. Use deterministic external forms;
        // decoding them later must not guess the original Nu type from their contents.
        Value::Date { val, .. } => Ok(JsonValue::String(val.to_rfc3339())),
        Value::Duration { val, .. } => Ok(JsonValue::String(format!("{val}ns"))),
        Value::Filesize { val, .. } => Ok(JsonValue::Number(val.get().into())),
        Value::List { vals, .. } => vals
            .iter()
            .enumerate()
            .map(|(index, item)| to_json_at_checked(item, format!("{path}[{index}]")))
            .collect::<Result<Vec<_>, _>>()
            .map(JsonValue::Array),
        Value::Record { val, .. } => val
            .iter()
            .try_fold(
                JsonMap::with_capacity(val.len()),
                |mut object, (key, item)| {
                    let item_path = format!("{path}.{key}");
                    // Nu Record permits duplicate columns, unlike a JSON map. Refuse the
                    // conversion rather than letting map insertion pick one value silently.
                    if object.contains_key(key) {
                        return Err(conversion_error(
                            &item_path,
                            "duplicate record key",
                            item.span(),
                        ));
                    }
                    object.insert(key.clone(), to_json_at_checked(item, item_path)?);
                    Ok(object)
                },
            )
            .map(JsonValue::Object),
        Value::Error { error, .. } => Err(ValueConversionError::Upstream(LabeledError::from(
            (**error).clone(),
        ))),
        Value::Binary { .. } => Err(conversion_error(&path, "binary", value.span())),
        Value::Closure { .. } => Err(conversion_error(&path, "closure", value.span())),
        Value::Range { .. } => Err(conversion_error(&path, "range", value.span())),
        Value::CellPath { .. } => Err(conversion_error(&path, "cell path", value.span())),
        Value::Custom { .. } => Err(conversion_error(&path, "custom value", value.span())),
        Value::Glob { .. } => Err(conversion_error(&path, "glob", value.span())),
    }
}

/// Moves an owned JSON tree into Nu containers, assigning the supplied span recursively.
/// Leaves strings as strings and rejects integers outside Nu's signed 64-bit domain.
pub(crate) fn from_json(value: JsonValue, span: Span) -> Result<Value, LabeledError> {
    match value {
        JsonValue::Null => Ok(Value::nothing(span)),
        JsonValue::Bool(value) => Ok(Value::bool(value, span)),
        JsonValue::String(value) => Ok(Value::string(value, span)),
        JsonValue::Number(number) => number_to_nu(&number, span),
        JsonValue::Array(values) => values
            .into_iter()
            .map(|value| from_json(value, span))
            .collect::<Result<Vec<_>, _>>()
            .map(|values| Value::list(values, span)),
        JsonValue::Object(values) => {
            let mut record = Record::with_capacity(values.len());
            for (key, value) in values {
                record.push(key, from_json(value, span)?);
            }
            Ok(Value::record(record, span))
        }
    }
}

/// Copies borrowed JSON leaves directly into newly owned Nu containers with a supplied span.
/// Avoids an intermediate JSON-tree clone when projecting shared cached answers.
pub(crate) fn from_json_ref(value: &JsonValue, span: Span) -> Result<Value, LabeledError> {
    match value {
        JsonValue::Null => Ok(Value::nothing(span)),
        JsonValue::Bool(value) => Ok(Value::bool(*value, span)),
        JsonValue::String(value) => Ok(Value::string(value, span)),
        JsonValue::Number(number) => number_to_nu(number, span),
        JsonValue::Array(values) => values
            .iter()
            .map(|value| from_json_ref(value, span))
            .collect::<Result<Vec<_>, _>>()
            .map(|values| Value::list(values, span)),
        JsonValue::Object(values) => {
            let mut record = Record::with_capacity(values.len());
            for (key, value) in values {
                record.push(key.clone(), from_json_ref(value, span)?);
            }
            Ok(Value::record(record, span))
        }
    }
}

/// Preserves JSON integer bounds when projecting a number to Nu.
fn number_to_nu(number: &JsonNumber, span: Span) -> Result<Value, LabeledError> {
    if let Some(value) = number.as_i64() {
        Ok(Value::int(value, span))
    } else if number.is_u64() {
        // A float fallback would lose integer precision and hide an unrepresentable API value.
        Err(LabeledError::new(format!(
            "JSON integer {number} exceeds Nushell's signed 64-bit range"
        ))
        .with_label("out-of-range API number", span))
    } else {
        number
            .as_f64()
            .map(|value| Value::float(value, span))
            .ok_or_else(|| {
                LabeledError::new("JSON number cannot be represented as a Nushell float")
                    .with_label("unsupported API number", span)
            })
    }
}

/// Produces a labeled conversion error without leaking the unsupported value.
fn conversion_error(path: &str, kind: &'static str, span: Span) -> ValueConversionError {
    ValueConversionError::Invalid {
        kind,
        path: DiagnosticPath::new(path),
        span,
    }
}

#[cfg(test)]
mod tests {
    use nu_protocol::{Record, ShellError, Span, Value};
    use serde_json::json;

    use super::{from_json, to_json};

    /// Preserves strings, nested scalar values, and special Nu representations.
    #[test]
    fn converts_nested_values_and_special_types() {
        let mut record = Record::new();
        record.push("text", Value::test_string("{\"not\": \"parsed\"}"));
        record.push(
            "items",
            Value::test_list(vec![Value::test_int(i64::MIN), Value::test_nothing()]),
        );
        record.push(
            "date",
            Value::test_date("2026-09-15T12:30:00+02:00".parse().unwrap()),
        );
        record.push("duration", Value::test_duration(-1_500_000_000));
        record.push("size", Value::test_filesize(4096));
        assert_eq!(
            to_json(&Value::test_record(record)).unwrap(),
            json!({
                "text": "{\"not\": \"parsed\"}",
                "items": [i64::MIN, null],
                "date": "2026-09-15T12:30:00+02:00",
                "duration": "-1500000000ns",
                "size": 4096
            })
        );
    }

    /// Rejects unsupported nested values and non-finite numbers with their paths.
    #[test]
    fn reports_nested_conversion_failures() {
        let mut record = Record::new();
        record.push(
            "bad",
            Value::test_list(vec![Value::binary(vec![1], Span::test_data())]),
        );
        let error = to_json(&Value::test_record(record)).unwrap_err();
        assert!(error.msg.contains("$.bad[0]"));
        assert!(to_json(&Value::test_float(f64::NAN)).is_err());
        assert!(to_json(&Value::test_float(f64::INFINITY)).is_err());
    }

    /// Rejects repeated Nu record keys instead of silently overwriting JSON fields.
    #[test]
    fn rejects_duplicate_record_keys() {
        let mut nested = Record::new();
        nested.push("same", Value::test_int(1));
        nested.push("same", Value::test_int(2));
        let mut outer = Record::new();
        outer.push("nested", Value::test_record(nested));
        let error = to_json(&Value::test_record(outer)).unwrap_err();
        assert!(error.msg.contains("duplicate record key"));
        assert!(error.msg.contains("$.nested.same"));
    }

    /// Preserves upstream Nu errors instead of formatting them as state text.
    #[test]
    fn propagates_upstream_error() {
        let shell_error =
            ShellError::LabeledError(Box::new(nu_protocol::LabeledError::new("upstream failure")));
        let error = to_json(&Value::error(shell_error, Span::test_data())).unwrap_err();
        assert!(error.msg.contains("upstream failure"));
    }

    /// Rejects unsigned API integers that cannot be represented by Nu integers.
    #[test]
    fn checks_json_integer_boundaries() {
        assert_eq!(
            from_json(json!(i64::MAX), Span::test_data())
                .unwrap()
                .as_int()
                .unwrap(),
            i64::MAX
        );
        assert!(from_json(json!(u64::MAX), Span::test_data()).is_err());
    }
}
