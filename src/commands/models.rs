//! Lists the service's current model metadata as ordinary Nushell records.

use nu_plugin::{EngineInterface, EvaluatedCall, PluginCommand};
use nu_protocol::{
    Example, LabeledError, PipelineData, Record, SignalAction, Signature, SyntaxShape, Value,
};

use crate::{
    api::cancel::CancelHandle,
    config::{ConfigScope, capture_sources, require_api_key, resolve_models},
    nu::cache::next_request_id,
    plugin::JevPlugin,
    tracing::trace_models,
};

use super::evaluate::measurement_value;

/// Fetches the current catalog without accepting pipeline state or questions.
pub(crate) struct JevModels;

impl PluginCommand for JevModels {
    type Plugin = JevPlugin;

    /// Names the model-discovery command.
    fn name(&self) -> &str {
        "jev models"
    }

    /// Accepts only transport and file-selection flags.
    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .named(
                "base-url",
                SyntaxShape::String,
                "TypeSafe API service root",
                None,
            )
            .named(
                "timeout",
                SyntaxShape::Duration,
                "Total request deadline",
                None,
            )
            .named(
                "config",
                SyntaxShape::String,
                "Explicit local TOML file",
                None,
            )
            .switch("metrics", "Include successful HTTP measurements", None)
    }

    /// Summarizes the uncached model lookup.
    fn description(&self) -> &str {
        "List models currently available from TypeSafe Jev"
    }

    /// Explains authentication, output, and the absence of implicit evaluation.
    fn extra_description(&self) -> &str {
        "Requires TYPESAFE_API_KEY or a private configured API key. Sends one authenticated GET /v1/models (plus configured retries), returning {models, meta: {base_url}}; --metrics adds HTTP measurements with request_bytes = 0. Model records preserve name, description, and release_date strings. Results are not cached and do not validate models used by jev ask or jev annotate."
    }

    /// Shows native Nushell filtering of model metadata.
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example {
            example: "jev models | get models | where name =~ '^jev' | select name release_date",
            description: "Inspect current Jev model names and release dates",
            result: None,
        }]
    }

    /// Rejects pipeline input before configuration or HTTP and returns ordered records.
    fn run(
        &self,
        plugin: &JevPlugin,
        engine: &EngineInterface,
        call: &EvaluatedCall,
        input: PipelineData,
    ) -> Result<PipelineData, LabeledError> {
        if !matches!(input, PipelineData::Empty) {
            return Err(
                LabeledError::new("jev models does not accept pipeline input")
                    .with_label("invoke jev models without an input state", call.head),
            );
        }
        let metrics = call.has_flag("metrics").map_err(LabeledError::from)?;
        let sources = capture_sources(engine, call, ConfigScope::Models)?;
        let config = resolve_models(&sources)?;
        let key = require_api_key(&sources)?;
        let client = plugin
            .client
            .for_policy(&config.proxy)
            .map_err(|error| error.into_labeled())?;
        engine
            .signals()
            .check(&call.head)
            .map_err(LabeledError::from)?;
        let (cancel, signal) = CancelHandle::new();
        let handler = cancel.clone();
        let _guard = engine
            .register_signal_handler(Box::new(move |action| {
                if action == SignalAction::Interrupt {
                    handler.cancel();
                }
            }))
            .map_err(LabeledError::from)?;
        let request_id = next_request_id();
        let list = plugin
            .runtime
            .block_on(trace_models(
                &request_id,
                client.models_measured(&config, &key, signal),
            ))
            .map_err(|error| error.into_labeled())?;
        let rows = list
            .response
            .models
            .into_iter()
            .map(|model| {
                let mut record = Record::with_capacity(3);
                record.push("name", Value::string(model.name, call.head));
                record.push("description", Value::string(model.description, call.head));
                record.push("release_date", Value::string(model.release_date, call.head));
                Value::record(record, call.head)
            })
            .collect();
        let mut metadata = Record::with_capacity(1);
        metadata.push("base_url", Value::string(list.base_url, call.head));
        let mut result = Record::with_capacity(if metrics { 3 } else { 2 });
        result.push("models", Value::list(rows, call.head));
        result.push("meta", Value::record(metadata, call.head));
        if metrics {
            result.push(
                "metrics",
                measurement_value(&list.measurement, None, call.head)?,
            );
        }
        Ok(PipelineData::value(Value::record(result, call.head), None))
    }
}

#[cfg(test)]
mod tests {
    use nu_plugin_test_support::PluginTest;
    use nu_protocol::{ListStream, PipelineData, ShellError, Signals, Span, Value};
    use serde_json::json;

    use crate::{commands::tests::serve, nu::value::to_json, plugin::JevPlugin};

    /// Creates an isolated plugin engine with the normal explicit Tokio runtime.
    fn plugin_test() -> Result<PluginTest, Box<ShellError>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        PluginTest::new("jev", JevPlugin::new(runtime).into()).map_err(Box::new)
    }

    /// Returns ordered string records and preserves an empty service catalog.
    #[test]
    fn lists_typed_models_and_empty_results() -> Result<(), Box<ShellError>> {
        let (url, server) = serve(vec![
            json!({"models": [
                {"name": "jev-latest", "description": "General", "release_date": "unknown"},
                {"name": "jev-fixed", "description": "Pinned", "release_date": "2026-09-15"}
            ]}),
            json!({"models": []}),
        ]);
        let mut test = plugin_test()?;
        let command = format!("$env.TYPESAFE_API_KEY = 'test-key'; jev models --base-url '{url}'");
        let first = test.eval(&command)?.into_value(Span::test_data())?;
        assert_eq!(
            to_json(&first).unwrap(),
            json!({
                "models": [
                    {"name": "jev-latest", "description": "General", "release_date": "unknown"},
                    {"name": "jev-fixed", "description": "Pinned", "release_date": "2026-09-15"}
                ],
                "meta": {"base_url": format!("{url}/")}
            })
        );
        let second = test.eval(&command)?.into_value(Span::test_data())?;
        assert_eq!(
            to_json(&second).unwrap(),
            json!({"models": [], "meta": {"base_url": format!("{url}/")}})
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| request.method == "GET"
            && request.path == "/v1/models"
            && request.authorization.as_deref() == Some("Bearer test-key")
            && request.body.is_null()));
        Ok(())
    }

    /// Keeps catalog provenance separate from optional bodyless GET metrics.
    #[test]
    fn metrics_wrap_empty_catalog_without_fabricated_usage() -> Result<(), Box<ShellError>> {
        let response = json!({"models": []});
        let response_bytes = response.to_string().len();
        let (url, server) = serve(vec![response]);
        let mut test = plugin_test()?;
        let value = test
            .eval(&format!(
                "$env.TYPESAFE_API_KEY = 'local-key'; jev models --base-url '{url}' --metrics"
            ))?
            .into_value(Span::test_data())?;
        let Value::Record { val, .. } = &value else {
            panic!("model listing is a record");
        };
        let Some(Value::Record { val: metrics, .. }) = val.get("metrics") else {
            panic!("model metrics is a record");
        };
        assert!(matches!(
            metrics.get("elapsed"),
            Some(Value::Duration { .. })
        ));
        assert!(matches!(
            metrics.get("attempt_elapsed"),
            Some(Value::Duration { .. })
        ));
        let wire = to_json(&value).unwrap();
        assert_eq!(wire["models"], json!([]));
        assert_eq!(wire["meta"], json!({"base_url": format!("{url}/")}));
        assert_eq!(wire["metrics"]["request_bytes"], 0);
        assert_eq!(wire["metrics"]["response_bytes"], response_bytes);
        assert_eq!(wire["metrics"]["attempts"], 1);
        assert_eq!(wire["metrics"]["http_version"], "HTTP/1.1");
        assert_eq!(
            wire["metrics"]["elapsed"],
            wire["metrics"]["attempt_elapsed"]
        );
        assert!(wire["metrics"].get("model").is_none());
        assert!(wire["metrics"].get("usage").is_none());
        assert_eq!(server.join().unwrap().len(), 1);
        Ok(())
    }

    /// Requires a live key and ignores unrelated malformed evaluation settings.
    #[test]
    fn requires_key_but_not_evaluation_settings() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        assert!(test.eval("$env.TYPESAFE_API_KEY = ''; jev models").is_err());
        let (url, server) = serve(vec![json!({"models": []})]);
        let command = format!(
            "$env.TYPESAFE_API_KEY = 'test'; $env.NU_PLUGIN_JEV_MODEL = 7; $env.NU_PLUGIN_JEV_JOBS = 'bad'; $env.config.plugins.jev = {{cache: 1}}; jev models --base-url '{url}'"
        );
        assert_eq!(
            to_json(&test.eval(&command)?.into_value(Span::test_data())?).unwrap(),
            json!({"models": [], "meta": {"base_url": format!("{url}/")}})
        );
        assert_eq!(server.join().unwrap().len(), 1);
        Ok(())
    }

    /// Rejects a lazy stream without polling it or consulting credentials.
    #[test]
    fn rejects_pipeline_input_without_consumption() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let stream = ListStream::new(
            std::iter::from_fn(|| -> Option<Value> { panic!("stream was consumed") }),
            Span::test_data(),
            Signals::empty(),
        );
        assert!(
            test.eval_with("jev models", PipelineData::list_stream(stream, None))
                .is_err()
        );
        assert!(test.eval("'state' | jev models").is_err());
        Ok(())
    }
}
