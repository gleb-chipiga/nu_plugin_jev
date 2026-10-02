---
name: jev-nushell
description: Compose Nushell pipelines with nu_plugin_jev for structured Jev decisions. Use for plugin usage and examples, not Rust implementation.
---

# Jev in Nushell

Nushell shapes the input and processes the result; the plugin evaluates named
questions. Prefer native Nu commands over inventing extra Jev operations.

## Choose the operation

- Build a string, record, or list with Nu and pass it as pipeline state. Do not
  stringify structured values. `--context` wraps state as `{input, context}`;
  it does not merge fields.
- Build named Noul, Choice, and Score questions with the offline
  `jev question noul|choice|score` constructors. Use `jev ask` to evaluate
  related questions about one state in one request. A list or finite stream
  passed to `jev ask` is one array state, not a batch of independent requests.
- Use `jev annotate` for independent table rows. `--state <cell-path>` and
  `--fields <list>` restrict outbound data without removing source columns;
  they cannot be combined. Shared context is sent with each row.
- Use native Nu commands for filtering, sorting, and projecting answers. There
  are no scalar `jev noul|choice|score`, `jev where`, or `jev models` commands.

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
| where jev.spam.noul >= 0.98
```

## Inspect and consume results

- Use `--dry-run` to inspect the exact outbound request before sending data.
  It needs no key or network. Never include credentials in examples or output.
- Live calls need a TypeSafe API key and permitted network access. If a sandbox
  blocks access, report the call as unverified, not as an API failure.
- `jev ask` returns `{model, answers, usage}`; `jev annotate` adds `answers`
  under `jev` or `--into`. Read `noul`, `choice`, or `score` from each named
  answer; Nu code chooses its own thresholds. `--meta` adds model, usage, and a
  local request ID. Reused results share that ID, so count reported usage once
  per distinct ID.
- With `--on-error keep` or `record`, some rows lack answers. Check for an
  annotation before accessing nested answer fields.
