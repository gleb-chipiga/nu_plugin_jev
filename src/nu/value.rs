//! Converts structured Nushell values to and from JSON with explicit type rules.

use nu_protocol::{LabeledError, Record, Span, Value};
use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};

/// Converts one Nu value recursively, reporting the path of unsupported values.
pub(crate) fn to_json(value: &Value) -> Result<JsonValue, LabeledError> {
    to_json_at(value, "$".to_owned())
}

/// Converts one value while retaining a precise path for nested failures.
fn to_json_at(value: &Value, path: String) -> Result<JsonValue, LabeledError> {
    match value {
        Value::String { val, .. } => Ok(JsonValue::String(val.clone())),
        Value::Int { val, .. } => Ok(JsonValue::Number((*val).into())),
        Value::Float { val, .. } => JsonNumber::from_f64(*val)
            .map(JsonValue::Number)
            .ok_or_else(|| conversion_error(&path, "non-finite float", value.span())),
        Value::Bool { val, .. } => Ok(JsonValue::Bool(*val)),
        Value::Nothing { .. } => Ok(JsonValue::Null),
        Value::Date { val, .. } => Ok(JsonValue::String(val.to_rfc3339())),
        Value::Duration { val, .. } => Ok(JsonValue::String(format!("{val}ns"))),
        Value::Filesize { val, .. } => Ok(JsonValue::Number(val.get().into())),
        Value::List { vals, .. } => vals
            .iter()
            .enumerate()
            .map(|(index, item)| to_json_at(item, format!("{path}[{index}]")))
            .collect::<Result<Vec<_>, _>>()
            .map(JsonValue::Array),
        Value::Record { val, .. } => val
            .iter()
            .map(|(key, item)| {
                to_json_at(item, format!("{path}.{key}")).map(|json| (key.clone(), json))
            })
            .collect::<Result<JsonMap<_, _>, _>>()
            .map(JsonValue::Object),
        Value::Error { error, .. } => Err(LabeledError::from((**error).clone())),
        Value::Binary { .. } => Err(conversion_error(&path, "binary", value.span())),
        Value::Closure { .. } => Err(conversion_error(&path, "closure", value.span())),
        Value::Range { .. } => Err(conversion_error(&path, "range", value.span())),
        Value::CellPath { .. } => Err(conversion_error(&path, "cell path", value.span())),
        Value::Custom { .. } => Err(conversion_error(&path, "custom value", value.span())),
        Value::Glob { .. } => Err(conversion_error(&path, "glob", value.span())),
    }
}

/// Converts an API JSON value to a Nu value without interpreting strings.
pub(crate) fn from_json(value: JsonValue, span: Span) -> Result<Value, LabeledError> {
    match value {
        JsonValue::Null => Ok(Value::nothing(span)),
        JsonValue::Bool(value) => Ok(Value::bool(value, span)),
        JsonValue::String(value) => Ok(Value::string(value, span)),
        JsonValue::Number(number) => {
            if let Some(value) = number.as_i64() {
                Ok(Value::int(value, span))
            } else if number.is_u64() {
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

/// Produces a labeled conversion error without leaking the unsupported value.
fn conversion_error(path: &str, kind: &str, span: Span) -> LabeledError {
    LabeledError::new(format!("cannot convert {kind} at {path} to Jev JSON"))
        .with_label("unsupported Jev state value", span)
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
            json!({"text": "{\"not\": \"parsed\"}", "items": [i64::MIN, null],
                "date": "2026-09-15T12:30:00+02:00", "duration": "-1500000000ns",
                "size": 4096})
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
