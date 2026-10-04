//! Adds typed Jev answers to record rows without materializing a whole table.

use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use nu_plugin::{EngineInterface, EvaluatedCall, PluginCommand};
use nu_protocol::{
    Example, LabeledError, ListStream, PipelineData, Record, ShellError, SignalAction, Signature,
    Span, SyntaxShape, Type, Value,
    ast::{CellPath, PathMember},
};

use crate::{
    api::{cancel::CancelHandle, types::Question},
    config::{ConfigScope, capture_sources, require_api_key, resolve},
    error::JevError,
    nu::{
        state::{build_request, build_shared_request},
        stream::{RowBuilder, RowOutcome, StreamSetup, start},
        typed::answers_to_nu,
        value::to_json,
    },
    plugin::JevPlugin,
};

use super::evaluate::{evaluation_meta, measurement_value, preview_value};

/// Annotates each record row with answers from one named multi-question request.
pub(crate) struct JevAnnotate;

/// Selects either a nested cell or literal top-level fields from each source row.
#[derive(Clone)]
enum StateSelector {
    Whole,
    Cell(CellPath),
    Fields(Vec<String>),
}

/// Controls how an invalid row or failed HTTP evaluation appears downstream.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ErrorPolicy {
    Fail,
    Keep,
    Record,
}

/// Captures parsed table options before the first input row is read.
struct TableOptions {
    selector: StateSelector,
    context: Option<Value>,
    into: String,
    metrics: bool,
    on_error: ErrorPolicy,
    unordered: bool,
    dry_run: bool,
}

impl PluginCommand for JevAnnotate {
    type Plugin = JevPlugin;

    /// Names the row-wise multi-question operation.
    fn name(&self) -> &str {
        "jev annotate"
    }

    /// Defines selectors, stream controls, and answer destinations.
    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_type(Type::Any, Type::list(Type::Any))
            .required(
                "questions",
                SyntaxShape::Record(vec![].into()),
                "Named Jev questions",
            )
            .named(
                "context",
                SyntaxShape::Any,
                "Explicit static context",
                Some('c'),
            )
            .named(
                "model",
                SyntaxShape::String,
                "Requested Jev model",
                Some('m'),
            )
            .named(
                "base-url",
                SyntaxShape::String,
                "TypeSafe API service root",
                None,
            )
            .named(
                "timeout",
                SyntaxShape::Duration,
                "Total deadline per evaluation",
                None,
            )
            .named(
                "config",
                SyntaxShape::String,
                "Explicit local TOML file",
                None,
            )
            .named(
                "jobs",
                SyntaxShape::Int,
                "Maximum concurrent unique evaluations",
                Some('j'),
            )
            .named(
                "state",
                SyntaxShape::CellPath,
                "Nested input state cell path",
                Some('s'),
            )
            .named(
                "fields",
                SyntaxShape::Any,
                "Literal top-level fields to send",
                Some('f'),
            )
            .named(
                "into",
                SyntaxShape::String,
                "Answer field (default: answers)",
                Some('i'),
            )
            .switch("metrics", "Add jev_metrics to successful rows", None)
            .named(
                "on-error",
                SyntaxShape::String,
                "fail, keep, or record",
                None,
            )
            .switch("unordered", "Emit rows as their evaluations complete", None)
            .switch(
                "dry-run",
                "Stream {request, request_bytes} without HTTP",
                None,
            )
    }

    /// Summarizes independent per-row decisions.
    fn description(&self) -> &str {
        "Annotate record rows with named Jev decisions"
    }

    /// Clarifies that each row is one state and native Nu handles filtering.
    fn extra_description(&self) -> &str {
        concat!(
            "Each row is an independent System One state; matching requests may share one ",
            "evaluation. Answers go under answers by default; --into jev preserves the former ",
            "answer path. Successful rows always add jev_meta with base_url, model, and usage; ",
            "--metrics adds jev_metrics with one shared request_id and HTTP measurements. ",
            "--fields selects literal top-level names, while --state follows a Nu cell path. ",
            "Use native where and sort-by on answers. Settings are snapshotted per call from ",
            "flags, Nu config, caller environment, local TOML (--config or ",
            "NU_PLUGIN_JEV_CONFIG, otherwise .nu_plugin_jev.toml), user TOML, then defaults."
        )
    }

    /// Shows a credential-free structured preview.
    fn examples(&self) -> Vec<Example<'_>> {
        vec![
            Example {
                example: concat!(
                    "[{id: 1, message: 'hello'}] | jev annotate ",
                    "{greeting: (jev question noul 'Is this a greeting?')} ",
                    "-f [message] --dry-run"
                ),
                description: "Preview the per-row outbound state without sending data",
                result: None,
            },
            Example {
                example: concat!(
                    "[{id: 1, document: {text: 'hello'}}] | jev annotate ",
                    "{greeting: {type: noul}} -s document.text ",
                    "-c {policy: 'greetings'} --dry-run"
                ),
                description: "Preview a nested cell-path state with explicit context",
                result: None,
            },
        ]
    }

    /// Validates options once, then returns a bounded lazy table stream.
    fn run(
        &self,
        plugin: &JevPlugin,
        engine: &EngineInterface,
        call: &EvaluatedCall,
        input: PipelineData,
    ) -> Result<PipelineData, LabeledError> {
        let questions: Value = call.req(0).map_err(LabeledError::from)?;
        let questions = crate::api::validate::parse_questions(&questions)?;
        let options = TableOptions::parse(call)?;
        let sources = capture_sources(engine, call, ConfigScope::Table)?;
        let config = resolve(&sources, ConfigScope::Table)?;
        let key = if options.dry_run {
            None
        } else {
            Some(require_api_key(&sources)?)
        };
        let rows = input_rows(input, call.head)?;
        let span = call.head;
        let signals = engine.signals();
        let build = request_builder(&options, config.model.clone(), questions);
        if options.dry_run {
            let iterator = preview_rows(rows, build, options.on_error, span);
            return Ok(PipelineData::list_stream(
                ListStream::new(iterator, span, signals.clone()),
                None,
            ));
        }
        let client = plugin
            .client
            .for_policy(&config.proxy)
            .map_err(JevError::into_labeled)?;
        signals.check(&span).map_err(LabeledError::from)?;
        let cancellation = CancelHandle::new();
        let handler_cancel = cancellation.0.clone();
        let handler = engine
            .register_signal_handler(Box::new(move |action| {
                if action == SignalAction::Interrupt {
                    handler_cancel.cancel();
                }
            }))
            .map_err(LabeledError::from)?;
        let setup = StreamSetup {
            client,
            config,
            key: key.expect("live command has a caller key"),
            unordered: options.unordered,
            fail_fast: options.on_error == ErrorPolicy::Fail,
        };
        let output = start(
            Arc::clone(&plugin.runtime),
            setup,
            rows,
            build,
            Some(handler),
            cancellation,
        )
        .map_err(JevError::into_labeled)?;
        let iterator = output.map(move |outcome| annotate_outcome(outcome, &options, span));
        Ok(PipelineData::list_stream(
            ListStream::new(iterator, span, signals.clone()),
            None,
        ))
    }
}

/// Previews rows lazily and stops after any terminal validation failure.
fn preview_rows(
    mut rows: Box<dyn Iterator<Item = Value> + Send>,
    build: RowBuilder,
    on_error: ErrorPolicy,
    span: Span,
) -> impl Iterator<Item = Value> {
    let mut stopped = false;
    std::iter::from_fn(move || {
        if stopped {
            return None;
        }
        let row = rows.next()?;
        Some(match build(&row) {
            Ok(request) => preview_value(request, span)
                .unwrap_or_else(|error| Value::error(ShellError::from(error), span)),
            Err(error) if matches!(&error, JevError::FieldCollision(_)) => {
                stopped = true;
                error_value(&error, span)
            }
            Err(error) => match on_error {
                ErrorPolicy::Fail => {
                    stopped = true;
                    error_value(&error, span)
                }
                ErrorPolicy::Keep => row,
                ErrorPolicy::Record => add_error(row, &error, span),
            },
        })
    })
}

impl TableOptions {
    /// Parses and validates selectors, destinations, and policies before input consumption.
    fn parse(call: &EvaluatedCall) -> Result<Self, LabeledError> {
        let state: Option<CellPath> = call.get_flag("state").map_err(LabeledError::from)?;
        let fields = call.get_flag_value("fields");
        if state.is_some() && fields.is_some() {
            return Err(option_error("--state and --fields cannot be used together"));
        }
        let selector = if let Some(path) = state {
            if path.members.is_empty()
                || path.members.iter().any(|member| match member {
                    PathMember::String { optional, .. } | PathMember::Int { optional, .. } => {
                        *optional
                    }
                })
            {
                return Err(option_error("--state requires a nonoptional cell path"));
            }
            StateSelector::Cell(path)
        } else if let Some(fields) = fields {
            StateSelector::Fields(parse_fields(&fields)?)
        } else {
            StateSelector::Whole
        };
        let into: Option<String> = call.get_flag("into").map_err(LabeledError::from)?;
        let into = into.unwrap_or_else(|| "answers".into());
        if into.trim().is_empty() {
            return Err(option_error("--into must be nonempty"));
        }
        let metrics = call.has_flag("metrics").map_err(LabeledError::from)?;
        let dry_run = call.has_flag("dry-run").map_err(LabeledError::from)?;
        if metrics && dry_run {
            return Err(option_error("--metrics requires a live Jev request"));
        }
        if into == "jev_meta" || (metrics && into == "jev_metrics") {
            return Err(option_error("--into conflicts with a Jev output field"));
        }
        let on_error: Option<String> = call.get_flag("on-error").map_err(LabeledError::from)?;
        let on_error = match on_error.as_deref().unwrap_or("fail") {
            "fail" => ErrorPolicy::Fail,
            "keep" => ErrorPolicy::Keep,
            "record" => ErrorPolicy::Record,
            _ => return Err(option_error("--on-error must be fail, keep, or record")),
        };
        if on_error == ErrorPolicy::Record && into == "jev_error" {
            return Err(option_error("answer field must differ from jev_error"));
        }
        Ok(Self {
            selector,
            context: call.get_flag_value("context"),
            into,
            metrics,
            on_error,
            unordered: call.has_flag("unordered").map_err(LabeledError::from)?,
            dry_run,
        })
    }
}

/// Validates a nonempty distinct list of literal column names.
fn parse_fields(value: &Value) -> Result<Vec<String>, LabeledError> {
    let Value::List { vals, .. } = value else {
        return Err(option_error("--fields must be a nonempty list of strings"));
    };
    let mut seen = HashSet::with_capacity(vals.len());
    let fields = vals
        .iter()
        .map(|value| match value {
            Value::String { val, .. } if !val.is_empty() && seen.insert(val.as_str()) => {
                Ok(val.clone())
            }
            _ => Err(option_error("--fields requires distinct nonempty strings")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if fields.is_empty() {
        return Err(option_error("--fields must be nonempty"));
    }
    Ok(fields)
}

/// Turns a record, list, or list stream into independently processed rows.
fn input_rows(
    input: PipelineData,
    span: Span,
) -> Result<Box<dyn Iterator<Item = Value> + Send>, LabeledError> {
    match input {
        PipelineData::Empty => Ok(Box::new(std::iter::empty())),
        PipelineData::Value(Value::List { vals, .. }, _) => {
            Ok(Box::new(vals.into_owned().into_iter()))
        }
        PipelineData::Value(value, _) => Ok(Box::new(std::iter::once(value))),
        PipelineData::ListStream(stream, _) => Ok(Box::new(stream.into_iter())),
        PipelineData::ByteStream(_, _) => Err(LabeledError::new("byte streams are not Jev rows")
            .with_label("decode the input first", span)),
    }
}

/// Constructs the exact shared request body from each selected row state.
fn request_builder(
    options: &TableOptions,
    model: String,
    questions: BTreeMap<String, Question>,
) -> RowBuilder {
    let selector = options.selector.clone();
    let context = options.context.clone();
    let prepared_context = context.as_ref().map(to_json).transpose().ok();
    let questions = Arc::new(questions);
    let into = options.into.clone();
    let metrics = options.metrics;
    let on_error = options.on_error;
    Box::new(move |source| {
        if matches!(source, Value::Error { .. }) {
            return Err(JevError::State("upstream Nushell row error"));
        }
        let Value::Record { val, .. } = source else {
            return Err(JevError::State("jev annotate requires record rows"));
        };
        if val.contains(&into)
            || val.contains("jev_meta")
            || (metrics && val.contains("jev_metrics"))
            || (on_error == ErrorPolicy::Record && val.contains("jev_error"))
        {
            return Err(JevError::FieldCollision(
                "Jev destination field already exists",
            ));
        }
        let selected = match &selector {
            StateSelector::Whole => source.clone(),
            StateSelector::Cell(path) => source
                .follow_cell_path(&path.members)
                .map_err(|_| JevError::State("selected Jev state cell path is missing"))?
                .into_owned(),
            StateSelector::Fields(names) => {
                let mut projected = Record::with_capacity(names.len());
                for name in names {
                    let value = val.get(name).ok_or_else(|| JevError::missing_field(name))?;
                    projected.push(name.clone(), value.clone());
                }
                Value::record(projected, source.span())
            }
        };
        let request = if let Some(context_json) = &prepared_context {
            build_shared_request(
                &selected,
                context_json.as_ref(),
                &model,
                Arc::clone(&questions),
            )
        } else {
            // Keep the original per-row error order for invalid static context.
            build_request(
                &selected,
                context.as_ref(),
                model.clone(),
                questions.as_ref().clone(),
            )
        };
        request.map_err(crate::nu::state::StateBuildError::into_jev)
    })
}

/// Appends answers, provenance, and optional measurements to a successful row.
fn annotate_outcome(outcome: RowOutcome, options: &TableOptions, span: Span) -> Value {
    match outcome.result {
        Ok(shared) => {
            let Value::Record {
                val, internal_span, ..
            } = outcome.source
            else {
                unreachable!("source row validated before HTTP")
            };
            let mut record = val.into_owned();
            let answers = match answers_to_nu(&shared.response.answers, span) {
                Ok(value) => value,
                Err(error) => return Value::error(ShellError::from(error), span),
            };
            record.push(options.into.clone(), answers);
            let meta = match evaluation_meta(&shared.base_url, &shared.response, span) {
                Ok(value) => value,
                Err(error) => return Value::error(ShellError::from(error), span),
            };
            record.push("jev_meta", meta);
            if options.metrics {
                let metrics =
                    match measurement_value(&shared.measurement, Some(&shared.request_id), span) {
                        Ok(value) => value,
                        Err(error) => return Value::error(ShellError::from(error), span),
                    };
                record.push("jev_metrics", metrics);
            }
            Value::record(record, internal_span)
        }
        Err(error) if matches!(&*error, JevError::FieldCollision(_)) => error_value(&error, span),
        Err(error) => match options.on_error {
            ErrorPolicy::Fail => error_value(&error, span),
            ErrorPolicy::Keep => outcome.source,
            ErrorPolicy::Record => add_error(outcome.source, &error, span),
        },
    }
}

/// Converts one classified failure to a native pipeline error value.
fn error_value(error: &JevError, span: Span) -> Value {
    Value::error(ShellError::from(error.to_labeled()), span)
}

/// Preserves the source row and appends a structured classified failure.
fn add_error(row: Value, error: &JevError, span: Span) -> Value {
    let Value::Record {
        val, internal_span, ..
    } = row
    else {
        return error_value(error, span);
    };
    if val.contains("jev_error") {
        return error_value(
            &JevError::FieldCollision("Jev error field already exists"),
            span,
        );
    }
    let mut source = val.into_owned();
    let mut details = Record::with_capacity(3);
    details.push("kind", Value::string(error.kind_name(), span));
    details.push("message", Value::string(error.to_string(), span));
    details.push(
        "status",
        error.status().map_or_else(
            || Value::nothing(span),
            |status| Value::int(i64::from(status), span),
        ),
    );
    source.push("jev_error", Value::record(details, span));
    Value::record(source, internal_span)
}

/// Reports option validation without echoing caller data.
fn option_error(message: impl Into<String>) -> LabeledError {
    LabeledError::new(message).with_code("jev::validation")
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        hint::black_box,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Instant,
    };

    use nu_plugin::PluginCommand;
    use nu_plugin_test_support::PluginTest;
    use nu_protocol::{Record, ShellError, Span, Value};
    use serde_json::json;

    use crate::{
        api::{client::PreparedRequest, types::Question},
        commands::tests::serve,
        error::JevError,
        nu::value::to_json,
        plugin::JevPlugin,
    };

    /// Creates an isolated plugin engine with an explicitly built runtime.
    fn plugin_test() -> Result<PluginTest, Box<ShellError>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        PluginTest::new("jev", JevPlugin::new(runtime).into()).map_err(Box::new)
    }

    /// Builds a stable Noul answer for annotation tests.
    fn mock_noul_response() -> serde_json::Value {
        json!({
            "model": "jev-fixed",
            "answers": {"q": {"type": "noul", "noul": 0.875}},
            "usage": {"input_tokens": 10, "output_tokens": 1}
        })
    }

    /// Keeps exact bodies while reusing questions across independent rows.
    #[test]
    fn shared_row_builder_matches_legacy_bytes() {
        let mut context = Record::new();
        context.push(
            "policy",
            Value::test_list(vec![
                Value::test_string("отвечай точно"),
                Value::test_int(7),
            ]),
        );
        let options = super::TableOptions {
            selector: super::StateSelector::Whole,
            context: Some(Value::test_record(context)),
            into: "answers".into(),
            metrics: false,
            on_error: super::ErrorPolicy::Fail,
            unordered: false,
            dry_run: false,
        };
        let questions = BTreeMap::from([
            (
                "spam".into(),
                Question::Noul {
                    instructions: Some(json!({"task": "classify", "language": "ru"})),
                    criteria: None,
                },
            ),
            (
                "kind".into(),
                Question::Choice {
                    instructions: Some(json!("Выбери тип")),
                    criteria: BTreeMap::from([
                        ("normal".into(), json!(null)),
                        ("spam".into(), json!("Нежелательная рассылка")),
                    ]),
                },
            ),
        ]);
        let build = super::request_builder(&options, "jev-latest".into(), questions.clone());
        let rows = ["Привет", "Buy now"].map(|message| {
            Value::test_record({
                let mut record = Record::new();
                record.push("message", Value::test_string(message));
                record
            })
        });
        let built = rows.map(|row| {
            let legacy = super::build_request(
                &row,
                options.context.as_ref(),
                "jev-latest".into(),
                questions.clone(),
            )
            .unwrap();
            let shared = build(&row).unwrap();
            assert_eq!(
                PreparedRequest::new(legacy).unwrap().body,
                PreparedRequest::new(shared.clone()).unwrap().body
            );
            shared
        });
        assert!(Arc::ptr_eq(&built[0].questions, &built[1].questions));
    }

    /// Retains the input-before-context error order when static context is invalid.
    #[test]
    fn invalid_static_context_preserves_row_error_precedence() {
        let mut context = Record::new();
        context.push("context_bad", Value::test_binary(vec![1]));
        let options = super::TableOptions {
            selector: super::StateSelector::Whole,
            context: Some(Value::test_record(context)),
            into: "answers".into(),
            metrics: false,
            on_error: super::ErrorPolicy::Fail,
            unordered: false,
            dry_run: false,
        };
        let questions = BTreeMap::from([(
            "q".into(),
            Question::Noul {
                instructions: None,
                criteria: None,
            },
        )]);
        let build = super::request_builder(&options, "jev-latest".into(), questions);
        let mut invalid = Record::new();
        invalid.push("input_bad", Value::test_binary(vec![2]));
        let error = build(&Value::test_record(invalid)).unwrap_err();
        assert_eq!(error.kind_name(), "state");
        assert!(error.to_string().contains("$.input_bad"));
        let mut valid = Record::new();
        valid.push("message", Value::test_string("hello"));
        let error = build(&Value::test_record(valid)).unwrap_err();
        assert_eq!(error.kind_name(), "state");
        assert!(error.to_string().contains("$.context_bad"));
    }

    /// Measures per-row construction and preparation with shared policy data.
    #[test]
    #[ignore = "manual local performance comparison"]
    fn benchmark_row_preparation() {
        for (label, count, context_bytes) in [("small", 3, 1024), ("large", 24, 8192)] {
            let criteria = (0..6)
                .map(|index| (format!("option_{index}"), json!(format!("Meaning {index}"))))
                .collect::<BTreeMap<_, _>>();
            let questions = (0..count)
                .map(|index| {
                    (
                        format!("question_{index}"),
                        Question::Choice {
                            instructions: Some(json!("Categorize this message")),
                            criteria: criteria.clone(),
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let mut context = Record::new();
            context.push("policy", Value::test_string("p".repeat(context_bytes)));
            let options = super::TableOptions {
                selector: super::StateSelector::Whole,
                context: Some(Value::test_record(context)),
                into: "answers".into(),
                metrics: false,
                on_error: super::ErrorPolicy::Fail,
                unordered: false,
                dry_run: false,
            };
            let build = super::request_builder(&options, "jev-latest".into(), questions.clone());
            let mut record = Record::new();
            record.push("message", Value::test_string("message ".repeat(32)));
            let row = Value::test_record(record);
            let legacy = || {
                super::build_request(
                    &row,
                    options.context.as_ref(),
                    "jev-latest".into(),
                    questions.clone(),
                )
                .unwrap()
            };
            assert_eq!(
                PreparedRequest::new(legacy()).unwrap().body,
                PreparedRequest::new(build(&row).unwrap()).unwrap().body
            );
            let mut baseline_build_times = Vec::new();
            let mut build_times = Vec::new();
            let mut baseline_prepared_times = Vec::new();
            let mut prepared_times = Vec::new();
            for _ in 0..5 {
                let started = Instant::now();
                for _ in 0..1000 {
                    black_box(legacy());
                }
                baseline_build_times.push(started.elapsed().as_nanos());
                let started = Instant::now();
                for _ in 0..1000 {
                    black_box(build(black_box(&row)).unwrap());
                }
                build_times.push(started.elapsed().as_nanos());
                let started = Instant::now();
                for _ in 0..1000 {
                    black_box(PreparedRequest::new(legacy()).unwrap().body.len());
                }
                baseline_prepared_times.push(started.elapsed().as_nanos());
                let started = Instant::now();
                for _ in 0..1000 {
                    let request = build(black_box(&row)).unwrap();
                    black_box(PreparedRequest::new(request).unwrap().body.len());
                }
                prepared_times.push(started.elapsed().as_nanos());
            }
            baseline_build_times.sort_unstable();
            build_times.sort_unstable();
            baseline_prepared_times.sort_unstable();
            prepared_times.sort_unstable();
            println!(
                concat!(
                    "{label}: baseline_build_ms={:.3}, optimized_build_ms={:.3}, ",
                    "baseline_prepared_ms={:.3}, optimized_prepared_ms={:.3}"
                ),
                baseline_build_times[2] as f64 / 1_000_000.0,
                build_times[2] as f64 / 1_000_000.0,
                baseline_prepared_times[2] as f64 / 1_000_000.0,
                prepared_times[2] as f64 / 1_000_000.0,
                label = label
            );
        }
    }

    /// Sends only projected literal fields and explicit context in every preview.
    #[test]
    fn dry_run_projects_fields_without_credentials() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let result = test
            .eval(concat!(
                "[{id: 1, message: 'hello', secret: 'local'} ",
                "{id: 2, message: 'bye', secret: 'local'}] | jev annotate ",
                "{q: {type: noul}} --fields [message] ",
                "--context {policy: 'greetings'} --dry-run"
            ))?
            .into_value(Span::test_data())?;
        let wire = to_json(&result).unwrap();
        assert_eq!(wire.as_array().unwrap().len(), 2);
        assert_eq!(
            wire[0]["request"]["state"],
            json!({"input": {"message": "hello"}, "context": {"policy": "greetings"}})
        );
        assert_eq!(
            wire[1]["request"]["state"]["input"],
            json!({"message": "bye"})
        );
        for row in wire.as_array().unwrap() {
            assert_eq!(
                row["request_bytes"],
                json!(serde_json::to_vec(&row["request"]).unwrap().len())
            );
        }
        Ok(())
    }

    /// Shares request construction and configuration precedence across flag spellings.
    #[test]
    fn short_options_match_long_projection_preview_and_live_output() -> Result<(), Box<ShellError>>
    {
        let response = mock_noul_response();
        let (base_url, server) = serve(vec![response]);
        let mut test = plugin_test()?;
        let setup = concat!(
            "$env.NU_PLUGIN_JEV_MODEL = 'env-model'; ",
            "$env.NU_PLUGIN_JEV_JOBS = '0'; ",
            "$env.config.plugins.jev = {model: 'config-model', jobs: 0};"
        );
        let common = format!(
            concat!(
                "[{{id: 1, message: 'hello'}}] | jev annotate ",
                "{{q: {{type: noul}}}} --base-url '{base_url}'"
            ),
            base_url = base_url
        );
        let long = test
            .eval(&format!(
                concat!(
                    "{setup} {common} --fields [message] ",
                    "--context {{policy: 'greetings'}} --model flag-model ",
                    "--jobs 1 --into ai --dry-run"
                ),
                setup = setup,
                common = common
            ))?
            .into_value(Span::test_data())?;
        let short = test
            .eval(&format!(
                concat!(
                    "{setup} {common} -f [message] ",
                    "-c {{policy: 'greetings'}} -m flag-model -j 1 -i ai --dry-run"
                ),
                setup = setup,
                common = common
            ))?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&short).unwrap(), to_json(&long).unwrap());
        assert_eq!(
            to_json(&short).unwrap()[0]["request"]["model"],
            "flag-model"
        );

        let live = test
            .eval(&format!(
                concat!(
                    "{setup} $env.TYPESAFE_API_KEY = 'local'; {common} ",
                    "-f [message] -c {{policy: 'greetings'}} -m flag-model -j 1 -i ai"
                ),
                setup = setup,
                common = common
            ))?
            .into_value(Span::test_data())?;
        let live = to_json(&live).unwrap();
        assert_eq!(live[0]["id"], 1);
        assert_eq!(live[0]["ai"]["q"]["noul"], 0.875);
        assert!(live[0].get("jev").is_none());
        assert_eq!(
            server.join().unwrap()[0].body,
            to_json(&long).unwrap()[0]["request"]
        );
        Ok(())
    }

    /// Uses the same nested cell-path selection for -s and --state.
    #[test]
    fn short_state_option_matches_long_state_preview() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let common = "[{payload: {text: 'hello'}, id: 1}] | jev annotate {q: {type: noul}}";
        let long = test
            .eval(&format!("{common} --state payload.text --dry-run"))?
            .into_value(Span::test_data())?;
        let short = test
            .eval(&format!("{common} -s payload.text --dry-run"))?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&short).unwrap(), to_json(&long).unwrap());
        assert_eq!(to_json(&short).unwrap()[0]["request"]["state"], "hello");
        Ok(())
    }

    /// Preserves every source column while inserting typed answers and provenance.
    #[test]
    fn live_annotation_preserves_source_and_metadata() -> Result<(), Box<ShellError>> {
        let response = mock_noul_response();
        let (base_url, server) = serve(vec![response]);
        let mut test = plugin_test()?;
        let result = test
            .eval(&format!(
                concat!(
                    "$env.TYPESAFE_API_KEY = 'local-key'; ",
                    "[{{id: 7, payload: {{text: 'hello'}}, secret: 'local'}}] ",
                    "| jev annotate {{q: {{type: noul}}}} --state payload.text ",
                    "--into ai --metrics --base-url '{base_url}'"
                ),
                base_url = base_url
            ))?
            .into_value(Span::test_data())?;
        let wire = to_json(&result).unwrap();
        assert_eq!(wire[0]["id"], 7);
        assert_eq!(wire[0]["secret"], "local");
        assert_eq!(wire[0]["ai"]["q"]["noul"], 0.875);
        assert_eq!(wire[0]["jev_meta"]["base_url"], format!("{base_url}/"));
        assert_eq!(wire[0]["jev_meta"]["model"], "jev-fixed");
        assert_eq!(wire[0]["jev_meta"]["usage"]["input_tokens"], 10);
        assert_eq!(wire[0]["jev_meta"].as_object().unwrap().len(), 3);
        assert!(
            wire[0]["jev_metrics"]["request_id"]
                .as_str()
                .unwrap()
                .starts_with("jev-")
        );
        let captured = server.join().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].body["state"], "hello");
        assert_eq!(
            captured[0].authorization.as_deref(),
            Some("Bearer local-key")
        );
        Ok(())
    }

    /// Defaults to answers while an explicit legacy destination changes only answer placement.
    #[test]
    fn default_answers_and_explicit_jev_keep_fixed_metadata() -> Result<(), Box<ShellError>> {
        let response = mock_noul_response();
        let (base_url, server) = serve(vec![response.clone(), response]);
        let mut test = plugin_test()?;
        let setup = format!(
            "$env.TYPESAFE_API_KEY = 'local-key'; $env.NU_PLUGIN_JEV_BASE_URL = '{base_url}';"
        );
        let source =
            "[{id: 7, message: 'hello'}] | jev annotate {q: {type: noul}} --fields [message]";
        let default = test
            .eval(&format!("{setup} {source}"))?
            .into_value(Span::test_data())?;
        let default = to_json(&default).unwrap();
        assert_eq!(default[0]["answers"]["q"]["noul"], 0.875);
        assert_eq!(default[0]["jev_meta"]["model"], "jev-fixed");
        assert!(default[0].get("jev").is_none());
        assert!(default[0].get("jev_metrics").is_none());

        let legacy = test
            .eval(&format!("{setup} {source} --into jev --metrics"))?
            .into_value(Span::test_data())?;
        let legacy = to_json(&legacy).unwrap();
        assert_eq!(legacy[0]["jev"]["q"]["noul"], 0.875);
        assert!(legacy[0].get("answers").is_none());
        assert_eq!(legacy[0]["jev_meta"], default[0]["jev_meta"]);
        assert!(legacy[0]["jev_metrics"]["request_id"].is_string());
        assert_eq!(server.join().unwrap().len(), 2);
        Ok(())
    }

    /// Reports malformed selectors before input or credential access.
    #[test]
    fn rejects_conflicting_and_duplicate_selectors() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        assert!(
            test.eval(
                "[] | jev annotate {q: {type: noul}} --state message --fields [message] --dry-run"
            )
            .is_err()
        );
        for flags in [
            "-s message --fields [message]",
            "--state message -f [message]",
            "-s message -f [message]",
        ] {
            assert!(
                test.eval(&format!(
                    "[] | jev annotate {{q: {{type: noul}}}} {flags} --dry-run"
                ))
                .is_err()
            );
        }
        assert!(
            test.eval("[] | jev annotate {q: {type: noul}} --fields [message message] --dry-run")
                .is_err()
        );
        assert!(
            test.eval("[] | jev annotate {q: {type: noul}} --fields [] --dry-run")
                .is_err()
        );
        assert!(
            test.eval("[] | jev annotate {q: {type: noul}} --fields [message 3] --dry-run")
                .is_err()
        );
        assert!(
            test.eval("[] | jev annotate {q: {type: noul}} --into ai --meta ai --dry-run")
                .is_err()
        );
        assert!(
            test.eval("[] | jev annotate {q: {type: noul}} --into jev_meta --dry-run")
                .is_err()
        );
        assert!(
            test.eval("[] | jev annotate {q: {type: noul}} --into jev_metrics --metrics")
                .is_err()
        );
        assert!(
            test.eval("[] | jev annotate {q: {type: noul}} --metrics --dry-run")
                .is_err()
        );
        assert!(
            test.eval("[] | jev annotate {q: {type: noul}} --into jev_metrics --dry-run")
                .is_ok()
        );
        Ok(())
    }

    /// Checks enabled output fields before projecting the outbound state.
    #[test]
    fn metrics_collision_is_terminal_even_when_field_is_not_sent() {
        let span = Span::test_data();
        let mut source = Record::new();
        source.push("message", Value::test_string("hello"));
        source.push("jev_metrics", Value::test_string("existing"));
        let options = super::TableOptions {
            selector: super::StateSelector::Fields(vec!["message".into()]),
            context: None,
            into: "jev".into(),
            metrics: true,
            on_error: super::ErrorPolicy::Keep,
            unordered: false,
            dry_run: false,
        };
        let raw = crate::nu::value::from_json(json!({"q": {"type": "noul"}}), span).unwrap();
        let questions = crate::api::validate::parse_questions(&raw).unwrap();
        let error = super::request_builder(&options, "jev-latest".into(), questions)(
            &Value::record(source, span),
        )
        .unwrap_err();
        assert!(matches!(error, JevError::FieldCollision(_)));
    }

    /// Preserves failed source rows or adds a classified record without sending HTTP.
    #[test]
    fn row_error_policies_keep_and_record() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let keep = test
            .eval(concat!(
                "[{id: 1}] | jev annotate {q: {type: noul}} ",
                "--fields [message] --on-error keep --dry-run"
            ))?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&keep).unwrap(), json!([{"id": 1}]));
        let record = test
            .eval(concat!(
                "[{id: 1}] | jev annotate {q: {type: noul}} ",
                "--fields [message] --on-error record --dry-run"
            ))?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&record).unwrap()[0]["jev_error"]["kind"], "state");
        assert_eq!(
            to_json(&record).unwrap()[0]["jev_error"]["status"],
            serde_json::Value::Null
        );
        Ok(())
    }

    /// Keeps dotted projection names literal and follows indexed paths only for --state.
    #[test]
    fn literal_fields_and_indexed_state_paths() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let literal = test
            .eval(concat!(
                "[{'a.b': 'value', id: 1}] | jev annotate {q: {type: noul}} ",
                "--fields ['a.b'] --dry-run"
            ))?
            .into_value(Span::test_data())?;
        assert_eq!(
            to_json(&literal).unwrap()[0]["request"]["state"],
            json!({"a.b": "value"})
        );
        let indexed = test
            .eval(concat!(
                "[{payload: [{text: 'first'} {text: 'second'}]}] ",
                "| jev annotate {q: {type: noul}} --state payload.1.text --dry-run"
            ))?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&indexed).unwrap()[0]["request"]["state"], "second");
        Ok(())
    }

    /// Equivalent projected states reuse one result while retaining different source IDs.
    #[test]
    fn projected_duplicates_share_usage_and_request_identity() -> Result<(), Box<ShellError>> {
        let response = mock_noul_response();
        let (base_url, server) = serve(vec![response]);
        let mut test = plugin_test()?;
        let source = format!(
            concat!(
                "$env.TYPESAFE_API_KEY = 'local-key'; ",
                "[{{id: 1, message: 'same'}} {{id: 2, message: 'same'}}] ",
                "| jev annotate {{q: {{type: noul}}}} --fields [message] ",
                "--metrics --base-url '{base_url}'"
            ),
            base_url = base_url
        );
        let result = test.eval(&source)?.into_value(Span::test_data())?;
        let wire = to_json(&result).unwrap();
        assert_eq!(wire[0]["id"], 1);
        assert_eq!(wire[1]["id"], 2);
        assert_eq!(
            wire[0]["jev_metrics"]["request_id"],
            wire[1]["jev_metrics"]["request_id"]
        );
        assert_eq!(wire[0]["jev_meta"], wire[1]["jev_meta"]);
        assert_eq!(wire[0]["jev_metrics"], wire[1]["jev_metrics"]);
        assert_eq!(wire[0]["jev_meta"]["base_url"], format!("{base_url}/"));
        assert_eq!(wire[0]["jev_metrics"]["attempts"], 1);
        assert!(wire[0]["jev_metrics"]["request_bytes"].as_u64().unwrap() > 0);
        assert!(wire[0]["jev_metrics"]["response_bytes"].as_u64().unwrap() > 0);
        let unique_usage = wire
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["jev_metrics"]["request_id"].as_str().unwrap(),
                    row["jev_meta"]["usage"]["input_tokens"].as_u64().unwrap(),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(unique_usage.values().sum::<u64>(), 10);
        assert_eq!(server.join().unwrap().len(), 1);
        Ok(())
    }

    /// Reads fresh caller credentials and service configuration on each invocation.
    #[test]
    fn later_invocation_uses_changed_caller_snapshot() -> Result<(), Box<ShellError>> {
        let response = mock_noul_response();
        let (first_url, first_server) = serve(vec![response.clone()]);
        let (second_url, second_server) = serve(vec![response]);
        let mut test = plugin_test()?;
        for (key, url) in [("first-key", &first_url), ("second-key", &second_url)] {
            let source = format!(
                concat!(
                    "$env.TYPESAFE_API_KEY = '{key}'; ",
                    "$env.NU_PLUGIN_JEV_BASE_URL = '{url}'; ",
                    "[{{message: 'same'}}] | jev annotate ",
                    "{{q: {{type: noul}}}} --fields [message]"
                ),
                key = key,
                url = url
            );
            let result = test.eval(&source)?.into_value(Span::test_data())?;
            let wire = to_json(&result).unwrap();
            assert_eq!(wire[0]["answers"]["q"]["noul"], 0.875);
            assert_eq!(wire[0]["jev_meta"]["base_url"], format!("{url}/"));
            assert!(wire[0].get("jev_metrics").is_none());
        }
        assert_eq!(
            first_server.join().unwrap()[0].authorization.as_deref(),
            Some("Bearer first-key")
        );
        assert_eq!(
            second_server.join().unwrap()[0].authorization.as_deref(),
            Some("Bearer second-key")
        );
        Ok(())
    }

    /// Converts only the selected state, leaving unsupported unselected fields untouched.
    #[test]
    fn projection_excludes_unsupported_source_values() {
        let mut source = Record::new();
        source.push("message", Value::test_string("hello"));
        source.push("elapsed", Value::test_duration(1_500_000_000));
        source.push("blob", Value::test_binary(vec![0, 1, 2]));
        let source = Value::test_record(source);
        let questions = crate::api::validate::parse_questions(&Value::test_record({
            let mut record = Record::new();
            record.push(
                "q",
                crate::nu::value::from_json(json!({"type": "noul"}), Span::test_data()).unwrap(),
            );
            record
        }))
        .unwrap();
        let options = super::TableOptions {
            selector: super::StateSelector::Fields(vec!["message".into(), "elapsed".into()]),
            context: None,
            into: "jev".into(),
            metrics: false,
            on_error: super::ErrorPolicy::Fail,
            unordered: false,
            dry_run: true,
        };
        let request =
            super::request_builder(&options, "jev-latest".into(), questions.clone())(&source)
                .unwrap();
        assert_eq!(
            request.state,
            json!({"message": "hello", "elapsed": "1500000000ns"})
        );
        let whole = super::TableOptions {
            selector: super::StateSelector::Whole,
            ..options
        };
        assert!(super::request_builder(&whole, "jev-latest".into(), questions)(&source).is_err());
    }

    /// Leaves duplicate keys in an unselected row field outside outbound validation.
    #[test]
    fn state_selection_ignores_unselected_duplicate_keys() {
        let mut duplicate = Record::new();
        duplicate.push("same", Value::test_int(1));
        duplicate.push("same", Value::test_int(2));
        let mut source = Record::new();
        source.push("message", Value::test_string("hello"));
        source.push("ignored", Value::test_record(duplicate));
        let source = Value::test_record(source);
        let questions = crate::api::validate::parse_questions(
            &crate::nu::value::from_json(json!({"q": {"type": "noul"}}), Span::test_data())
                .unwrap(),
        )
        .unwrap();
        let options = super::TableOptions {
            selector: super::StateSelector::Cell("message".parse().unwrap()),
            context: None,
            into: "answers".into(),
            metrics: false,
            on_error: super::ErrorPolicy::Fail,
            unordered: false,
            dry_run: true,
        };

        let request =
            super::request_builder(&options, "jev-latest".into(), questions.clone())(&source)
                .unwrap();
        assert_eq!(request.state, json!("hello"));
        let whole = super::TableOptions {
            selector: super::StateSelector::Whole,
            ..options
        };
        assert!(super::request_builder(&whole, "jev-latest".into(), questions)(&source).is_err());
    }

    /// Reuses one body builder for previews and live requests with the same context.
    #[test]
    fn streaming_preview_matches_live_request_body() -> Result<(), Box<ShellError>> {
        let response = mock_noul_response();
        let (base_url, server) = serve(vec![response]);
        let mut test = plugin_test()?;
        let common = format!(
            concat!(
                "[{{id: 7, message: 'hello', secret: 'local'}}] | jev annotate ",
                "{{q: {{type: noul, instructions: {{task: 'greet'}}}}}} ",
                "--fields [message] --context {{policy: ['first' 'second']}} ",
                "--base-url '{base_url}'"
            ),
            base_url = base_url
        );
        let preview = test
            .eval(&format!("{common} --dry-run"))?
            .into_value(Span::test_data())?;
        let live = test
            .eval(&format!("$env.TYPESAFE_API_KEY = 'local-key'; {common}"))?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&live).unwrap()[0]["id"], 7);
        assert_eq!(
            server.join().unwrap()[0].body,
            to_json(&preview).unwrap()[0]["request"]
        );
        Ok(())
    }

    /// Applies keep/record after a malformed live response without caching the failure.
    #[test]
    fn live_response_failures_follow_row_policy() -> Result<(), Box<ShellError>> {
        for policy in ["keep", "record"] {
            let (base_url, server) = serve(vec![json!({"malformed": true})]);
            let mut test = plugin_test()?;
            let source = format!(
                concat!(
                    "$env.TYPESAFE_API_KEY = 'local-key'; ",
                    "[{{id: 1, message: 'hello'}}] | jev annotate ",
                    "{{q: {{type: noul}}}} --on-error {policy} ",
                    "--metrics --base-url '{base_url}'"
                ),
                policy = policy,
                base_url = base_url
            );
            let result = test.eval(&source)?.into_value(Span::test_data())?;
            let wire = to_json(&result).unwrap();
            assert_eq!(wire[0]["id"], 1);
            assert!(wire[0].get("answers").is_none());
            assert!(wire[0].get("jev_meta").is_none());
            assert!(wire[0].get("jev_metrics").is_none());
            if policy == "record" {
                assert_eq!(wire[0]["jev_error"]["kind"], "response");
            } else {
                assert!(wire[0].get("jev_error").is_none());
            }
            assert_eq!(server.join().unwrap().len(), 1);
        }
        Ok(())
    }

    /// Treats one record as one row and an empty list as an empty output stream.
    #[test]
    fn accepts_single_and_empty_tables() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let single = test
            .eval("{message: 'hello'} | jev annotate {q: {type: noul}} --dry-run")?
            .into_value(Span::test_data())?;
        assert_eq!(
            to_json(&single).unwrap()[0]["request"]["state"],
            json!({"message": "hello"})
        );
        let empty = test
            .eval("[] | jev annotate {q: {type: noul}} --dry-run")?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&empty).unwrap(), json!([]));
        Ok(())
    }

    /// Emits a preview for every original row even when their requests are identical.
    #[test]
    fn dry_run_preserves_duplicate_rows_in_input_order() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let result = test
            .eval(concat!(
                "[{id: 1, message: 'same'} {id: 2, message: 'same'}] ",
                "| jev annotate {q: {type: noul}} --fields [message] --dry-run"
            ))?
            .into_value(Span::test_data())?;
        let wire = to_json(&result).unwrap();
        assert_eq!(wire.as_array().unwrap().len(), 2);
        assert_eq!(wire[0], wire[1]);
        assert_eq!(wire[0]["request"]["state"], json!({"message": "same"}));
        Ok(())
    }

    /// Records selector misses but terminates on a destination collision.
    #[test]
    fn selector_misses_and_collisions_do_not_issue_requests() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let missing = test
            .eval(concat!(
                "[{id: 1}] | jev annotate {q: {type: noul}} ",
                "--state payload.text --on-error record --dry-run"
            ))?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&missing).unwrap()[0]["jev_error"]["kind"], "state");
        for policy in ["keep", "record"] {
            let collision = test.eval(&format!(
                concat!(
                    "[{{id: 1, message: 'hello', answers: 'existing'}}] ",
                    "| jev annotate {{q: {{type: noul}}}} --fields [message] ",
                    "--on-error {policy} --dry-run"
                ),
                policy = policy
            ))?;
            assert!(collision.into_value(Span::test_data()).is_err());
        }
        let legacy = test
            .eval(concat!(
                "[{id: 1, message: 'hello', jev: 'existing'}] ",
                "| jev annotate {q: {type: noul}} --fields [message] --dry-run"
            ))?
            .into_value(Span::test_data())?;
        assert_eq!(
            to_json(&legacy).unwrap()[0]["request"]["state"],
            json!({"message": "hello"})
        );
        let meta_collision = test.eval(concat!(
            "[{id: 1, message: 'hello', jev_meta: 'existing'}] ",
            "| jev annotate {q: {type: noul}} --fields [message] ",
            "--on-error keep --dry-run"
        ))?;
        assert!(meta_collision.into_value(Span::test_data()).is_err());
        let missing_field = test
            .eval(concat!(
                "[{id: 1}] | jev annotate {q: {type: noul}} ",
                "--fields [message] --on-error record --dry-run"
            ))?
            .into_value(Span::test_data())?;
        assert_eq!(
            to_json(&missing_field).unwrap()[0]["jev_error"]["kind"],
            "state"
        );
        Ok(())
    }

    /// Stops a long offline preview when downstream takes only a bounded prefix.
    #[test]
    fn dry_run_downstream_truncation() -> Result<(), Box<ShellError>> {
        let consumed = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&consumed);
        let input = Box::new((0..100_000).map(move |id| {
            counter.fetch_add(1, Ordering::Relaxed);
            let mut record = Record::with_capacity(1);
            record.push("message", Value::test_int(id));
            Value::test_record(record)
        }));
        let options = super::TableOptions {
            selector: super::StateSelector::Fields(vec!["message".into()]),
            context: None,
            into: "jev".into(),
            metrics: false,
            on_error: super::ErrorPolicy::Fail,
            unordered: false,
            dry_run: true,
        };
        let questions = BTreeMap::from([(
            "q".into(),
            Question::Noul {
                instructions: None,
                criteria: None,
            },
        )]);
        let build = super::request_builder(&options, "jev-latest".into(), questions);
        let rows = super::preview_rows(input, build, options.on_error, Span::test_data())
            .take(10)
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 10);
        assert_eq!(consumed.load(Ordering::Relaxed), 10);
        Ok(())
    }

    /// Includes numeric HTTP status in a complete classified row error.
    #[test]
    fn error_record_has_kind_message_and_status() {
        let row = Value::test_record({
            let mut record = Record::new();
            record.push("id", Value::test_int(1));
            record
        });
        let wire = to_json(&super::add_error(
            row,
            &JevError::Http { status: 422 },
            Span::test_data(),
        ))
        .unwrap();
        assert_eq!(wire["id"], 1);
        assert_eq!(wire["jev_error"]["kind"], "http");
        assert_eq!(wire["jev_error"]["message"], "Jev returned HTTP 422");
        assert_eq!(wire["jev_error"]["status"], 422);
        assert_eq!(wire["jev_error"].as_object().unwrap().len(), 3);
    }

    /// Classifies non-record and upstream error rows before any outbound request.
    #[test]
    fn nonrecord_and_upstream_rows_fail_before_http() {
        let options = super::TableOptions {
            selector: super::StateSelector::Whole,
            context: None,
            into: "jev".into(),
            metrics: false,
            on_error: super::ErrorPolicy::Fail,
            unordered: false,
            dry_run: false,
        };
        let questions = crate::api::validate::parse_questions(
            &crate::nu::value::from_json(json!({"q": {"type": "noul"}}), Span::test_data())
                .unwrap(),
        )
        .unwrap();
        let builder = super::request_builder(&options, "jev-latest".into(), questions);
        let nonrecord = builder(&Value::test_int(3)).unwrap_err();
        assert_eq!(nonrecord.kind_name(), "state");
        let upstream = Value::error(
            ShellError::from(nu_protocol::LabeledError::new("upstream")),
            Span::test_data(),
        );
        let error = builder(&upstream).unwrap_err();
        assert_eq!(error.kind_name(), "state");
        assert_eq!(error.to_string(), "upstream Nushell row error");
    }

    /// Keeps every documented help example executable without a credential.
    #[test]
    fn help_examples_run_offline() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        for example in super::JevAnnotate.examples() {
            let result = test.eval(example.example)?.into_value(Span::test_data())?;
            assert_eq!(to_json(&result).unwrap().as_array().unwrap().len(), 1);
        }
        Ok(())
    }
}
