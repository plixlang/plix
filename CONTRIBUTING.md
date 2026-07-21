# Contributing to Plix

Thanks for improving Plix. This repository contains a Rust toolchain, embedded
runtime, language examples, and language-level test suites.

## Before opening a pull request

1. Create a focused branch from the current development branch.
2. Keep behavior changes accompanied by tests.
3. Update user-facing documentation, examples, and `CHANGELOG.md` when needed.
4. Run the local quality commands:

```sh
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo build
./target/debug/plix test tests
bash tests/run_all.sh ./target/debug/plix
bash tests/fuzz_parity.sh 40 ./target/debug/plix
```

## Test selection

- Rust unit tests belong next to deterministic Rust logic under `#[cfg(test)]`.
- Language behavior belongs in `tests/*_test.px`.
- Native compiler/runtime changes require a parity test or an existing parity
  suite that demonstrably covers the behavior.
- A bug fix should include a regression test.

## Change scope

Avoid mixing unrelated refactors, feature work, and release metadata changes.
Breaking language, CLI, standard-library, manifest, or FFI changes require a
migration note and maintainer approval. v1.0.0 release constraints are defined
in [`plix_roadmap_1_0_0.md`](plix_roadmap_1_0_0.md).

## Pull request description

State the problem, design, user-visible impact, tests run, documentation
changes, compatibility impact, and any remaining limitations. Never include
secrets, private credentials, or production data in tests or issues.

## Community standards

By participating, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
Report security vulnerabilities privately as described in [SECURITY.md](SECURITY.md).
