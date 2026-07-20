#![allow(unused_unsafe, static_mut_refs)]
//! Plix runtime library (plixrt).
//!
//! This crate is used twice:
//!  1. linked (rlib) into the `plix` toolchain — interpreter and compiler
//!     share these semantics;
//!  2. compiled as a staticlib and linked into every native executable
//!     produced by `plix build` (Cranelift-generated code calls the
//!     `plix_*` extern "C" functions).
//!
//! No external dependencies: only the Rust standard library.  The Python
//! bridge uses raw dlopen/dlsym on libpython at runtime (see pyffi.rs),
//! so Plix binaries never need Python headers or link flags.

pub mod builtins;
pub mod heap;
pub mod net;
pub mod pyffi;
pub mod value;
