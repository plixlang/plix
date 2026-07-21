# Plix

> A small, gradually typed programming language with an interpreter and a
> native compiler.

**Current development release: v0.9.5.** Plix source files use the `.px`
extension. The toolchain is implemented in Rust and provides two execution
paths with the same language semantics:

- `plix run` interprets a program for fast iteration and readable diagnostics.
- `plix build` produces a native executable through Cranelift.
- `plix exec` compiles and immediately runs a program.

> **Status.** v0.9.5 is a stabilization release on the road to v1.0.0. The
> compatibility and LTS commitments described in
> [`plix_roadmap_1_0_0.md`](plix_roadmap_1_0_0.md) apply only when v1.0.0 is
> released; pre-1.0 APIs may still evolve.

## Quick start

### Run from a release archive

Download the archive for your platform from the GitHub Releases page, extract
it, put `bin/plix` (or `bin/plix.exe`) on your `PATH`, then verify it:

```sh
plix --version
plix run examples/typed.px
plix build examples/closures_match.px -o closures
./closures
```

See the complete platform and linker requirements in
[`docs/install.md`](docs/install.md).

### Build from source

Plix requires a current stable Rust toolchain and a host C linker for native
executables. Python is **not** required unless using the optional Python FFI.

```sh
git clone https://github.com/plixlang/Plix.git
cd Plix
cargo build --release --locked
./target/release/plix --version
bash tests/run_all.sh ./target/release/plix
```

## A first program

```plix
func fib(n: int) -> int {
    if (n <= 1) { return n; }
    return fib(n - 1) + fib(n - 2);
}

struct Vec2 { x: float, y: float }
impl Vec2 {
    func norm2(&self) -> float { return self.x * self.x + self.y * self.y; }
}

say("fib(20) = ${fib(20)}");
say("norm² = ${Vec2 { x: 3.0, y: 4.0 }.norm2()}");
```

Save it as `app.px`, then run either:

```sh
plix run app.px              # interpreter
plix build app.px -o app     # native executable
./app
```

## Language and runtime highlights

- **Gradual typing:** type annotations are optional; the checker reports
  provable mismatches and native compilation specializes provably typed values.
- **Data-oriented OOP:** `struct`, `impl`, and `trait`, with `&self` and
  `&mut self` receivers; there is no data inheritance.
- **Ownership where required:** `own`, `&`, and `&mut` enable static ownership
  and borrowing checks. `auto` and `const` values use the runtime memory model.
- **Everyday language features:** functions and closures, arrays and objects,
  pattern matching, loops, modules, string interpolation, slicing, and errors.
- **Native and interpreted execution:** the repository checks observable
  output parity between both paths for its examples, guards, and fuzz inputs.
- **Optional Python FFI:** `import py "numpy" as np;` accesses CPython through
  the C API. Read the FFI safety and installation notes before using it.

## Command line reference

| Command | Purpose |
|---|---|
| `plix run [file.px] [args...]` | Interpret a program. |
| `plix build [file.px] -o <file>` | Compile a native executable. |
| `plix exec [file.px] [args...]` | Compile and execute natively. |
| `plix check <file.px>` | Parse, type-check, and ownership-check. |
| `plix test [paths...]` | Run `*_test.px` test suites. |
| `plix fmt [--check] [paths...]` | Format Plix source. |
| `plix lint [paths...]` | Report built-in lint diagnostics. |
| `plix repl` | Start the interactive shell. |

When a project contains `plix.toml`, the run/build/test commands can discover
its configured entry point and test paths. See [`docs/tooling.md`](docs/tooling.md).

## Documentation

Start here:

- [Installation and supported platforms](docs/install.md)
- [Language grammar reference](docs/grammar.md) and [grammar guide](docs/grammar_guide.md)
- [Gradual typing](docs/typing.md), [ownership and memory](docs/memory.md), and
  [structs, traits, and impls](docs/oop.md)
- [Standard library](docs/stdlib.md) and [Python FFI](docs/ffi-python.md)
- [Tooling and project manifests](docs/tooling.md)
- [Testing and quality gates](docs/testing.md)
- [Security model and vulnerability reporting](docs/security.md)
- [Release process](docs/release.md), [changelog](CHANGELOG.md), and the
  [v1.0.0 roadmap](plix_roadmap_1_0_0.md)

## Quality and development

The release verification battery runs examples through interpreter and native
modes, runs negative checker suites, and compares their normalized output.
A deterministic parity fuzzer generates additional programs for both modes.

```sh
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo build --release --locked
bash tests/run_all.sh ./target/release/plix
bash tests/fuzz_parity.sh 150 ./target/release/plix
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the contributor workflow and
[`docs/testing.md`](docs/testing.md) for what each command proves.

## Project layout

```text
src/        CLI, lexer, parser, checker, interpreter, native code generator
rt/         embedded runtime: tagged values, heap, standard-library services,
            networking, and optional Python FFI
examples/   runnable language examples
tests/      Plix test suites, parity guards, and deterministic fuzzing
docs/       user, language, tooling, security, testing, and release documents
bench/      benchmark programs and harness
```

## Community and security

- Contributions: [`CONTRIBUTING.md`](CONTRIBUTING.md)
- Code of conduct: [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)
- Private security reports: [`SECURITY.md`](SECURITY.md)
- License: [MIT](LICENSE)
