---
name: jev-nushell
description: Use nu_plugin_jev to compose TypeSafe Jev questions and Nushell pipelines, inspect outbound state, and work with typed decisions. Apply to plugin usage and examples, not Rust implementation.
---

# Jev in Nushell

Use this repository's `nu_plugin_jev` command surface. Read the repository
`README.md` or `help jev <command>` when exact syntax matters. Describe only
behavior present in the current binary, not planned OpenSpec work.

## Shape the request

- Construct a string, record, or list with ordinary Nu commands and pass it as
  pipeline input. Do not stringify records or lists. `--context <value>` wraps
  state as `{input: <pipeline value>, context: <value>}` without merging fields.
- Put related named Noul, Choice, and Score questions in one record. The
  `jev question noul|choice|score` constructors are offline and return ordinary
  Nu records, which can also be stored as NUON or Nu modules.
- Use `jev ask` for one state and all its questions. A list or finite input
  stream is one array state, **not** a batch of independent evaluations.
- Use `jev annotate` for independent record rows. Select an outbound state with
  `--state <cell-path>` or `--fields <list>` when the full row should not leave
  Nushell; the source row remains intact. These selectors are exclusive.
- For optional row packing, combine bounded Nu `chunks` with `jev ask` and map
  answers back using Nu. Each question's instructions must identify its target
  row; question names are only answer keys. All rows in a group become shared
  state, unlike independent `jev annotate` calls. Check decision quality and
  usage before relying on packing; `par-each --keep-order` may read far ahead
  unless parallel work is divided into bounded waves.
- The short aliases are `-c`/`--context` and `-m`/`--model` on `jev ask` and
  `jev annotate`, plus `-j`/`--jobs`, `-s`/`--state`, `-f`/`--fields`, and
  `-i`/`--into` on `jev annotate`. They have the same behavior as the long
  forms; other flags are long-only.

## Inspect and consume decisions

- Before sending sensitive data, use `--dry-run` to inspect the exact request
  body; it requires no API key and makes no network request. Live evaluations
  select the key from caller `TYPESAFE_API_KEY`, then local TOML `api_key`, then
  user TOML `api_key`. Do not print or embed credentials in a pipeline example.
- Live `jev ask` and `jev annotate` require network access to the configured
  service URL. For a real call from a sandbox, arrange permitted network access
  before running it; if access is unavailable, report the call as unverified.
  A sandbox-blocked transport error does not establish a plugin or API failure.
- `jev ask` returns `{model, answers, usage}`. `jev annotate` inserts only
  `answers` under `jev` by default, or under `--into`; `--meta <field>` adds
  model, usage, and local request identity separately.
- Use Nu `get`, `where`, `sort-by`, `group-by`, `select`, and `reject` for
  projections and thresholds. The plugin has no scalar `jev noul|choice|score`
  commands, `jev where`, or `jev models` command.
- Annotation defaults to `--on-error fail`. With `keep` or `record`, filter
  rows lacking an annotation before accessing nested answer fields. Duplicate
  successful evaluations may reuse a result; count reported usage by distinct
  `request_id` when metadata is included.

```nu
let questions = {
    spam: (jev question noul "Is this unsolicited?" --yes "Unrequested bulk mail")
    urgency: (jev question score "Review urgency?" ["later" "today" "now"])
}

[{message: "hello", sender: "Ada"}]
| jev annotate $questions -f [message sender] --dry-run
```

After inspecting the preview, remove `--dry-run` to obtain typed answers and
filter them with native Nu commands. A table annotation is one logical Jev
evaluation per distinct row request, subject to in-flight/result reuse; it is
not a server batch request.

## Retries and diagnostics

Live evaluations retry HTTP `429`, `502`, `503`, `504`, and `529` up to the
configured number of additional attempts (default 3). A valid
`retry-after-ms` delay takes precedence over `Retry-After`; otherwise retries
use 375–500 ms for the first unguided wait and an exponential base capped at
5 seconds. The configured timeout covers the complete logical evaluation,
including attempts and waits. Retries can repeat remote work; neither a local
request identity nor a server response ID is an idempotency guarantee.

`NU_PLUGIN_JEV_LOG=debug` set before plugin startup writes attempt diagnostics to stderr.
When the service provides a bounded, safe `x-typesafe-request-id`, those logs
include it as `server_request_id` alongside the local logical `request_id` and
attempt number. The server ID is not included in `jev ask`, annotation
metadata, or `jev_error`. Restart the plugin with `plugin stop jev` after
changing `NU_PLUGIN_JEV_LOG` in an already running Nu session.

## Proxy policy

The default `auto` policy uses ordinary process/OS proxy discovery captured
when the plugin process starts. Restart with `plugin stop jev` after changing
ordinary proxy settings. Jev-specific policy is read for every invocation:
`$env.config.plugins.jev.proxy` overrides caller-scoped `$env.NU_PLUGIN_JEV_PROXY`,
then selected local and user TOML;
accepted values are `auto`, `direct`, `http://...`, and `socks5h://...`.
Explicit Jev proxies ignore global `NO_PROXY`, never fall back to direct
routing, and may contain credentials. Do not print proxy URLs in examples or
previews. `--dry-run` validates the policy without opening a connection.

## TOML defaults

`jev ask` and `jev annotate` read optional user `nu_plugin_jev/config.toml` under the
platform user-config directory (absolute caller `XDG_CONFIG_HOME` on Unix)
and `.nu_plugin_jev.toml` in the calling Nu directory, without walking parents. `--config`
or caller `NU_PLUGIN_JEV_CONFIG` explicitly selects the local file; the flag wins and a
relative path is anchored to the caller directory. Only selected explicit
local files may set `base_url` or `proxy`; implicit `.nu_plugin_jev.toml` rejects them.
Unknown fields, malformed files, and files over 64 KiB fail. Settings are
resolved independently as flag > Nu plugin config > caller env > local TOML >
user TOML > default, once per invocation. The plugin process and pooled HTTP
clients can persist, but later calls reload files. A key-bearing TOML file
must be owner-only on Unix, and should have private permissions elsewhere.
Offline root/question commands do not read TOML; dry runs use file settings
without sending the key or opening a connection.

Plugin-owned environment settings use `NU_PLUGIN_JEV_*`: interim
`TYPESAFE_{MODEL,BASE_URL,TIMEOUT_MS,JOBS,RETRIES}` and
`JEV_{PROXY,CONFIG,LOG}` names map to the same suffix under that prefix and
are no longer read. Move implicit `.jev.toml` to `.nu_plugin_jev.toml` and user
`jev/config.toml` to `nu_plugin_jev/config.toml`; an old-named local file can
still be selected explicitly with `--config`. `TYPESAFE_API_KEY` remains the
service credential, while `$env.config.plugins.jev` and `--config` are unchanged.
