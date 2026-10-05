# nu_plugin_jev

[![crates.io](https://img.shields.io/crates/v/nu_plugin_jev.svg)](https://crates.io/crates/nu_plugin_jev)

`nu_plugin_jev` brings TypeSafe Jev / System One decisions into Nushell. Nu
builds the state and processes the answers; the plugin sends the request.

The command set is deliberately small:

| Command | Purpose |
| --- | --- |
| `jev ask` | Ask several named questions about one state in one request. |
| `jev annotate` | Ask the same questions independently for each table row. |
| `jev models` | List currently available model metadata. |
| `jev question noul` | Build a probability-of-true question. |
| `jev question choice` | Build a categorical question. |
| `jev question score` | Build an ordered-score question. |
| `jev` | Show offline usage guidance; accepts no pipeline input. |

Use native Nu commands such as `where`, `sort-by`, `select`, and `group-by` on
the typed answers. There are no plugin-specific filtering or sorting commands.
Related questions share one request; table processing streams with bounded
concurrency instead of collecting every row.
Nu declares record results for `ask`, `models`, and question constructors;
`annotate` declares `list<any>` because `--on-error keep` can pass non-record
rows through unchanged. Bare `jev` returns a guidance string.

## Install

The plugin targets Nushell `0.116.x`.

```nu
cargo install --path . --locked
plugin add ~/.cargo/bin/nu_plugin_jev
plugin use jev
help jev
```

The default build uses mimalloc, supports NUON diagnostics, and requires
HTTP/2 for live requests. A custom `base_url` and its proxy route must support
HTTP/2; local `http://` services can use cleartext prior-knowledge HTTP/2.
If an endpoint supports only HTTP/1.1, rebuild without the default-enabled
`http2-prior-knowledge` feature while retaining the other defaults:

```nu
cargo install --path . --locked --no-default-features --features mimalloc,nuon-tracing-format
```

That build retains normal HTTPS HTTP/2 negotiation with HTTP/1.1 fallback.
There is no runtime protocol switch. Plain `--no-default-features` also uses
the system allocator and omits NUON diagnostic formatting.

Live requests need an API key from `TYPESAFE_API_KEY` or a private NUON file.
Question constructors and `--dry-run` work without a key or network access.

## List current models

```nu
jev models | get models | sort-by name | select name description release_date
```

`jev models` makes an authenticated, bodyless `GET /v1/models` and returns
`{models: <table>, meta: {base_url}}`. With `--metrics`, its request body size
is `0`. It accepts no pipeline input. The
list is fetched on every call: names and aliases can change, and `jev ask`
and `jev annotate` do not consult it before evaluating. Use native Nu table
commands to inspect it. The command accepts `--base-url`, `--timeout`, and
`--config`, but needs neither a selected evaluation model nor table settings.

## Ask about one state

Questions are ordinary Nu records. The constructors only build data; they do
not contact Jev or accept pipeline input. Raw questions reject unknown fields,
duplicate names, and duplicate nested record keys before an HTTP request.

```nu
let questions = {
    spam: (jev question noul "Is this unsolicited?" --yes "Unrequested bulk mail")
    kind: (jev question choice "Message kind?" [normal promo spam])
    urgency: (jev question score "Review urgency?" ["later" "today" "now"])
}

let result = (
    {message: "Hello", sender: "Ada"}
    | jev ask $questions
)

$result | get answers.spam.noul
$result | get answers.kind.choice
$result | get answers.urgency.score
```

`jev ask` returns `{answers, meta: {base_url, model, usage}}`. Each answer retains its type and
service-provided details, including confidence and probability distributions.
The caller chooses thresholds; the plugin does not decide what counts as true.

The pipeline value is the state. A string stays a string, a record becomes a
JSON object, and a list becomes a JSON array. A finite stream passed to
`jev ask` is **one array state**, not a batch of independent requests.
Duplicate keys in the outbound state or context are rejected.

Add separate context with `--context <value>`. The outgoing state becomes
`{input: <pipeline value>, context: <value>}`; fields are not merged.

For independent single-state calls, Nu provides command parallelism:

```nu
$states | par-each --threads 4 { |state| $state | jev ask $questions }
```

For table rows, prefer one `jev annotate` call with `--jobs` instead.

Inspect the exact request body and its compact UTF-8 JSON size before sending data:

```nu
let preview = ({message: "Hello"} | jev ask $questions --dry-run)
$preview.request.state
$preview.request_bytes
```

The preview is `{request, request_bytes}`. The byte count covers only the JSON
body, not HTTP headers or other network overhead. It contains neither the API
key nor a network response.

## Annotate a table

`jev annotate` evaluates each record row independently. It preserves the
source fields, adds named answers under `answers` by default, and always adds
`jev_meta: {base_url, model, usage}` on successful rows.

```nu
open messages.nuon
| jev annotate $questions --fields [message sender]
| where answers.spam.noul >= 0.98
| sort-by answers.urgency.score --reverse
```

Use `--into ai` for another answer field (`ai.spam.noul`), or `--into jev`
to retain the former `jev.spam.noul` path. If a source row already has the
selected answer field, annotation stops rather than overwriting it, including
with `--on-error keep` or `record`.

`--fields` sends only the named top-level columns. Use `--state document.text`
instead to send one cell path; these selectors cannot be combined. The
original row remains intact either way. `--context` has the same wrapping
behavior as in `jev ask`.
Duplicate keys in the selected state or context fail; keys in unselected row
fields are not inspected or sent.

Useful options:

| Option | Effect |
| --- | --- |
| `--metrics` | Add HTTP measurements (`metrics` for `ask`/`models`, `jev_metrics` for `annotate`). |
| `--jobs 32` | Limit concurrent distinct evaluations; default is 16. |
| `--unordered` | Emit ready rows without waiting for earlier slow rows. |
| `--on-error keep` | Pass a failed row through without an annotation. |
| `--on-error record` | Add `jev_error: {kind, message, status}` to a failed row; only HTTP errors have a numeric status. |
| `--dry-run` | Stream `{request, request_bytes}` previews without a key or network call. |

State-error paths in `jev_error` are escaped and limited in length. Nested
upstream Nu error text is not copied into a row diagnostic.

The default error mode is `fail`: an observed terminal failure stops remaining
local work before waiting to deliver its error. Successful duplicates can share an
in-progress request or a bounded, per-invocation cache. Eviction permits a
later request for the same state. Output and input are bounded, and stopping
downstream consumption cancels outstanding local work. If a third-party input
iterator is already blocked in `next()`, the plugin cannot force that call to
return; it discards the row when the call eventually finishes. HTTP waits and
output stop without waiting for the iterator. With `--metrics`, shared
annotation rows reuse one `jev_metrics.request_id`; count their usage and body
bytes once per distinct ID. Metrics contain `request_bytes`, `response_bytes`,
`elapsed`, `attempt_elapsed`, `attempts`, and `http_version`; both times are Nu
durations. The service has no
independent-row batch endpoint: an array sent through `jev ask` is still one
shared state.

## Configuration

Invocation settings resolve independently, from highest to lowest priority:

1. Command flag, where available.
2. `$env.config.plugins.jev`.
3. Caller environment (`NU_PLUGIN_JEV_*`; key: `TYPESAFE_API_KEY`).
4. Local NUON file.
5. User NUON file.
6. Built-in default.

An invalid value is an error, not a reason to try a lower-priority source.
Settings are captured per command invocation, so later calls can see edited
files even when the plugin process persists.

| Setting | Environment | NUON | Default |
| --- | --- | --- | --- |
| Model | `NU_PLUGIN_JEV_MODEL` | `model` | `jev-latest` |
| Service root | `NU_PLUGIN_JEV_BASE_URL` | `base_url` | `https://api.typesafe.ai` |
| Timeout | `NU_PLUGIN_JEV_TIMEOUT_MS` | `timeout_ms` | 30 seconds |
| Table jobs | `NU_PLUGIN_JEV_JOBS` | `jobs` | 16 |
| Additional retries | `NU_PLUGIN_JEV_RETRIES` | `retries` | 3 |
| Proxy policy | `NU_PLUGIN_JEV_PROXY` | `proxy` | `auto` |

All live commands in one plugin process share an HTTP attempt limit of **128**.
It resolves once at plugin startup: `NU_PLUGIN_JEV_MAX_IN_FLIGHT` → local NUON
`max_in_flight` → user NUON `max_in_flight` → 128. Use a positive integer and
restart with `plugin stop jev` after changing it. Startup selects the local file
from `NU_PLUGIN_JEV_CONFIG` or `.nu_plugin_jev.nuon` in startup Nu `PWD`.
Later file edits, caller directories, `--config`, and Nu plugin
config cannot change this limit. It does not change the invocation-local
`--jobs` default of **16** or limit requests per second.
An invalid selected startup value prevents the plugin from starting, including
for offline commands.

`jev models` reads only the transport, retry, proxy, and credential settings;
invalid evaluation-only model, jobs, or cache values do not block a listing.

The optional user file is `nu_plugin_jev/config.nuon` in the platform user
config directory (usually `~/.config/nu_plugin_jev/config.nuon` on Linux). The
optional local file is `.nu_plugin_jev.nuon` in the calling Nu directory.
Select another local file with `--config <path>` or
`NU_PLUGIN_JEV_CONFIG`. Files contain a NUON record; every field is optional:

```nuon
{
    model: "jev-latest"
    timeout_ms: 30000
    jobs: 16
    max_in_flight: 128
    cache: {max_entries: 1024, max_approx_bytes: 16777216}
}
```

Empty files and `{}` inherit all settings. Files are data, not executable Nu
scripts. Duplicate or unknown keys are errors. `timeout_ms` remains an integer
millisecond count, not a Nu duration.

A local file discovered implicitly cannot set `base_url` or `proxy`. Select it
explicitly if you intend to allow those settings. A NUON file containing
`api_key` must be owner-only on Unix (for example, `chmod 600`). Keep it out
of version control. The key can also come from `TYPESAFE_API_KEY`; it is never
accepted as a command flag or included in a request preview.

The proxy setting accepts `auto`, `direct`, `http://...`, or `socks5h://...`.
`auto` uses the process/OS proxy settings captured when the plugin starts;
restart it with `plugin stop jev` after changing those settings. Jev-specific
proxy settings are resolved on each command invocation. An explicit proxy
does not honor global `NO_PROXY` and does not silently fall back to a direct
connection.

## Diagnostics and failures

Set `NU_PLUGIN_JEV_LOG=info` or `debug` before the plugin starts to write
plugin diagnostics to stderr. The default is `warn`; bare levels affect only
`nu_plugin_jev`. Dependency logs may contain sensitive URLs, headers, or
payloads; the plugin cannot redact them. To opt into a dependency target:

```nu
$env.NU_PLUGIN_JEV_LOG = "nu_plugin_jev=info,reqwest=debug"
plugin stop jev
```

For structured diagnostics, set `$env.NU_PLUGIN_JEV_LOG_FORMAT = "nuon"` and
restart with `plugin stop jev`. NUON support is included in the default Cargo
features via `nuon-tracing-format`; a `--no-default-features` build omits it and
rejects this setting. Each selected diagnostic event becomes one stderr line
with `timestamp`, `level`, `target`, `message`, typed `fields`, and root-to-leaf
`spans`. Successful completion events include the selected `base_url`, body
byte counts, attempt count, HTTP version, both durations in nanoseconds, and
evaluation input/output tokens. They occur once per HTTP operation, not per
cached row. HTTP durations start at sending, after initial capacity waiting.
`elapsed` includes retries and later slot waits; `attempt_elapsed` covers
only the final successful attempt through response validation. Byte counts
exclude headers and earlier retry responses.
Parse captured diagnostic-only output in Nu with `use std/formats *` and
`open --raw jev.log | from ndnuon`; one line also works with `from nuon`.
The plugin does not write log files. A whole Nu stderr capture may include
other messages that are not NUON records.

Both settings are read when the plugin process starts; changing them requires
`plugin stop jev`. Text is the default format. Unlisted dependencies stay
silent; `RUST_LOG` and `JEV_LOG` are not used. Plugin-owned events omit
credentials, state, questions, and request bodies. Treat captured third-party
stderr as sensitive. Selecting a target does not enable instrumentation a
library does not emit.
Use the always-present `meta` or `jev_meta` for model and usage data. Add
`--metrics` for HTTP measurements and annotation request identity.

One logical evaluation has a total timeout covering capacity waits, retry
waits, response decoding, and answer validation. HTTP `429`, `502`, `503`,
`504`, and `529` may be retried; valid `Retry-After` guidance is honored.
Other client errors and invalid responses are not retried.
Retries can repeat remote work, so
`request_id` guarantees neither idempotency nor billing.

Successful API response bodies are limited to 16 MiB per request. Larger bodies
fail with a nonretryable response error before JSON decoding, including when
sent without a `Content-Length` header. No partial response is returned.

For exact question validation, value conversion, retry, cache, and proxy
contracts, see the [OpenSpec requirements](openspec/changes/add-jev-plugin/specs/).
The repository [usage skill](skills/jev-nushell/SKILL.md) contains additional
pipeline guidance.

## Development and release

Run `cargo nextest run --all-features --all-targets --locked` for the test
suite. CI also checks formatting, Clippy, OpenSpec, dependency policy, and
the publishable crate. Integration tests use local mock servers and need no
TypeSafe key.

A `v<crate-version>` tag starts the release workflow, which builds platform
archives and publishes through configured crates.io trusted publishing.
Release notes live in [CHANGELOG.md](CHANGELOG.md).

MIT licensed. See [LICENSE](LICENSE).
