## General coding rules
- All modules, public types, and functions must have docstrings (Rust `///`).
- Docstring style: brief but precise; 1-3 lines describing what the module/type/function does, key guarantees, and expected side effects.
- Prefer stack types and minimize allocations (use heap only when necessary).
- Comments and docstrings must be in English only (requirement for code).
- In the tokio runtime, avoid blocking or potentially blocking calls (if needed, move to a dedicated thread or `tokio::task::spawn_blocking`).
- Configure `tracing` with a non-blocking subscriber/writer; do not use a blocking default logger for runtime I/O paths.
- Keep tracing initialization in a dedicated module rather than in `main.rs`.
- Create the Tokio runtime explicitly with `tokio::runtime::Builder`; do not use the `#[tokio::main]` macro.
- Initialize tracing before entering the Tokio runtime; tracing setup must happen outside the runtime.
- Prefer iterators and functional style over manual loops where possible.
- Rule: minimal visibility by default — if not needed even within the crate, keep it non-`pub`; use `pub(crate)` only if needed inside the crate; use `pub` only for external API.
- When changing Rust code or Cargo dependencies, run `cargo fmt`, `cargo clippy --all-targets --all-features`, and `cargo nextest run --all-features --all-targets --locked`; fix all findings. For documentation- or pipeline-only changes, validate the changed files without running Rust code checks.
- Prefer specific types (NewType idiom) where justified.
- Do not use `lib.rs` (binary only).
- `main.rs` should stay thin, build the runtime, initialize tracing via the dedicated module before runtime entry, and delegate application behavior to a single function.
- Use Conventional Commits for every commit message. The project convention uses a
  lower-case type and lower-case description, for example `docs: update changelog`.
- Keep the repository-owned usage skill at `skills/jev-nushell/SKILL.md` in sync
  with implemented, user-visible plugin behavior. Update it in the same change
  as command, flag, state, response, configuration, proxy, error, or safety
  behavior changes; update README examples and guidance alongside it. Do not
  describe planned OpenSpec behavior as already implemented.
- Read `skills/jev-nushell/SKILL.md` when preparing Jev/Nushell usage examples
  or workflows for this repository, even if the root-level skill directory is
  not included in an agent's automatic skill discovery paths.
