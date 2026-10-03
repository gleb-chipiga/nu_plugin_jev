//! Formats filtered diagnostics as JSON-compatible, newline-delimited NUON.

use std::fmt;

use ::tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use serde_json::{Map, Value, json};
use tracing_log::NormalizeEvent;
use tracing_subscriber::{
    fmt::{
        FmtContext,
        format::{FormatEvent, FormatFields, Writer},
    },
    layer::{Context, Layer},
    registry::LookupSpan,
};

/// Retains typed tracing fields, including fields recorded after span creation.
#[derive(Default)]
struct CapturedFields(Map<String, Value>);

impl CapturedFields {
    /// Replaces a field with its latest typed value.
    fn insert(&mut self, field: &Field, value: Value) {
        self.0.insert(field.name().to_owned(), value);
    }
}

impl Visit for CapturedFields {
    /// Preserves a signed value that Nu can represent as an integer.
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, Value::from(value));
    }

    /// Falls back to a string when an unsigned integer exceeds Nu's range.
    fn record_u64(&mut self, field: &Field, value: u64) {
        let value = i64::try_from(value)
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(value.to_string()));
        self.insert(field, value);
    }

    /// Keeps finite floats numeric and renders non-finite values as strings.
    fn record_f64(&mut self, field: &Field, value: f64) {
        let value = serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(format!("{value:?}")));
        self.insert(field, value);
    }

    /// Retains boolean values without converting them to text.
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, Value::Bool(value));
    }

    /// Escapes strings only once during final serialization.
    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, Value::String(value.to_owned()));
    }

    /// Represents debug-only values as strings.
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.insert(field, Value::String(format!("{value:?}")));
    }
}

/// Stores recorded span fields so the event formatter can recover hierarchy.
pub(super) struct SpanFieldLayer;

impl<S> Layer<S> for SpanFieldLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    /// Captures fields when a span is first created.
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut fields = CapturedFields::default();
            attrs.record(&mut fields);
            span.extensions_mut().insert(fields);
        }
    }

    /// Updates fields recorded on an existing span.
    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(fields) = span.extensions_mut().get_mut::<CapturedFields>()
        {
            values.record(fields);
        }
    }
}

/// Emits each event as one compact NUON record and a single newline.
pub(super) struct NuonFormatter;

impl<S, N> FormatEvent<S, N> for NuonFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    /// Serializes event and span fields without emitting span lifecycle events.
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let timestamp = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| fmt::Error)?;
        let mut fields = CapturedFields::default();
        event.record(&mut fields);
        let message = fields
            .0
            .remove("message")
            .map(|value| match value {
                Value::String(value) => value,
                other => other.to_string(),
            })
            .unwrap_or_default();
        let spans: Vec<Value> = ctx
            .event_scope()
            .map(|scope| {
                scope
                    .from_root()
                    .map(|span| {
                        let extensions = span.extensions();
                        let fields = extensions
                            .get::<CapturedFields>()
                            .map(|fields| fields.0.clone())
                            .unwrap_or_default();
                        json!({"name": span.name(), "fields": fields})
                    })
                    .collect()
            })
            .unwrap_or_default();
        let normalized_metadata = event.normalized_metadata();
        let target = normalized_metadata
            .as_ref()
            .map_or_else(|| event.metadata().target(), |metadata| metadata.target());
        let record = json!({
            "timestamp": timestamp,
            "level": event.metadata().level().as_str().to_ascii_lowercase(),
            "target": target,
            "message": message,
            "fields": fields.0,
            "spans": spans,
        });
        let line = serde_json::to_string(&record).map_err(|_| fmt::Error)?;
        writer.write_str(&line)?;
        writer.write_char('\n')
    }
}
