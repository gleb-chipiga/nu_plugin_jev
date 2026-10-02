# nu_plugin_jev

`nu_plugin_jev` is a Nushell plugin for TypeSafe Jev / System One. Nushell builds
the state and works with the returned decisions; the plugin handles Jev requests.

## Why this command set

The six commands cover the intended Jev-specific work: `jev ask` evaluates one
structured state against named questions, `jev annotate` evaluates independent
table rows, the three `jev question` constructors build reusable question data,
and `jev` shows offline guidance. Nushell already provides selection, filtering,
sorting, grouping, joins, and field updates, so the plugin does not duplicate
those operations. Asking several related questions in one request avoids
resending the same state; bounded, streaming annotation handles tables without
collecting them. Thresholds and subsequent transformations remain explicit Nu
operations over typed answers.

The plugin targets Nushell `0.116.x`. Before Nushell 1.0, a plugin built for one
Nu minor release may need to be rebuilt against another minor release.

## Install

```nu
cargo install --path . --locked
plugin add ~/.cargo/bin/nu_plugin_jev
plugin use jev
help jev
```

The default `mimalloc` feature uses mimalloc as the process allocator. To use
the system allocator instead, pass `--no-default-features` to `cargo install`.

Set `TYPESAFE_API_KEY` in the calling environment before using commands that
contact the service. Offline commands and request previews do not need a key.

Diagnostics go to stderr, not into Nu pipeline values. Set `NU_PLUGIN_JEV_LOG=info` before
the plugin process starts to see logical evaluation starts, completions, and
failures; `NU_PLUGIN_JEV_LOG=debug` additionally shows HTTP attempts, retry delays, and
table-result reuse. When a response contains a short, safe
`x-typesafe-request-id`, debug diagnostics also show it as
`server_request_id` for that HTTP attempt. This is separate from the local
logical `request_id` in `--meta` and does not appear in Nu response or error
records. The default level is `warn`. If the plugin is already running in a Nu
session, set `$env.NU_PLUGIN_JEV_LOG = "debug"` and run `plugin stop jev`;
the next command starts it with the new level. Credentials, state, questions,
and request bodies are not logged. `--meta` provides `request_id`, model, and
usage as ordinary Nu data when needed downstream.

The command surface and behavior are specified in the
[current OpenSpec change](openspec/changes/add-jev-plugin/proposal.md).
The repository-owned [Jev usage skill](skills/jev-nushell/SKILL.md) helps agents
compose pipelines using the implemented command surface.

## State and raw question contract

The pipeline value is sent as structured JSON state. A Nu string stays a JSON
string even if it looks like JSON; records and lists become JSON objects and
arrays. The System One endpoint accepts a top-level string, object, or array.
Nested values may contain JSON scalars. Supplying `--context` wraps the selected
input exactly as `{input: <value>, context: <value>}`; an explicit `null` context
is different from no context flag, and no record fields are merged implicitly.

| Nu value | JSON representation |
| --- | --- |
| string, integer, finite float, bool, nothing | string, number, number, bool, null |
| record, list | object, array, recursively converted |
| date | RFC3339 string |
| duration | exact signed nanoseconds string, e.g. `-1500000000ns` |
| filesize | integer byte count |

Binary, closure, range, cell path used as data, custom values, non-finite floats,
and other unsupported Nu types fail conversion with a field path. Incoming Nu
errors remain errors. API response JSON is converted back recursively; strings
that resemble dates remain strings, and integers outside Nu's signed 64-bit
range are rejected rather than rounded.

`jev ask` accepts a nonempty record of named questions. The same names appear
under `answers` in the response. Each question has `type: noul`, `choice`, or
`score`. `instructions` may be omitted or be a string, record, list, or `null`;
a top-level number or bool is invalid. Noul `criteria` may be omitted, `null`,
or a record with optional `true` and `false` descriptions. Choice requires a
record mapping option names to descriptions. Those descriptions have the same
allowed root types as instructions. Score requires a nonempty ordered list of
string/record/list level descriptions; a `null` level is invalid. Nested
records/lists may contain ordinary JSON scalars. Omission and explicit `null`
remain distinct in a request preview. Raw question maps follow the API minimum
cardinality; the question constructors additionally limit Choice to 255
options and Score to 10 levels. A Score rubric with at least two levels is
usually more useful.

```nu
let questions = {
    spam: {type: noul, instructions: "Is this unsolicited?", criteria: {"true": "spam", "false": "expected"}}
    kind: {type: choice, instructions: {task: "categorize", priority: 1}, criteria: {normal: null, promo: "advertising"}}
    urgency: {type: score, criteria: ["later", "today", "now"]}
}
{message: "Hello", sender: "Ada"} | jev ask $questions --dry-run
```

The request and answer field shapes follow the [TypeSafe OpenAPI
schema](https://api.typesafe.ai/openapi.json). Service-specific constructor
limits are distinguished in the [TypeSafe API reference](https://docs.typesafe.ai/api).

## Offline question constructors

The three constructors return ordinary Nu records in the raw question format.
They do not read credentials or contact Jev:

```nu
let questions = {
    spam: (jev question noul "Is this spam?" --yes "Unsolicited message" --no "Expected message")
    kind: (jev question choice "Message kind?" [normal promo spam])
    urgency: (jev question score "Review urgency?" ["later" "today" "now"])
}
```

Noul emits `true`/`false` criteria only for supplied flags, including an
explicit `null`. Choice accepts either a list of distinct option names (mapped
to `null` descriptions) or a record of descriptions, with 1–255 options. Score
accepts 1–10 ordered levels; each list position is its ordinal score starting
at zero. Instructions and Choice/Noul descriptions may be structured records
or lists. Score levels must be non-null strings, records, or lists.

Policies are plain data. Save a question record as `spam-policy.nuon` and load
it with `let questions = open spam-policy.nuon`, or export `questions` from a
Nu module and import it with `use spam-policy.nu questions`. No plugin-specific
policy DSL is required.

## Single-state evaluation

Use one `jev ask` request for one state and all related decisions. The result
retains the API envelope: `model`, named `answers`, and `usage`.

```nu
let questions = {
    spam: (jev question noul "Is this unsolicited?" --yes "Unrequested bulk mail")
    kind: (jev question choice "Message kind?" [normal promo spam])
    urgency: (jev question score "Review urgency?" ["later" "today" "now"])
}

let message = "Hello"
let sender = "Ada"
let result = ({message: $message, sender: $sender} | jev ask $questions)
$result
```

The response retains each full typed answer. Use Nu's own field access to
extract a single value while keeping model and usage available when needed:

```nu
$result | get answers.spam.noul
$result | get answers.kind.choice
$result | get answers.urgency.score
$result | get answers.kind.confidence
```

Score remains a fractional expected level, and confidence and distributions
remain exactly as the service returned them. The caller chooses any threshold
with a Nu expression. `--context` wraps input and context under separate keys,
with no implicit merge:

```nu
let policy = {spam: "Flag unsolicited bulk mail"}
{message: "Hello", sender: "Ada"} | jev ask $questions -c $policy -m jev-latest --dry-run
```

`--dry-run` on `jev ask` returns the exact request body without reading a key
or contacting the service. It is a useful check before transmitting sensitive
state. For example:

```nu
{message: "hello"} | jev ask {greeting: {type: noul, instructions: "Is this a greeting?"}} --dry-run
```

A finite pipeline stream passed to `jev ask` is collected into **one** JSON
array state and produces one evaluation about the entire array. This is not a
batch of independent row requests. `jev annotate` processes record rows
independently; native `where`, `sort-by`, and `reject` operate on its typed
answers:

```nu
open messages.nuon
| jev annotate $questions --fields [message sender] --into ai
| where ai.spam.noul >= 0.98
| sort-by ai.urgency.score --reverse
```

Optional row packing needs no extra plugin command: Nu `chunks` forms bounded
groups, `jev ask` evaluates each group with row-specific questions, and
`each --flatten` can emit the answers mapped back to source rows. Groups can
stream, but `ask` materializes the current group. Unlike `annotate`, every
question sees every row in its group; question names are answer keys, so each
instruction must identify its target row. Compare decisions and usage before
adopting packing. For ordered parallel processing, bound work into waves:
`par-each --keep-order` alone may read far ahead.

`jev annotate` preserves every source field, even when `--fields` sends only a
projection to Jev. `--fields [message sender]` names literal top-level columns;
`--state document.text` instead follows a Nu cell path, including list indices.
The two selectors are exclusive. `--context` wraps the selected state exactly
as for `jev ask`. Destination fields (`jev` by default, `--into`, and optional
`--meta`) must not overwrite existing row fields. `--meta jev_meta` adds the
resolved `model`, `usage`, and a local `request_id` for each logical evaluation.

```nu
[{id: 1, document: {text: "hello"}}]
| jev annotate $questions -s document.text -c $policy --dry-run
```

```nu
open messages.nuon
| jev annotate $questions -f [message sender] -c $policy -i ai --meta ai_meta -j 32
| where ai.spam.noul >= 0.98
| select id message ai ai_meta
```

The default `--on-error fail` stops on the first row failure. With
`--on-error keep`, a failed row passes through unchanged; with
`--on-error record`, it gains `jev_error: {kind, message, status}`. In those
modes, filter rows without an annotation before native field access, for
example `| where { |row| 'ai' in ($row | columns) }`. `--dry-run` streams one exact request body per
valid row, in input order and including duplicates, without a key or network
call. A failed row under `keep` or `record` instead yields the unchanged or
error-annotated source row, respectively.

`--jobs` limits simultaneous *unique* evaluations (default 16); an admitted
duplicate shares its in-progress request and retry sequence. The plugin
admits at most `2 × jobs` rows and uses bounded channels, so stopped downstream
consumption prevents unbounded row reads. Ordered output is the default and
can wait behind an early slow request; `--unordered` emits ready rows sooner.
Dropping the downstream stream or interrupting the command cancels outstanding
local work. An HTTP request already accepted by Jev cannot be recalled.

Successful results are reused through a per-invocation LRU, limited by both
`cache.max_entries` (1024) and `cache.max_approx_bytes` (16 MiB) by default.
The byte estimate counts canonical request key bytes, serialized response,
request identity, and fixed per-entry overhead; it is not a cap on process
RSS, active rows, or network buffers. A result larger than the byte limit is
not cached. Errors are never cached. After eviction, a failed evaluation, or
an oversized bypass, the same state may be evaluated again with a new
`request_id`; there is no once-per-invocation guarantee. The cache ends with
the annotation stream, so a moving model alias such as `jev-latest` is not
reused across invocations. To aggregate reported usage without counting cache
hits or in-flight subscribers twice, group metadata by distinct `request_id`.
That identity is local provenance, not a server idempotency key or an invoice.

The service has no independent-row batch endpoint. A client could technically
pack multiple rows into one shared state and rewrite questions, but that would
change each decision's context and complicate quality, cost, and partial-error
semantics. Automatic packing is excluded pending separate validation; an
intentional array state passed to `jev ask` remains one shared-state decision.

## Configuration

Each applicable setting is selected independently in this order: command flag,
`$env.config.plugins.jev`, caller environment, selected local TOML, user TOML,
then the default. A present but
invalid value is an error; it does not fall back to a lower-priority source.
Settings and files are captured once per command invocation, before reading
table rows; edits affect the next invocation, even in a reused plugin process.

Plugin-specific environment settings use `NU_PLUGIN_JEV_*`. If you used the
interim names, rename them before upgrading: `TYPESAFE_MODEL`,
`TYPESAFE_BASE_URL`, `TYPESAFE_TIMEOUT_MS`, `TYPESAFE_JOBS`, and
`TYPESAFE_RETRIES` become `NU_PLUGIN_JEV_` plus the same suffix;
`JEV_PROXY`, `JEV_CONFIG`, and `JEV_LOG` become `NU_PLUGIN_JEV_PROXY`,
`NU_PLUGIN_JEV_CONFIG`, and `NU_PLUGIN_JEV_LOG`. Move `.jev.toml` to
`.nu_plugin_jev.toml` and the user `jev/config.toml` to
`nu_plugin_jev/config.toml`. The old environment names and implicit file paths
are not fallback aliases. An old-named file remains usable if selected with
`--config`. The service credential `TYPESAFE_API_KEY`, the Nu config path
`$env.config.plugins.jev`, and the `--config` flag are unchanged.

| Setting | Flag | Plugin config | Environment | TOML | Default |
| --- | --- | --- | --- | --- | --- |
| Requested model | `--model` | `model` | `NU_PLUGIN_JEV_MODEL` | `model` | `jev-latest` |
| Service root | `--base-url` | `base_url` | `NU_PLUGIN_JEV_BASE_URL` | `base_url` | `https://api.typesafe.ai` |
| Total evaluation deadline | `--timeout` | `timeout` | `NU_PLUGIN_JEV_TIMEOUT_MS` | `timeout_ms` | `30sec` |
| Table concurrency | `--jobs` | `jobs` | `NU_PLUGIN_JEV_JOBS` | `jobs` | `16` |
| Additional retry attempts | — | `retries` | `NU_PLUGIN_JEV_RETRIES` | `retries` | `3` |
| Jev proxy | — | `proxy` | `NU_PLUGIN_JEV_PROXY` | `proxy` | `auto` |
| Completed cache entries | — | `cache.max_entries` | — | `cache.max_entries` | `1024` |
| Completed cache approximate bytes | — | `cache.max_approx_bytes` | — | `cache.max_approx_bytes` | `16777216` |

Frequently used flags have short aliases: `jev ask` accepts `-c` for
`--context` and `-m` for `--model`; `jev annotate` accepts those plus `-j` for
`--jobs`, `-s` for `--state`, `-f` for `--fields`, and `-i` for `--into`. Short and
long forms have identical validation and precedence. Other flags remain
long-only.

`--timeout` and the plugin-config `timeout` are positive Nu durations. The
environment variable `NU_PLUGIN_JEV_TIMEOUT_MS` and TOML `timeout_ms` are positive
integer numbers of milliseconds. Jobs and cache limits are positive integers; retries is a
nonnegative integer. The timeout covers one logical request, including retries
and waits, not the entire table. The service root must be an absolute HTTP(S)
URL without embedded credentials, query, or fragment; HTTP is useful for local
mock servers. `jev annotate` additionally uses jobs and cache settings.

For example, place non-secret defaults in your Nu configuration:

```nu
$env.config.plugins.jev = {
    model: "jev-latest"
    base_url: "https://api.typesafe.ai"
    jobs: 16
    timeout: 30sec
    retries: 3
    proxy: "auto"
    cache: {max_entries: 1024, max_approx_bytes: 16777216}
}
```

The optional user file is `$XDG_CONFIG_HOME/nu_plugin_jev/config.toml` when caller
`XDG_CONFIG_HOME` is absolute, otherwise the platform user-config directory's
`nu_plugin_jev/config.toml` (typically `$HOME/.config/nu_plugin_jev/config.toml` on Linux). The
optional local file is exactly `.nu_plugin_jev.toml` in the calling Nu directory; no
parent directories are searched. `--config <path>` chooses another local file,
or caller `$env.NU_PLUGIN_JEV_CONFIG` does so if the flag is absent. Relative paths are
resolved against the calling Nu directory. Missing implicit files are ignored;
missing explicit files, malformed TOML, unknown fields, and files over 64 KiB
are errors. Every TOML field is optional, including in an empty file.

For example, a user file may contain only a credential:

```toml
api_key = "YOUR_TYPESAFE_API_KEY"
```

Other supported fields are `model`, `base_url`, `timeout_ms`, `jobs`, `retries`,
`proxy`, and `[cache]` with `max_entries` and `max_approx_bytes`. The default
implicit `.nu_plugin_jev.toml` cannot set `base_url` or `proxy`; explicitly selecting
that file with `--config` or `NU_PLUGIN_JEV_CONFIG` permits validated transport settings.
This prevents an untrusted project file from silently redirecting a key.

Live requests select the key from caller `$env.TYPESAFE_API_KEY`, then local
`api_key`, then user `api_key`; a present empty key fails rather than falling
back. No key flag or plugin-config key exists. On Unix, a TOML file containing
`api_key` must be owner-only (for example, `chmod 600`); other platforms need
their corresponding private-file permissions, without a portable ACL check.
Keep key-bearing local files out of version control. Root usage, question
constructors, and `--dry-run` require no key and never include one in output.

## Proxy behavior

The default `auto` policy uses reqwest's ordinary process/OS proxy discovery,
including `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY` where
applicable. This automatic client is created when the plugin process starts.
Changes to ordinary proxy variables or OS settings require `plugin stop jev`
and a new plugin invocation to take effect.

Jev-specific proxy policy is resolved **for every command invocation**:
`$env.config.plugins.jev.proxy` takes precedence over caller-scoped
`$env.NU_PLUGIN_JEV_PROXY`, then local and user TOML, then defaults to `auto`. Accepted values are `auto`, `direct`
(bypass all proxies), an explicit `http://` proxy URL, or an explicit
`socks5h://` proxy URL (destination DNS resolves at the proxy). There is no
proxy command flag. To change policy while the plugin process stays alive,
change `NU_PLUGIN_JEV_PROXY` in the calling Nu environment, or change the plugin config
field; the plugin config field still takes precedence.

An explicit Jev proxy replaces ordinary discovery, is **not** bypassed by
global `NO_PROXY`, and never silently falls back to direct routing when it
fails. Proxy URLs can contain credentials; keep them in the caller environment
or secret storage rather than shared config files. Errors, diagnostics, and
`--dry-run` do not print the configured proxy URL. Dry runs validate policy
but do not open a proxy connection.

## HTTP behavior

Live evaluations send one authenticated `POST /v1/systemone` with the complete
state and all named questions. The plugin does not forward bearer credentials through HTTP
redirects. The configured `--timeout`/`timeout` is a total deadline for one
logical evaluation, including all attempts and retry waits; it is not a
deadline for an entire table pipeline.
HTTPS routes negotiate HTTP/2 when supported and otherwise use HTTP/1.1. The
configured application retry budget governs wire attempts; reqwest's separate
automatic protocol-NACK retry is disabled.

HTTP `429`, `502`, `503`, `504`, and `529` may be retried up to the configured
number of *additional* attempts. A valid finite, nonnegative, representable
`retry-after-ms` delay (milliseconds, including fractions) takes precedence
over `Retry-After`. If it is invalid or absent, a valid `Retry-After` delay in
seconds or HTTP-date form is honored. If the advised delay does not fit within
the evaluation deadline, the operation times out instead of retrying early.
Without usable guidance, the first retry waits 375–500 ms; the exponential
base doubles from 500 ms up to 5 seconds with 0–25% downward jitter.
Other 4xx statuses, malformed or contract-invalid responses, and transport
failures are not retried automatically. Local interruption cancels in-progress
requests and waits; it cannot undo a request already accepted by the service.
Retries may therefore repeat remote work; the local request ID and optional
server response IDs do not guarantee remote deduplication or reconcile billing.
Errors retain their category and HTTP status when available, but do not include
credentials, request bodies, response bodies, or raw headers.

## License

MIT. See [LICENSE](LICENSE).

## CI and releases

GitHub Actions checks formatting, Clippy, nextest runs with and without the default
allocator, the OpenSpec artifacts, dependency policy, and the publishable crate.
The Nu integration tests use Nushell `0.116.0` and local mock servers; CI does
not need a TypeSafe API key. Keep credentials out of the repository and crate
package.

Run the full local test suite with
`cargo nextest run --all-features --all-targets --locked`.

A `v<crate-version>` tag starts the release workflow after verification. It
uses `cargo-dist` to build seven platform archives, attests them, publishes to
crates.io, and creates a GitHub release with notes from
[CHANGELOG.md](CHANGELOG.md). The dist target
list and the temporary fat-LTO, single-codegen-unit release profile live in
`Cargo.toml`; this Rust package does not need a separate `dist.toml`.
Maintainers must first configure the `crates-io` GitHub environment and
crates.io trusted publishing for this repository and workflow. Tagging alone
does not supply those external publishing permissions.
