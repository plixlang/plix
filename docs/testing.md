# Testing and quality gates

This document describes the verification required for a Plix change and what
each test layer covers. Passing one layer does not replace the others.

## Fast local loop

```sh
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
./target/debug/plix test tests
```

- `cargo fmt` checks Rust formatting.
- `cargo check` type-checks the Rust workspace without producing an optimized
  binary.
- `cargo test` executes Rust unit tests for lexer, parser, manifest handling,
  and future Rust components.
- `plix test tests` executes language-level `*_test.px` suites.

## Native/interpreter parity

Plix has two execution paths. Any language/runtime change must preserve the
observable behavior of both unless an intentional, documented compatibility
change has been approved.

```sh
cargo build
bash tests/run_all.sh ./target/debug/plix
bash tests/fuzz_parity.sh 150 ./target/debug/plix
```

`tests/run_all.sh` runs selected examples in interpreter and native modes,
normalizes non-deterministic diagnostics, and requires matching output. It also
runs negative type and ownership checker suites. `tests/fuzz_parity.sh` creates
deterministic generated programs and compares both output **and exit status**.

A failure should preserve the generated input (`target/fuzz/fail_<seed>.px`) so
it can become a minimized regression test.

## Release gate

Before a release, run:

```sh
bash tests/release_preflight.sh ./target/release/plix
```

The preflight checks package-version consistency, confirms the CLI reports the
same version, runs the dual-mode battery, and runs a deterministic fuzz sample.
For the final release candidate, use the full 150-seed command shown above.

## Test policy

1. Add a Rust unit test for deterministic, isolated Rust logic (lexer, parser,
   manifest parsing, formatter, type checker, or runtime helpers).
2. Add a `.px` test for user-visible language behavior.
3. Add or update an interpreter/native parity case when code generation or the
   runtime is involved.
4. Reproduce every fixed defect with a test before or alongside the fix.
5. Keep tests deterministic; do not depend on external network services or
   local user state in the default suite.

## CI

GitHub Actions checks formatting, compilation, Rust unit tests, Plix tests,
dual-mode parity, and fuzz parity on Ubuntu. Release packaging builds and
smoke-tests the supported binary artifacts. Platform-specific functionality
must be tested on its target runner before being marked supported.
