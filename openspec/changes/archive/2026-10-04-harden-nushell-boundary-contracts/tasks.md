# Tasks

## 1. Question input validation

- [x] 1.1 Reconcile raw-question parsing with the question delta: reject unknown top-level fields, duplicate outer names, and nested duplicate record keys before dispatch; verify with unit tests for each case and an `jev ask --dry-run` plugin test.
- [x] 1.2 Confirm the README and repository usage skill explain strict raw-question validation; verify their guidance matches the tested command behavior.

## 2. Lossless outbound conversion

- [x] 2.1 Reconcile recursive Nu-to-JSON conversion with duplicate-key detection and location-aware errors for selected state and context; verify nested-key unit tests and an annotation test showing unselected duplicate fields do not block `--state` selection.
- [x] 2.2 Document selected outbound duplicate-key behavior in the README and repository usage skill; verify both distinguish selected state/context from unrelated row fields.

## 3. Command signatures and offline input

- [x] 3.1 Reconcile all seven Nu signatures with the declared type pairs, keeping `annotate` output broad enough for `--on-error keep`; verify the command-signature test covers every command.
- [x] 3.2 Reject nonempty pipeline input to root guidance and all three constructors; verify plugin tests cover each command while no-input invocations still work without credentials.
- [x] 3.3 Update command help, README, and repository usage skill for no-input commands and truthful type declarations; verify Nu help or plugin-test inspection agrees with the registered signatures.

## 4. Integration verification

- [x] 4.1 Run `cargo fmt --all`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo nextest run --all-features --all-targets --locked`, `openspec validate harden-nushell-boundary-contracts --strict --no-interactive`, and `git diff --check`; verify every command exits successfully.
