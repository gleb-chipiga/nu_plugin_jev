---
name: jev-nushell
description: Compose Nushell pipelines with nu_plugin_jev for structured Jev decisions. Use for plugin usage and examples, not Rust implementation.
---

# Jev in Nushell

Nushell shapes the input and processes the result; the plugin evaluates named
questions. Prefer native Nu commands over inventing extra Jev operations.
Run bare `jev` without pipeline input for offline guidance.

## Choose the operation

- Build a string, record, or list with Nu and pass it as pipeline state. Do not
  stringify structured values. `--context` wraps state as `{input, context}`;
  it does not merge fields. Duplicate outbound record keys are rejected.
- Build named Noul, Choice, and Score questions with the offline
  `jev question noul|choice|score` constructors; pass arguments, not pipeline
  input. Raw questions reject unknown fields and duplicate names or nested
  record keys before HTTP. Use `jev ask` to evaluate
  related questions about one state in one request. A list or finite stream
  passed to `jev ask` is one array state, not a batch of independent requests.
- Use `jev annotate` for independent table rows. `--state <cell-path>` and
  `--fields <list>` restrict outbound data without removing source columns;
  they cannot be combined. Duplicate keys in selected values fail, while
  unselected row fields are not inspected or sent. Shared context is sent with
  each row.
- Use native Nu commands for filtering, sorting, and projecting answers. There
  are no scalar `jev noul|choice|score` or `jev where` commands.
- Use `jev models` to fetch the current model catalog. It accepts no pipeline
  input and returns `{models, meta: {base_url}}`; each model has `name`,
  `description`, and `release_date` strings. The list is not cached and does
  not preflight evaluations. Filter or sort after `get models`.

For example, build related questions once, preview one outgoing state, then
apply the same questions independently to table rows:

```nu
let questions = {
    spam: (jev question noul "Is this spam?" --yes "Unsolicited bulk mail")
    urgency: (jev question score "Review urgency?" ["later" "today" "now"])
}

{message: "Hello"} | jev ask $questions --dry-run
[{message: "Hello"}, {message: "Buy now"}]
| jev annotate $questions --fields [message]
| where answers.spam.noul >= 0.98
```

To inspect available names before choosing a model:

```nu
jev models | get models | sort-by name | select name release_date
```

## Inspect and consume results

- Use `--dry-run` to inspect the exact outbound request before sending data.
  It returns `{request, request_bytes}`; use `get request.state` for the state.
  The size is compact UTF-8 JSON body bytes, not total wire traffic. It needs
  no key or network. Never include credentials in examples or output.
- Live calls, including `jev models`, need a TypeSafe API key and permitted
  network access. The listing accepts `--base-url`, `--timeout`, and `--config`
  but no evaluation model or table options. If a sandbox
  blocks access, report the call as unverified, not as an API failure.
  A successful API response body larger than 16 MiB fails with a nonretryable
  response error before JSON decoding; no partial answers are exposed.
- `jev ask` returns `{answers, meta: {base_url, model, usage}}`;
  `jev annotate` adds answers under `answers` by default and always adds
  `jev_meta: {base_url, model, usage}` on success. Read `noul`, `choice`, or
  `score` from each named answer; Nu code chooses its own thresholds. Use
  `--into ai` for a custom answer field or `--into jev` for the old path.
  A source field matching the answer destination terminates annotation,
  including with `--on-error keep` or `record`.
  `--metrics` adds HTTP measurements: `metrics` on `ask`/`models`,
  `jev_metrics` on `annotate`. Reused rows share `jev_metrics.request_id`;
  count usage and body bytes once per distinct ID.
- With `--on-error keep` or `record`, some rows lack answers. Check for an
  annotation before accessing nested answer fields. Nu declares `annotate`
  output as `list<any>` because `keep` can pass through non-record rows.
  `record` adds `jev_error: {kind, message, status}`; only HTTP failures have a
  numeric status. State paths are escaped and bounded; nested upstream Nu
  error text is not copied into row diagnostics.
- `NU_PLUGIN_JEV_LOG=info|debug` enables plugin-only text diagnostics.
  Third-party logs may contain secrets and are not automatically redacted.
  To enable them, select targets explicitly, for example
  `NU_PLUGIN_JEV_LOG=nu_plugin_jev=info,reqwest=debug`, then restart with
  `plugin stop jev`. Unlisted dependencies remain off.
- Successful `evaluation completed` and `model listing completed` events at
  `info` include the selected `base_url`, body bytes, attempt count, HTTP
  version, and total/final-attempt nanoseconds even without `--metrics`.
  They occur once per HTTP operation, not once per cached row.
- Set `NU_PLUGIN_JEV_LOG_FORMAT=nuon` before startup for one structured record
  per plugin stderr line. Records contain `timestamp`, `level`, `target`,
  `message`, `fields`, and `spans`. Parse captured diagnostic-only lines with
  Nu's `from nuon`; for multiple lines use
  `use std/formats *; open --raw jev.log | from ndnuon`. Whole Nu stderr may
  contain non-NUON messages; the plugin has no managed log file.
