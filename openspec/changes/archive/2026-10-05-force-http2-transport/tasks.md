# Tasks

## 1. Force the transport policy

- [x] 1.1 Add default-enabled `http2-prior-knowledge` Cargo feature in every pooled reqwest client; verify default-build HTTP/2 for evaluation and model discovery, HTTP/1.1-only failure, and feature-off negotiation.
- [x] 1.2 Document default-build HTTP/2 requirements and the compile-time compatibility choice in README; keep transport-build details out of the usage skill.

## 2. Preserve test coverage on HTTP/2

- [x] 2.1 Use a ready-made HTTP/2 server for API and command-level network tests without disabling the default transport policy; verify retry, body-limit, proxy, and result-contract behavior.
- [x] 2.2 Migrate stream and real-Nushell network fixtures to the ready-made HTTP/2 server, counting multiplexed streams rather than sockets; verify relevant nextest targets.

## 3. Integration checks

- [x] 3.1 Run `cargo fmt`, `cargo clippy --all-targets --all-features`, `cargo nextest run --all-features --all-targets --locked`, focused feature-off tests, strict OpenSpec validation, and `git diff --check`; fix findings.
