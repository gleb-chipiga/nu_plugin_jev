# nu_plugin_jev

[![crates.io](https://img.shields.io/crates/v/nu_plugin_jev.svg)](https://crates.io/crates/nu_plugin_jev)

`nu_plugin_jev` brings TypeSafe Jev / System One decisions into Nushell. Nu
builds the state and processes the answers; the plugin sends the request.

The command set is deliberately small:

| Command | Purpose |
| --- | --- |
| `jev ask` | Ask several named questions about one state in one request. |
| `jev annotate` | Ask the same questions independently for each table row. |
| `jev models` | List currently available models. |
| `jev question noul` | Build a probability-of-true question. |
| `jev question choice` | Build a categorical question. |
| `jev question score` | Build an ordered-score question. |
| `jev` | Show offline usage guidance. |

Use native Nu commands such as `where`, `sort-by`, `select`, and `group-by` on
the typed answers. There are no plugin-specific filtering or sorting commands.
Related questions share one request; table processing streams with bounded
concurrency instead of collecting every row.

## Install

The plugin targets Nushell `0.116.x`.

```nu
cargo install --path . --locked
plugin add ~/.cargo/bin/nu_plugin_jev
plugin use jev
help jev
```

The default build uses mimalloc, supports NUON logs, and requires HTTP/2.
Custom endpoints and proxy routes must support HTTP/2 too.
For HTTP/1.1 fallback, build without `http2-prior-knowledge`:

```nu
cargo install --path . --locked --no-default-features --features mimalloc,nuon-tracing-format
```

Live requests need an API key from `TYPESAFE_API_KEY` or a private NUON file.
Question constructors and `--dry-run` work without a key or network access.

## Ask about one state

Questions are ordinary Nu records. The constructors only build data; they do
not contact Jev. Unknown question fields and duplicate outbound keys fail before sending.

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

`jev ask` returns `{answers, meta: {base_url, model, usage}}`. Answers retain
service-provided details, including confidence and probability distributions.
The caller chooses thresholds; the plugin does not decide what counts as true.

The pipeline value is the state. A string stays a string, a record becomes a
JSON object, and a list becomes a JSON array. A finite stream passed to
`jev ask` is **one array state**, not a batch of independent requests.

Add separate context with `--context <value>`. The outgoing state becomes
`{input: <pipeline value>, context: <value>}`; fields are not merged.

For independent calls, use native Nu parallelism:

```nu
$states | par-each --threads 4 { |state| $state | jev ask $questions }
```

For table rows, prefer `jev annotate --jobs N`.

Inspect the exact request body before sending data:

```nu
{message: "Hello"} | jev ask $questions --dry-run
```

The preview is `{request, request_bytes}`. Bytes measure the compact UTF-8 JSON body,
not headers or network overhead. No API key or network response is included.

## Annotate a table

`jev annotate` evaluates each record row independently. It preserves the
source fields, adds answers under `answers` (or `--into`), and always adds
`jev_meta: {base_url, model, usage}` on success.

```nu
open messages.nuon
| jev annotate $questions --fields [message sender] --into ai
| where ai.spam.noul >= 0.98
| sort-by ai.urgency.score --reverse
```

`--fields` sends only the named top-level columns. Use `--state document.text`
instead to send one cell path; these selectors cannot be combined. The
original row remains intact either way. `--context` has the same wrapping
behavior as in `jev ask`.

Destination-field conflicts stop annotation, even with `--on-error keep` or `record`.

Useful options:

| Option | Effect |
| --- | --- |
| `--metrics` | Add HTTP measurements under `jev_metrics`. |
| `--jobs 32` | Limit concurrent distinct evaluations; default is 16. |
| `--unordered` | Emit ready rows without waiting for earlier slow rows. |
| `--on-error keep` | Pass a failed row through without an annotation. |
| `--on-error record` | Add `jev_error: {kind, message, status}` to a failed row. |
| `--dry-run` | Stream `{request, request_bytes}` previews without a key or network call. |

The default error mode is `fail`. With `keep` or `record`, failed rows may lack answers;
only HTTP errors have a numeric `status`.

Identical requests share in-progress work or cached successes within one invocation.
Cache eviction allows another request. Output is ordered unless `--unordered` is set;
stopping downstream consumption cancels outstanding local work.

## List models

```nu
jev models | get models | sort-by name | select name description release_date
```

- Returns `{models: <table>, meta: {base_url}}`; requires an API key.
- Accepts `--base-url`, `--timeout`, `--config`, and `--metrics`, but no pipeline input.
- Fetches a fresh list on every call. Evaluations do not consult it automatically.

## Request metrics

Use `--metrics` to add `metrics` on `ask`/`models` or `jev_metrics` on `annotate`:

- `request_bytes`: body bytes per attempt. `response_bytes`: final successful body bytes.
- `elapsed`: first send through validation, including retries and later capacity waits.
- `attempt_elapsed`: final successful send through validation. Both times are Nu durations.
- `attempts` and `http_version`: HTTP attempt count and final response protocol.
- `request_id`: shared annotation rows reuse it; count usage and bytes once per distinct ID.

Headers and earlier retry responses are excluded. Initial capacity waiting counts toward the
timeout, not HTTP durations.
`jev models` has no request body, so its `request_bytes` is `0`.

## Configuration

Each setting is resolved independently, from highest to lowest priority:

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

- Local file: `.nu_plugin_jev.nuon` in the calling Nu directory.
- User file: `nu_plugin_jev/config.nuon` in the platform config directory;
  usually `~/.config/nu_plugin_jev/config.nuon` on Linux.

Select another local file with `--config <path>` or `NU_PLUGIN_JEV_CONFIG`.
Files are NUON data, not executable scripts. Every field is optional:

```nuon
{
    model: "jev-latest"
    timeout_ms: 30000
    jobs: 16
}
```

Empty files and `{}` add no overrides. Unknown or duplicate keys fail.
`timeout_ms` is an integer millisecond count, not a Nu duration.

- An implicitly discovered local file cannot set `base_url` or `proxy`; select it explicitly.
- Files containing `api_key` must be owner-only on Unix (`chmod 600`) and kept out of Git.
- API keys are never accepted as command flags or included in request previews.

### Settings that need a restart

The shared HTTP limit defaults to **128 simultaneous attempts**, independently of `--jobs 16`.
It resolves once at startup: `NU_PLUGIN_JEV_MAX_IN_FLIGHT` → local NUON `max_in_flight`
→ user NUON `max_in_flight` → `128`. Use a positive integer.

Startup selects the local file via `NU_PLUGIN_JEV_CONFIG` or `.nu_plugin_jev.nuon` in startup Nu
`PWD`. Later `--config`, Nu plugin config, and file edits cannot change the limit.
An invalid startup value prevents the plugin from starting, even for offline commands.
Restart with `plugin stop jev` after changing it.

### Proxies

- `auto`: use process/OS proxy settings captured at startup; changes need `plugin stop jev`.
- `direct`: connect without a proxy.
- `http://...` or `socks5h://...`: use the explicit proxy, without `NO_PROXY` or direct fallback.

Jev-specific proxy settings refresh per command invocation.

## Diagnostics and failures

Set logging before startup; run `plugin stop jev` after changing it:

```nu
$env.NU_PLUGIN_JEV_LOG = "info"
$env.NU_PLUGIN_JEV_LOG_FORMAT = "nuon"
plugin stop jev
```

- Default: plugin-only `warn` in text format. NUON support is included in the default build.
- Dependency logs require explicit targets, such as `nu_plugin_jev=info,reqwest=debug`.
- Plugin events omit credentials and request contents. Dependency logs may expose secrets.
- `RUST_LOG` and `JEV_LOG` are not used.

Successful `info` events include base URL, HTTP measurements, and evaluation token usage,
even without `--metrics`. Log durations are nanoseconds; events are emitted once per HTTP operation.
Use `meta`/`jev_meta` and `--metrics` when you need these values as pipeline data.

Parse captured NUON diagnostic lines with Nu:

```nu
use std/formats *
open --raw jev.log | from ndnuon
```

The plugin does not manage log files. Whole Nu stderr may also contain non-NUON messages.

One logical request has a total timeout covering capacity waits, retries, decoding, and validation.
HTTP `429`, `502`, `503`, `504`, and `529` may be retried; `Retry-After` guidance
is honored when valid. Other client errors and invalid responses are not
retried. Retries can repeat remote work, so `request_id` is not an idempotency
or billing guarantee. Successful response bodies above 16 MiB fail without a partial result.

For exact question validation, value conversion, retry, cache, and proxy
contracts, see the [OpenSpec requirements](openspec/specs/).
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
