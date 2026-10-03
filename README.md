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

The default build uses mimalloc. Pass `--no-default-features` to `cargo install`
to use the system allocator.

Live requests need an API key from `TYPESAFE_API_KEY` or a private TOML file.
Question constructors and `--dry-run` work without a key or network access.

## List current models

```nu
jev models | sort-by name | select name description release_date
```

`jev models` makes an authenticated, bodyless `GET /v1/models` and returns a
table of string fields in service order. It accepts no pipeline input. The
list is fetched on every call: names and aliases can change, and `jev ask`
and `jev annotate` do not consult it before evaluating. Use native Nu table
commands to inspect it. The command accepts `--base-url`, `--timeout`, and
`--config`, but needs neither a selected evaluation model nor table settings.

## Ask about one state

Questions are ordinary Nu records. The constructors only build data; they do
not contact Jev.

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

`jev ask` returns `{model, answers, usage}`. Each answer retains its type and
service-provided details, including confidence and probability distributions.
The caller chooses thresholds; the plugin does not decide what counts as true.

The pipeline value is the state. A string stays a string, a record becomes a
JSON object, and a list becomes a JSON array. A finite stream passed to
`jev ask` is **one array state**, not a batch of independent requests.

Add separate context with `--context <value>`. The outgoing state becomes
`{input: <pipeline value>, context: <value>}`; fields are not merged.

Inspect the exact request body before sending data:

```nu
{message: "Hello"} | jev ask $questions --dry-run
```

The preview contains neither the API key nor a network response.

## Annotate a table

`jev annotate` evaluates each record row independently. It preserves the
source fields and adds the named answers under `jev`, or under `--into`.

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

Useful options:

| Option | Effect |
| --- | --- |
| `--meta jev_meta` | Add model, token usage, and a local `request_id` separately. |
| `--jobs 32` | Limit concurrent distinct evaluations; default is 16. |
| `--unordered` | Emit ready rows without waiting for earlier slow rows. |
| `--on-error keep` | Pass a failed row through without an annotation. |
| `--on-error record` | Add a `jev_error` record to a failed row. |
| `--dry-run` | Stream request bodies without a key or network call. |

The default error mode is `fail`. Successful duplicates can share an
in-progress request or a bounded, per-invocation cache. Eviction permits a
later request for the same state. Output and input are bounded, and stopping
downstream consumption cancels outstanding local work. The service has no
independent-row batch endpoint: an array sent through `jev ask` is still one
shared state.

## Configuration

Each setting is resolved independently, from highest to lowest priority:

1. Command flag, where available.
2. `$env.config.plugins.jev`.
3. Caller environment (`NU_PLUGIN_JEV_*`; key: `TYPESAFE_API_KEY`).
4. Local TOML file.
5. User TOML file.
6. Built-in default.

An invalid value is an error, not a reason to try a lower-priority source.
Settings are captured per command invocation, so later calls can see edited
files even when the plugin process persists.

| Setting | Environment | TOML | Default |
| --- | --- | --- | --- |
| Model | `NU_PLUGIN_JEV_MODEL` | `model` | `jev-latest` |
| Service root | `NU_PLUGIN_JEV_BASE_URL` | `base_url` | `https://api.typesafe.ai` |
| Timeout | `NU_PLUGIN_JEV_TIMEOUT_MS` | `timeout_ms` | 30 seconds |
| Table jobs | `NU_PLUGIN_JEV_JOBS` | `jobs` | 16 |
| Additional retries | `NU_PLUGIN_JEV_RETRIES` | `retries` | 3 |
| Proxy policy | `NU_PLUGIN_JEV_PROXY` | `proxy` | `auto` |

`jev models` reads only the transport, retry, proxy, and credential settings;
invalid evaluation-only model, jobs, or cache values do not block a listing.

The optional user file is `nu_plugin_jev/config.toml` in the platform user
config directory (usually `~/.config/nu_plugin_jev/config.toml` on Linux). The
optional local file is `.nu_plugin_jev.toml` in the calling Nu directory.
Select another local file with `--config <path>` or
`NU_PLUGIN_JEV_CONFIG`. Every TOML field is optional.

A local file discovered implicitly cannot set `base_url` or `proxy`. Select it
explicitly if you intend to allow those settings. A TOML file containing
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
diagnostics to stderr. In an existing Nu session, run `plugin stop jev` after
changing the variable. Logs omit credentials, state, questions, and request
bodies. Use `--meta` when downstream Nu code needs model, usage, or the local
request ID as data.

One logical evaluation has a total timeout, including retry waits. HTTP
`429`, `502`, `503`, `504`, and `529` may be retried; `Retry-After` guidance
is honored when valid. Other client errors and invalid responses are not
retried. Retries can repeat remote work, so `request_id` is not an idempotency
or billing guarantee.

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
