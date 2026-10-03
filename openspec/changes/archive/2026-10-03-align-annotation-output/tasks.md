# Tasks

## 1. Align answer destination

- [x] 1.1 Change the default `jev annotate` answer destination to `answers`, retaining `--into`; verify focused plugin and real-Nu tests for default, custom, projected, empty, and ordinarily failed rows, plus terminal `answers` collisions under `--on-error keep|record` before request dispatch.
- [x] 1.2 Update command help, README, usage skill, and changelog for `answers` paths and the `--into jev` migration option; verify the documented Nu examples against mock responses.
- [x] 1.3 Run `cargo fmt`, `cargo clippy --all-targets --all-features`, and `cargo nextest run --all-features --all-targets --locked`; fix findings from the answer-destination stage.

## 2. Combined-contract verification

- [x] 2.1 With `add-request-metrics` integrated, verify a default successful live row contains `answers` and fixed `jev_meta` plus optional fixed `jev_metrics`; `--into jev` changes only the answer destination, and annotation rejects `--meta-into` and `--metrics-into`. Cover terminal source collisions despite `--on-error keep|record`, ordinary failed rows, duplicate sharing, and ordered/unordered output in real-Nu tests without changing metric semantics.
- [x] 2.2 Run `openspec validate --all --strict --no-interactive`, `cargo fmt --check`, `cargo clippy --all-targets --all-features`, and `cargo nextest run --all-features --all-targets --locked`; verify the two changes leave no contradictory annotation output contract.
