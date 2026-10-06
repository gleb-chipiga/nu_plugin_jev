## General coding rules

- All modules, public types, and functions must have docstrings (Rust `///`).
- Docstring style: brief but precise; 1-3 lines describing what the module/type/function does,
  key guarantees, and expected side effects.
- Prefer stack types and minimize allocations (use heap only when necessary).
- Comments and docstrings must be in English only (requirement for code).
- Keep every Rust source line, including tests and comments, at most 100 characters long.
- Keep short single-line text and expressions as ordinary string literals;
  use `format!` for interpolation. Do not turn them into artificial text blocks.
  Use `indoc!` for multiline text blocks, long prose, and multiline Nu examples,
  including long source blocks whose resulting text is one line;
  use `formatdoc!` for interpolated blocks.
  Import only the needed `indoc` macros in the module where they are used;
  call `indoc!` and `formatdoc!` without the `indoc::` path prefix.
  Reserve `concat!` for compile-time composition with macros such as `env!`, not
  for splitting a literal text into fragments to satisfy the source line limit.
  Prefer indented raw strings and actual line breaks for Nu scripts; wrap long
  Nu expressions instead of joining lines with Rust continuation escapes.
  Put text-block content on dedicated lines. Combine the opening quote with the
  macro's opening brace, and the closing quote with its closing brace when no
  formatting arguments intervene. Format embedded Nu pipelines, records, and
  closures as Nu code.
  For text that must remain one line, use continuation escapes inside `indoc!`,
  including after the final content line to avoid adding a trailing newline.
  Do not add blank lines merely to preserve insignificant opening newlines.
  Preserve meaningful whitespace and exact protocol separators; keep explicit
  `\r\n` and continuation escapes when needed for HTTP wire text.
- Format `json!` objects with multiple fields or nested structures using one field
  per line and explicit nesting indentation. Put array objects on separate lines;
  keep scalars, empty containers, and short simple values compact.
- In the tokio runtime, avoid blocking or potentially blocking calls (if needed, move to a
  dedicated thread or `tokio::task::spawn_blocking`).
- Configure `tracing` with a non-blocking subscriber/writer; do not use a blocking default logger
  for runtime I/O paths.
- Keep tracing initialization in a dedicated module rather than in `main.rs`.
- Create the Tokio runtime explicitly with `tokio::runtime::Builder`; do not use the
  `#[tokio::main]` macro.
- Initialize tracing before entering the Tokio runtime; tracing setup must happen outside
  the runtime.
- Prefer iterators and functional style over manual loops where possible.
- Rule: minimal visibility by default — if not needed even within the crate, keep it non-`pub`;
  use `pub(crate)` only if needed inside the crate; use `pub` only for external API.
- When changing Rust code or Cargo dependencies, run `cargo fmt`,
  `cargo clippy --all-targets --all-features`, and
  `cargo nextest run --all-features --all-targets --locked`; fix all findings.
  For documentation- or pipeline-only changes, validate the changed files without running
  Rust code checks.
- Prefer specific types (NewType idiom) where justified.
- Do not use `lib.rs` (binary only).
- `main.rs` should stay thin, build the runtime, initialize tracing via the dedicated module
  before runtime entry, and delegate application behavior to a single function.
- Use Conventional Commits for every commit message. The project convention uses a
  lower-case type and lower-case description, for example `docs: update changelog`.
- Keep the repository-owned usage skill at `skills/jev-nushell/SKILL.md` in sync
  with implemented, user-visible plugin behavior. Update it in the same change
  as command, flag, state, response, configuration, proxy, error, or safety
  behavior changes; update README examples and guidance alongside it. Do not
  describe planned OpenSpec behavior as already implemented.
- Keep the usage skill focused on implemented guidance that changes command
  or flag choice, outbound data, result or error interpretation, access
  configuration, or material disclosure risks. Leave non-actionable
  implementation mechanics in code and technical specs.
- Read `skills/jev-nushell/SKILL.md` when preparing Jev/Nushell usage examples
  or workflows for this repository, even if the root-level skill directory is
  not included in an agent's automatic skill discovery paths.

## Markdown formatting

- Wrap ordinary Markdown prose at 100 characters per source line, including OpenSpec artifacts
  and skill instructions. Wrap at word boundaries without inserting blank lines inside a
  paragraph; preserve list indentation and meaningful Markdown line breaks.
- Allow longer lines for URLs, indivisible identifiers, tables, and code blocks when wrapping
  would damage readability or change meaning. Do not enforce a blanket limit on every
  Markdown line.
- The line-length rule is independent of OpenSpec's 500-character requirement-description
  limit. Wrapping a description does not split it into separate requirements or reduce
  its total length.

## OpenSpec specifications

- Follow the official spec-driven guidance: each `### Requirement:` description
  (the text before its first scenario) must state one testable behavior, use
  `SHALL` or `MUST`, and be at most 500 characters long.
- Put examples and edge cases in `#### Scenario:` blocks with `WHEN`/`THEN`.
  Split multiple behaviors into separate requirements, each with at least one
  scenario; do not shorten text mechanically just to satisfy the length limit.
- After editing OpenSpec specs, run `openspec validate --all --strict --no-interactive`
  and leave no findings for the affected specs, including `INFO` notices.
  A zero exit code is not enough; `--strict` does not reject informational
  findings. If archived changes were edited, validate them with `--archived` too.
