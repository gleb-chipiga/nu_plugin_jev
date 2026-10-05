//! Evaluates one finite Nushell state against multiple named questions.

use nu_plugin::{EngineInterface, EvaluatedCall, PluginCommand};
use nu_protocol::{
    Example, LabeledError, PipelineData, Record, Signature, SyntaxShape, Type, Value,
};

use crate::{api::validate::parse_questions, nu::typed::answers_to_nu, plugin::JevPlugin};

use super::evaluate::{Evaluation, evaluate, evaluation_meta, measurement_value, preview_value};

/// Sends one structured state and a nonempty question record to System One.
pub(crate) struct JevAsk;

impl PluginCommand for JevAsk {
    type Plugin = JevPlugin;

    /// Names the multi-question evaluation command.
    fn name(&self) -> &str {
        "jev ask"
    }

    /// Accepts a question record and shared evaluation settings.
    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_type(Type::Any, Type::record())
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
                "Total request deadline",
                None,
            )
            .named(
                "config",
                SyntaxShape::String,
                "Explicit local TOML file",
                None,
            )
            .switch(
                "dry-run",
                "Return {request, request_bytes} without HTTP",
                None,
            )
            .switch("metrics", "Include successful HTTP measurements", None)
    }

    /// Summarizes the single-state, multi-question primitive.
    fn description(&self) -> &str {
        "Evaluate one structured state against named Jev questions"
    }

    /// Explains that a stream is intentionally collected as one array state.
    fn extra_description(&self) -> &str {
        concat!(
            "A finite input stream becomes one JSON array state and one API request. ",
            "Live success returns {answers, meta: {base_url, model, usage}}; ",
            "--metrics adds HTTP-only metrics. --context wraps the state as {input, context}; ",
            "--dry-run returns {request, request_bytes} without an API key. ",
            "The byte count covers only the compact JSON body. Defaults are resolved per call ",
            "from flags, Nu config, caller environment, selected local TOML (--config or ",
            "NU_PLUGIN_JEV_CONFIG, otherwise .nu_plugin_jev.toml), user TOML, then built-ins."
        )
    }

    /// Shows the ordinary one-state usage.
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example {
            example: concat!(
                "'hello' | jev ask {greeting: (jev question noul 'Is this a greeting?')} ",
                "-c {policy: 'greetings'} -m jev-latest --dry-run"
            ),
            description: "Preview a named Noul question without sending data",
            result: None,
        }]
    }

    /// Validates a finite state and returns either the preview or full API envelope.
    fn run(
        &self,
        plugin: &JevPlugin,
        engine: &EngineInterface,
        call: &EvaluatedCall,
        input: PipelineData,
    ) -> Result<PipelineData, LabeledError> {
        let questions: Value = call.req(0).map_err(LabeledError::from)?;
        let questions = parse_questions(&questions)?;
        let metrics = call.has_flag("metrics").map_err(LabeledError::from)?;
        if metrics && call.has_flag("dry-run").map_err(LabeledError::from)? {
            return Err(LabeledError::new("--metrics requires a live Jev request")
                .with_label("remove --metrics or --dry-run", call.head));
        }
        let state = match input {
            PipelineData::Empty => {
                return Err(LabeledError::new("jev ask requires an input state")
                    .with_label("missing pipeline input", call.head));
            }
            PipelineData::Value(value, _) => value,
            PipelineData::ListStream(stream, _) => {
                Value::list(stream.into_iter().collect(), call.head)
            }
            PipelineData::ByteStream(_, _) => {
                return Err(LabeledError::new("byte streams are not Jev states")
                    .with_label("decode the stream to a string, record, or list", call.head));
            }
        };
        let result = match evaluate(plugin, engine, call, &state, questions)? {
            Evaluation::Preview(request) => preview_value(request, call.head)?,
            Evaluation::Response(success) => {
                let mut result = Record::with_capacity(if metrics { 3 } else { 2 });
                result.push(
                    "answers",
                    answers_to_nu(&success.response.answers, call.head)?,
                );
                result.push(
                    "meta",
                    evaluation_meta(&success.base_url, &success.response, call.head)?,
                );
                if metrics {
                    result.push(
                        "metrics",
                        measurement_value(&success.measurement, None, call.head)?,
                    );
                }
                Value::record(result, call.head)
            }
        };
        Ok(PipelineData::value(result, None))
    }
}

#[cfg(test)]
mod tests {
    use nu_plugin::PluginCommand;
    use nu_plugin_test_support::PluginTest;
    use nu_protocol::{ListStream, PipelineData, ShellError, Signals, Span, Value};
    use serde_json::json;

    use crate::{commands::tests::serve, nu::value::to_json, plugin::JevPlugin};

    /// Starts a fresh plugin engine without credentials or external requests.
    fn plugin_test() -> Result<PluginTest, Box<ShellError>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        PluginTest::new("jev", JevPlugin::new(runtime).into()).map_err(Box::new)
    }

    /// Keeps the documented short-form help example executable without a key.
    #[test]
    fn help_example_runs_offline() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        for example in super::JevAsk.examples() {
            let result = test.eval(example.example)?.into_value(Span::test_data())?;
            assert_eq!(to_json(&result).unwrap()["request"]["model"], "jev-latest");
        }
        Ok(())
    }

    /// Preserves a mixed question map and structured object state in the preview.
    #[test]
    fn preview_mixed_questions_and_context() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let result = test
            .eval(concat!(
                "{message: 'hello'} | jev ask {spam: {type: noul, ",
                "instructions: {task: 'spam'}, criteria: {'true': null}}, ",
                "kind: {type: choice, criteria: {normal: null, spam: 'bulk'}}, ",
                "urgency: {type: score, criteria: ['later' 'now']}} ",
                "--context null --dry-run"
            ))?
            .into_value(Span::test_data())?;
        let preview = to_json(&result).unwrap();
        assert_eq!(
            preview["request"],
            json!({"model": "jev-latest", "state": {"input": {"message": "hello"}, "context": null},
            "questions": {
                "spam": {
                    "type": "noul",
                    "instructions": {"task": "spam"},
                    "criteria": {"true": null}
                },
                "kind": {"type": "choice", "criteria": {"normal": null, "spam": "bulk"}},
                "urgency": {"type": "score", "criteria": ["later", "now"]}
            }})
        );
        assert_eq!(
            preview["request_bytes"],
            json!(serde_json::to_vec(&preview["request"]).unwrap().len())
        );
        Ok(())
    }

    /// Rejects an unknown raw question field before creating a request preview.
    #[test]
    fn raw_question_typo_is_not_silently_ignored() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        assert!(
            test.eval("'hello' | jev ask {q: {type: noul, instrucitons: 'typo'}} --dry-run")
                .is_err()
        );
        Ok(())
    }

    /// Keeps short and long options equivalent across previews and live dispatch.
    #[test]
    fn short_options_match_long_options_and_override_configuration() -> Result<(), Box<ShellError>>
    {
        let response = json!({"model": "jev-fixed", "answers": {
            "q": {"type": "noul", "noul": 0.75}},
            "usage": {"input_tokens": 8, "output_tokens": 1}});
        let (base_url, server) = serve(vec![response]);
        let mut test = plugin_test()?;
        let setup = concat!(
            "$env.NU_PLUGIN_JEV_MODEL = 'env-model'; ",
            "$env.config.plugins.jev = {model: 'config-model'};"
        );
        let common = format!("'hello' | jev ask {{q: {{type: noul}}}} --base-url '{base_url}'");
        let long = test
            .eval(&format!(
                "{setup} {common} --context {{policy: 'greetings'}} --model flag-model --dry-run"
            ))?
            .into_value(Span::test_data())?;
        let short = test
            .eval(&format!(
                "{setup} {common} -c {{policy: 'greetings'}} -m flag-model --dry-run"
            ))?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&short).unwrap(), to_json(&long).unwrap());
        assert_eq!(to_json(&short).unwrap()["request"]["model"], "flag-model");

        let live = test
            .eval(&format!(
                concat!(
                    "{setup} $env.TYPESAFE_API_KEY = 'local'; {common} ",
                    "-c {{policy: 'greetings'}} -m flag-model"
                ),
                setup = setup,
                common = common
            ))?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&live).unwrap()["answers"]["q"]["noul"], 0.75);
        assert_eq!(
            server.join().unwrap()[0].body,
            to_json(&long).unwrap()["request"]
        );
        Ok(())
    }

    /// Treats a finite list stream as one array state and distinguishes no input.
    #[test]
    fn collects_stream_as_one_state() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let span = Span::test_data();
        let input = PipelineData::list_stream(
            ListStream::new(
                vec![Value::test_string("a"), Value::test_string("b")].into_iter(),
                span,
                Signals::empty(),
            ),
            None,
        );
        let result = test
            .eval_with("jev ask {match: {type: noul}} --dry-run", input)?
            .into_value(Span::test_data())?;
        assert_eq!(
            to_json(&result).unwrap()["request"]["state"],
            json!(["a", "b"])
        );
        assert!(
            test.eval("jev ask {match: {type: noul}} --dry-run")
                .is_err()
        );
        Ok(())
    }

    /// Sends mixed questions in one request and returns the complete answer envelope.
    #[test]
    fn live_mixed_questions_use_one_http_request() -> Result<(), Box<ShellError>> {
        let answer = json!({"model": "jev-2026-09", "answers": {
            "spam": {"type": "noul", "noul": 0.982},
            "kind": {"type": "choice", "choice": "spam", "confidence": 0.91,
                "probabilities": {"normal": 0.09, "spam": 0.91}},
            "urgency": {"type": "score", "score": 1.4, "confidence": 0.81,
                "legend": {"0": "later", "1": "today", "2": "now"},
                "probabilities": {"0": 0.1, "1": 0.4, "2": 0.5}}
        }, "usage": {"input_tokens": 42, "output_tokens": 6}});
        let (base_url, server) = serve(vec![answer.clone()]);
        let mut test = plugin_test()?;
        let source = format!(
            "$env.TYPESAFE_API_KEY = 'local-key'; $env.NU_PLUGIN_JEV_BASE_URL = '{base_url}';"
        ) + r#"
let questions = {
    spam: (jev question noul "Is this unsolicited?" --yes "Unrequested bulk mail")
    kind: (jev question choice "Message kind?" [normal promo spam])
    urgency: (jev question score "Review urgency?" ["later" "today" "now"])
}
let message = "Hello"
let sender = "Ada"
let result = ({message: $message, sender: $sender} | jev ask $questions)
$result
"#;
        let result = test.eval(&source)?.into_value(Span::test_data())?;
        assert_eq!(
            to_json(&result).unwrap(),
            json!({
                "answers": answer["answers"],
                "meta": {
                    "base_url": format!("{base_url}/"),
                    "model": answer["model"],
                    "usage": answer["usage"]
                }
            })
        );
        let captured = server.join().expect("mock server thread");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].method, "POST");
        assert_eq!(captured[0].path, "/v1/systemone");
        assert_eq!(
            captured[0].authorization.as_deref(),
            Some("Bearer local-key")
        );
        assert_eq!(
            captured[0].body["state"],
            json!({"message": "Hello", "sender": "Ada"})
        );
        assert_eq!(captured[0].body["questions"].as_object().unwrap().len(), 3);
        Ok(())
    }

    /// Keeps provenance always visible and reports HTTP-only metrics on request.
    #[test]
    fn live_metrics_match_preview_bytes_and_use_nu_durations() -> Result<(), Box<ShellError>> {
        let response = json!({
            "model": "jev-fixed",
            "answers": {"q": {"type": "noul", "noul": 0.75}},
            "usage": {"input_tokens": 4, "output_tokens": 1}
        });
        let response_bytes = response.to_string().len();
        let (base_url, server) = serve(vec![response]);
        let mut test = plugin_test()?;
        let question = "{q: {type: noul}}";
        let preview = test
            .eval(&format!("'hello' | jev ask {question} --dry-run"))?
            .into_value(Span::test_data())?;
        let live = test
            .eval(&format!(
                concat!(
                    "$env.TYPESAFE_API_KEY = 'local-key'; 'hello' | jev ask {question} ",
                    "--metrics --base-url '{base_url}'"
                ),
                question = question,
                base_url = base_url
            ))?
            .into_value(Span::test_data())?;
        let Value::Record { val, .. } = &live else {
            panic!("ask result is a record");
        };
        let Some(Value::Record { val: metrics, .. }) = val.get("metrics") else {
            panic!("ask metrics is a record");
        };
        assert!(matches!(
            metrics.get("elapsed"),
            Some(Value::Duration { .. })
        ));
        assert!(matches!(
            metrics.get("attempt_elapsed"),
            Some(Value::Duration { .. })
        ));
        let preview = to_json(&preview).unwrap();
        let live = to_json(&live).unwrap();
        assert_eq!(live["answers"]["q"]["noul"], 0.75);
        assert_eq!(live["meta"]["base_url"], format!("{base_url}/"));
        assert_eq!(live["meta"]["usage"]["input_tokens"], 4);
        assert_eq!(live["metrics"]["request_bytes"], preview["request_bytes"]);
        assert_eq!(live["metrics"]["response_bytes"], response_bytes);
        assert_eq!(live["metrics"]["attempts"], 1);
        let expected_version = if cfg!(feature = "http2-prior-knowledge") {
            "HTTP/2"
        } else {
            "HTTP/1.1"
        };
        assert_eq!(live["metrics"]["http_version"], expected_version);
        assert_eq!(
            live["metrics"]["elapsed"],
            live["metrics"]["attempt_elapsed"]
        );
        assert!(live["metrics"].get("base_url").is_none());
        assert!(live["metrics"]["elapsed"].as_str().unwrap().ends_with("ns"));
        assert!(
            test.eval(&format!("'hello' | jev ask {question} --metrics --dry-run"))
                .is_err()
        );
        assert_eq!(server.join().unwrap().len(), 1);
        Ok(())
    }

    /// Makes the offline preview byte-equivalent in JSON structure to a live body.
    #[test]
    fn preview_matches_live_body_without_sending_twice() -> Result<(), Box<ShellError>> {
        let (base_url, server) = serve(vec![json!({"model": "jev-fixed", "answers": {
            "match": {"type": "noul", "noul": 0.75}},
            "usage": {"input_tokens": 8, "output_tokens": 1}})]);
        let mut test = plugin_test()?;
        let common = format!(
            concat!(
                "{{message: 'hello'}} | jev ask {{match: {{type: noul, ",
                "instructions: {{task: 'judge'}}, criteria: {{'true': null}}}}}} ",
                "--context {{policy: ['first' 'second']}} --base-url '{base_url}'"
            ),
            base_url = base_url
        );
        let preview = test
            .eval(&format!("{common} --dry-run"))?
            .into_value(Span::test_data())?;
        assert!(to_json(&preview).unwrap().get("authorization").is_none());
        let live = test
            .eval(&format!("$env.TYPESAFE_API_KEY = 'local'; {common}"))?
            .into_value(Span::test_data())?;
        assert_eq!(
            to_json(&live).unwrap()["answers"]["match"]["noul"],
            json!(0.75)
        );
        let captured = server.join().expect("mock server thread");
        assert_eq!(captured.len(), 1);
        let preview = to_json(&preview).unwrap();
        assert_eq!(captured[0].body, preview["request"]);
        assert_eq!(
            preview["request_bytes"],
            json!(serde_json::to_vec(&captured[0].body).unwrap().len())
        );
        Ok(())
    }

    /// Rejects an empty caller credential before attempting a live request.
    #[test]
    fn empty_live_key_is_a_labeled_error() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        assert!(
            test.eval("$env.TYPESAFE_API_KEY = ''; 'hello' | jev ask {match: {type: noul}}")
                .is_err()
        );
        Ok(())
    }

    /// Applies caller-scoped proxy changes between calls and hides them in previews.
    #[test]
    fn proxy_policy_changes_between_invocations() -> Result<(), Box<ShellError>> {
        let answer = json!({"model": "jev-fixed", "answers": {
            "match": {"type": "noul", "noul": 0.75}},
            "usage": {"input_tokens": 8, "output_tokens": 1}});
        let (first_proxy, first_server) = serve(vec![answer.clone()]);
        let (second_proxy, second_server) = serve(vec![answer]);
        let mut test = plugin_test()?;
        let source = format!(
            "$env.TYPESAFE_API_KEY = 'local'; $env.NU_PLUGIN_JEV_BASE_URL = 'http://localhost:1'; \
             $env.NU_PLUGIN_JEV_PROXY = '{first_proxy}'; \
             let first = ('hello' | jev ask {{match: {{type: noul}}}}); \
             $env.NU_PLUGIN_JEV_PROXY = '{second_proxy}'; \
             let second = ('world' | jev ask {{match: {{type: noul}}}}); \
             [$first $second]"
        );
        let result = test.eval(&source)?.into_value(Span::test_data())?;
        assert_eq!(to_json(&result).unwrap().as_array().unwrap().len(), 2);
        let first = first_server.join().unwrap();
        let second = second_server.join().unwrap();
        assert_eq!(first[0].path, "/v1/systemone");
        assert_eq!(second[0].path, "/v1/systemone");
        assert_eq!(first[0].authority.as_deref(), Some("localhost:1"));
        assert_eq!(second[0].authority.as_deref(), Some("localhost:1"));

        let preview = test
            .eval(concat!(
                "$env.NU_PLUGIN_JEV_PROXY = 'http://user:secret@127.0.0.1:1'; ",
                "'hello' | jev ask {match: {type: noul}} --dry-run"
            ))?
            .into_value(Span::test_data())?;
        assert!(!to_json(&preview).unwrap().to_string().contains("secret"));
        let failure = test
            .eval(concat!(
                "$env.NU_PLUGIN_JEV_PROXY = 'http://user:secret@127.0.0.1:1/path'; ",
                "'hello' | jev ask {match: {type: noul}} --dry-run"
            ))
            .unwrap_err();
        assert!(!failure.to_string().contains("secret"));
        Ok(())
    }
}
