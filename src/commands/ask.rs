//! Evaluates one finite Nushell state against multiple named questions.

use nu_plugin::{EngineInterface, EvaluatedCall, PluginCommand};
use nu_protocol::{Example, LabeledError, PipelineData, Signature, SyntaxShape, Value};

use crate::{api::validate::parse_questions, plugin::JevPlugin};

use super::evaluate::{Evaluation, evaluate, to_nu};

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
                "Return the exact request body without HTTP",
                None,
            )
    }

    /// Summarizes the single-state, multi-question primitive.
    fn description(&self) -> &str {
        "Evaluate one structured state against named Jev questions"
    }

    /// Explains that a stream is intentionally collected as one array state.
    fn extra_description(&self) -> &str {
        "A finite input stream becomes one JSON array state and one API request. --context wraps the state as {input, context}; --dry-run needs no API key. Defaults are resolved per call from flags, Nu config, caller environment, selected local TOML (--config or NU_PLUGIN_JEV_CONFIG, otherwise .nu_plugin_jev.toml), user TOML, then built-ins."
    }

    /// Shows the ordinary one-state usage.
    fn examples(&self) -> Vec<Example<'_>> {
        vec![Example {
            example: "'hello' | jev ask {greeting: (jev question noul 'Is this a greeting?')} -c {policy: 'greetings'} -m jev-latest --dry-run",
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
            Evaluation::Preview(request) => to_nu(request, call.head)?,
            Evaluation::Response(response) => to_nu(response, call.head)?,
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
            assert_eq!(to_json(&result).unwrap()["model"], "jev-latest");
        }
        Ok(())
    }

    /// Preserves a mixed question map and structured object state in the preview.
    #[test]
    fn preview_mixed_questions_and_context() -> Result<(), Box<ShellError>> {
        let mut test = plugin_test()?;
        let result = test
            .eval("{message: 'hello'} | jev ask {spam: {type: noul, instructions: {task: 'spam'}, criteria: {'true': null}}, kind: {type: choice, criteria: {normal: null, spam: 'bulk'}}, urgency: {type: score, criteria: ['later' 'now']}} --context null --dry-run")?
            .into_value(Span::test_data())?;
        assert_eq!(
            to_json(&result).unwrap(),
            json!({"model": "jev-latest", "state": {"input": {"message": "hello"}, "context": null},
            "questions": {
                "spam": {"type": "noul", "instructions": {"task": "spam"}, "criteria": {"true": null}},
                "kind": {"type": "choice", "criteria": {"normal": null, "spam": "bulk"}},
                "urgency": {"type": "score", "criteria": ["later", "now"]}
            }})
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
        let setup = "$env.NU_PLUGIN_JEV_MODEL = 'env-model'; $env.config.plugins.jev = {model: 'config-model'};";
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
        assert_eq!(to_json(&short).unwrap()["model"], "flag-model");

        let live = test
            .eval(&format!(
                "{setup} $env.TYPESAFE_API_KEY = 'local'; {common} -c {{policy: 'greetings'}} -m flag-model"
            ))?
            .into_value(Span::test_data())?;
        assert_eq!(to_json(&live).unwrap()["answers"]["q"]["noul"], 0.75);
        assert_eq!(server.join().unwrap()[0].body, to_json(&long).unwrap());
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
        assert_eq!(to_json(&result).unwrap()["state"], json!(["a", "b"]));
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
        assert_eq!(to_json(&result).unwrap(), answer);
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

    /// Makes the offline preview byte-equivalent in JSON structure to a live body.
    #[test]
    fn preview_matches_live_body_without_sending_twice() -> Result<(), Box<ShellError>> {
        let (base_url, server) = serve(vec![json!({"model": "jev-fixed", "answers": {
            "match": {"type": "noul", "noul": 0.75}},
            "usage": {"input_tokens": 8, "output_tokens": 1}})]);
        let mut test = plugin_test()?;
        let common = format!(
            "{{message: 'hello'}} | jev ask {{match: {{type: noul, instructions: {{task: 'judge'}}, criteria: {{'true': null}}}}}} --context {{policy: ['first' 'second']}} --base-url '{base_url}'"
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
        assert_eq!(captured[0].body, to_json(&preview).unwrap());
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
        assert_eq!(
            first_server.join().unwrap()[0].path,
            "http://localhost:1/v1/systemone"
        );
        assert_eq!(
            second_server.join().unwrap()[0].path,
            "http://localhost:1/v1/systemone"
        );

        let preview = test.eval("$env.NU_PLUGIN_JEV_PROXY = 'http://user:secret@127.0.0.1:1'; 'hello' | jev ask {match: {type: noul}} --dry-run")?.into_value(Span::test_data())?;
        assert!(!to_json(&preview).unwrap().to_string().contains("secret"));
        let failure = test.eval("$env.NU_PLUGIN_JEV_PROXY = 'http://user:secret@127.0.0.1:1/path'; 'hello' | jev ask {match: {type: noul}} --dry-run").unwrap_err();
        assert!(!failure.to_string().contains("secret"));
        Ok(())
    }
}
