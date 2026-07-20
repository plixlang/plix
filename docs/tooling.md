# Plix Tooling & Project System (v0.5)

Plix v0.5 adds first-class project tooling while keeping single-file scripts
working exactly as before.

## Project manifest: `plix.toml`

At the root of a project:

```toml
[package]
name = "my_app"
version = "0.1.0"

[build]
entry = "src/main.px"
out = "target/my_app"

[test]
paths = ["tests"]
```

When no input file is passed, `plix run`, `plix exec`, `plix build`, and
`plix test` walk upward from the current directory and use `plix.toml`.

```bash
plix run          # runs [build].entry
plix exec         # native compile+run of [build].entry
plix build        # builds [build].entry to [build].out
plix test         # uses [test].paths
```

Passing explicit paths still overrides the manifest.

## Formatter

```bash
plix fmt file.px
plix fmt src/ tests/
plix fmt --check .
```

The formatter validates syntax before rewriting and recursively formats `.px`
files while skipping build/tool directories.

## Linter

```bash
plix lint file.px
plix lint src/ tests/
```

Current warnings:

| code | meaning |
|---|---|
| `W0001` | unused variable/function/binding |
| `W0002` | unused import |
| `W0003` | unreachable statement |

Linting runs parse + typecheck + ownership check before reporting warnings, so
invalid programs fail with normal Plix diagnostics.

## Test runner improvements

```bash
plix test --filter option
plix test --fail-fast
plix test --json
```

`--json` is intended for CI and editor integrations.
