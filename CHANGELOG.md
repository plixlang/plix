# Changelog

All notable user-facing changes are documented here. Plix follows
[Semantic Versioning](https://semver.org/) after v1.0.0. Until then, releases
may make compatible or incompatible changes; migration notes will identify
any known breakage.

## [0.9.6] - 2026-07-21

### Added

- **docker module** (12 functions): `available`, `run`, `build`, `pull`, `images`, `ps`, `stop`, `rm`, `rmi`, `logs`, `inspect`, `exec` — full container lifecycle management via the Docker CLI.
- **security module** (12 functions): `hash_str`, `hash_file`, `verify_str`, `sandbox_run`, `sandbox_check`, `allowed_dirs`, `add_allowed_dir`, `network`, `set_network`, `max_memory_mb`, `set_max_memory_mb`, `available` — hashing, sandboxing, and resource limits.
- **docs module** (5 functions): `extract`, `json`, `html`, `markdown`, `search` — doc-comment extraction with JSON, HTML, and Markdown rendering.
- **lsp module** (5 functions): `version`, `capabilities`, `format_request`, `parse_message`, `start` — Language Server Protocol utilities; `plix lsp` starts a full JSON-RPC server with completion, diagnostics, hover, formatting, and document symbols.
- **wasm module** (4 functions): `compile`, `validate`, `magic`, `version` — compile Plix source to a valid WASM binary (uses the real AST-based codegen backend).
- **ffi module** (23 functions): `load`, `close`, `call`, `buffer`, `buffer_len`, `read_u8/i8/i16/i32/i64/f32/f64`, `write_u8/i8/i32/i64/f32/f64`, `buffer_to_array`, `array_to_buffer`, `sizeof`, `alignof` — zero-copy dlopen/dlsym FFI with typed buffer access.
- **WASM codegen backend** (`plix build --target wasm`): full AST→WASM binary compiler supporting integer arithmetic, comparisons, logical operators, if/else, while loops, function declarations/calls, local variables, `say()` for integers (including negatives) and string literals via WASI `fd_write`.
- `HeapObj::Buffer(Vec<u8>)` and `HeapObj::ForeignLib(*mut c_void)` runtime variants for zero-copy FFI.
- `fs.write` / `fs.append` now accept Buffer arguments for raw binary writes.

### Changed

- Interpreter overflow promotion: integer overflow now promotes to float instead of raising a runtime error.
- Formatter rewritten with comment/blank-line preservation, escape re-escaping, float `.0` suffix, and keyword spacing fixes.

### Fixed

- WASM `say()` digit-reversal bug: digits are now written backwards from the end of the scratch buffer, producing correct output.
- WASM negative number support: `say(-7)` now outputs `-7` correctly.
- WASM LEB128 encoding for `i32.const` values ≥ 64 (0x40 was decoded as -64).
- WASM memory layout: data segments start at offset 128 to avoid collision with helper scratch area.
- `wasm.compile` stdlib function now invokes the real compiler (previously generated broken stub WASM).
- `tests/run_all.sh` ownership-error grep pattern fixed from `E03` to `E0[35]`.

## [Unreleased]

No unreleased changes are recorded yet.

## [0.9.5] - 2026-07-21

### Changed

- Unified the toolchain and embedded-runtime package versions at `0.9.5`; the
  CLI now derives its version from Cargo package metadata to avoid a third,
  drifting version constant.
- Reworked the top-level documentation, installation, tooling, testing,
  security, contribution, and release guidance.
- Added a public v1.0.0 stable/LTS roadmap with measurable release gates.
- Expanded the GitHub Actions quality and packaging workflow.

### Added

- Rust unit coverage for lexer, parser, and project-manifest behavior.
- Release preflight verification for version consistency and the release test
  battery.

### Fixed

- Ignored local packaging and parity-test artifacts so they are not accidentally
  committed.

## [0.9.0]

- Historical development release. Consult the Git history for the complete
  pre-0.9.5 change set.
