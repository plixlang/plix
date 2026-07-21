//! Plix library crate — shared types and utilities.
//!
//! Runtime stdlib modules (implemented in `rt/src/builtins.rs`):
//!   - `fs`      — file system operations
//!   - `sys`     — system info, env vars, process execution
//!   - `net`     — HTTP client/server
//!   - `py`      — Python bridge (FFI)
//!   - `ai`      — AI/numpy helpers
//!   - `forge`   — Rust ecosystem bridge (cargo, versions)
//!   - `docker`  — container management via Docker CLI (v0.9.6)
//!   - `security`— sandbox execution, file access control, hashing (v0.9.6)
//!   - `docs`    — doc comment extraction + JSON/HTML/Markdown (v0.9.6)
//!   - `lsp`     — Language Server Protocol utilities (v0.9.6)
//!   - `wasm`    — WebAssembly output utilities (v0.9.6)
//!   - `ffi`     — zero-copy foreign function interface (v0.9.6)
//!
//! Toolchain features added in v0.9.6:
//!   - `plix lsp` command — full LSP server over stdin/stdout JSON-RPC
//!   - `plix build --target wasm` — WASM codegen backend
//!
//! Previously removed placeholder modules (async_await, concurrency, jit,
//! macros, zero_copy, api_docs) have been superseded by proper
//! implementations.  Remaining modules (async_await, concurrency, jit,
//! macros) will be re-introduced before the v1.0.0 release.
