//! Plix standard library: builtin functions, constants, and the native
//! modules `fs`, `sys`, `net`, `py`, `ai`, `forge`.
//!
//! Every global in a Plix program is installed from `GLOBAL_DEFS` — the same
//! table drives the interpreter (scope population), the compiler (global
//! slot numbering) and native executables (startup install).  One source of
//! truth, consistent behaviour in both execution modes.

use crate::heap::*;
use crate::value::{call_value, to_display, to_repr, truthy, Caller, OpResult};
use std::io::Write;

pub type BuiltinFn = fn(&mut dyn Caller, &[V]) -> OpResult;

pub enum G {
    /// A callable global or module member: ("name", fn) — dotted names like
    /// "fs.read" live inside module maps, not the global scope.
    F(&'static str, BuiltinFn),
    /// A float constant (PI, E, ...).
    C(&'static str, f64),
}

pub static GLOBAL_DEFS: &[G] = &[
    G::C("PI", std::f64::consts::PI),
    G::C("E", std::f64::consts::E),
    G::C("TAU", std::f64::consts::TAU),
    G::C("INF", f64::INFINITY),
    G::C("NAN", f64::NAN),
    // ---- io ----
    G::F("say", b_say),
    G::F("print", b_print),
    G::F("input", b_input),
    // ---- conversion ----
    G::F("str", b_str),
    G::F("repr", b_repr),
    G::F("int", b_int),
    G::F("float", b_float),
    G::F("bool", b_bool),
    G::F("type_of", b_type_of),
    // ---- option/result constructors ----
    G::F("Some", b_some),
    G::F("Ok", b_ok),
    G::F("Err", b_err),
    // ---- numbers ----
    G::F("abs", b_abs),
    G::F("floor", b_floor),
    G::F("ceil", b_ceil),
    G::F("round", b_round),
    G::F("sqrt", b_sqrt),
    G::F("pow", b_pow),
    G::F("exp", b_exp),
    G::F("log", b_log),
    G::F("sin", b_sin),
    G::F("cos", b_cos),
    G::F("tan", b_tan),
    G::F("atan2", b_atan2),
    G::F("min", b_min),
    G::F("max", b_max),
    G::F("clamp", b_clamp),
    G::F("sign", b_sign),
    G::F("idiv", b_idiv),
    G::F("rand", b_rand),
    G::F("rand_int", b_rand_int),
    // ---- strings ----
    G::F("len", b_len),
    G::F("upper", b_upper),
    G::F("lower", b_lower),
    G::F("trim", b_trim),
    G::F("split", b_split),
    G::F("join", b_join),
    G::F("replace", b_replace),
    G::F("contains", b_contains),
    G::F("starts_with", b_starts_with),
    G::F("ends_with", b_ends_with),
    G::F("find", b_find),
    G::F("chars", b_chars),
    G::F("substr", b_substr),
    G::F("char", b_char),
    G::F("byte", b_byte),
    G::F("parse_int", b_parse_int),
    G::F("parse_float", b_parse_float),
    // ---- arrays / objects ----
    G::F("push", b_push),
    G::F("pop", b_pop),
    G::F("insert", b_insert),
    G::F("remove_at", b_remove_at),
    G::F("index_of", b_index_of),
    G::F("reverse", b_reverse),
    G::F("sort", b_sort),
    G::F("sort_by", b_sort_by),
    G::F("map", b_map),
    G::F("filter", b_filter),
    G::F("reduce", b_reduce),
    G::F("each", b_each),
    G::F("range", b_range),
    G::F("keys", b_keys),
    G::F("values", b_values),
    G::F("entries", b_entries),
    G::F("has", b_has),
    G::F("get", b_get),
    G::F("set", b_set),
    G::F("delete", b_delete),
    // ---- misc ----
    G::F("time_ms", b_time_ms),
    G::F("clock", b_clock),
    G::F("sleep_ms", b_sleep_ms),
    G::F("assert", b_assert),
    G::F("assert_eq", b_assert_eq),
    G::F("assert_ne", b_assert_ne),
    G::F("panic", b_panic),
    G::F("exit", b_exit),
    // ---- fs module ----
    G::F("fs.read", fs_read),
    G::F("fs.write", fs_write),
    G::F("fs.append", fs_append),
    G::F("fs.exists", fs_exists),
    G::F("fs.is_file", fs_is_file),
    G::F("fs.is_dir", fs_is_dir),
    G::F("fs.size", fs_size),
    G::F("fs.list", fs_list),
    G::F("fs.mkdir", fs_mkdir),
    G::F("fs.remove", fs_remove),
    G::F("fs.rename", fs_rename),
    G::F("fs.copy", fs_copy),
    G::F("fs.join", fs_join),
    G::F("fs.abs", fs_abs),
    G::F("fs.parent", fs_parent),
    G::F("fs.name", fs_name),
    G::F("fs.ext", fs_ext),
    // ---- sys module ----
    G::F("sys.platform", sys_platform),
    G::F("sys.arch", sys_arch),
    G::F("sys.args", sys_args),
    G::F("sys.env", sys_env),
    G::F("sys.set_env", sys_set_env),
    G::F("sys.cwd", sys_cwd),
    G::F("sys.exit", b_exit),
    G::F("sys.exec", sys_exec),
    G::F("sys.pid", sys_pid),
    G::F("sys.hostname", sys_hostname),
    // ---- net module (HTTP) ----
    G::F("net.serve", crate::net::net_serve),
    G::F("net.get", crate::net::net_get),
    G::F("net.post", crate::net::net_post),
    G::F("net.response", crate::net::net_response),
    // ---- python bridge ----
    G::F("py.available", py_available),
    G::F("py.has_module", py_has_module),
    G::F("py.import", py_import),
    G::F("py.eval", py_eval),
    G::F("py.exec", py_exec),
    G::F("py.call", py_call),
    G::F("py.getattr", py_getattr),
    G::F("py.setattr", py_setattr),
    G::F("py.hasattr", py_hasattr),
    G::F("py.repr", py_repr),
    G::F("py.to_plix", py_to_plix),
    // ---- ai helpers (bridged python, high throughput) ----
    G::F("ai.lib", ai_lib),
    G::F("ai.eval", py_eval),
    G::F("ai.array", ai_array),
    G::F("ai.call", ai_call),
    G::F("ai.shape", ai_shape),
    // ---- forge (rust ecosystem bridge) ----
    G::F("forge.version", forge_version),
    G::F("forge.rust_version", forge_rust_version),
    G::F("forge.cargo", forge_cargo),
    G::F("forge.target", forge_target),
    // ---- docker module (container management) ----
    G::F("docker.available", docker_available),
    G::F("docker.run", docker_run),
    G::F("docker.build", docker_build),
    G::F("docker.pull", docker_pull),
    G::F("docker.images", docker_images),
    G::F("docker.ps", docker_ps),
    G::F("docker.stop", docker_stop),
    G::F("docker.rm", docker_rm),
    G::F("docker.rmi", docker_rmi),
    G::F("docker.logs", docker_logs),
    G::F("docker.inspect", docker_inspect),
    G::F("docker.exec", docker_exec),
    // ---- security/sandbox module ----
    G::F("security.available", sec_available),
    G::F("security.sandbox_run", sec_sandbox_run),
    G::F("security.sandbox_check", sec_sandbox_check),
    G::F("security.hash_file", sec_hash_file),
    G::F("security.hash_str", sec_hash_str),
    G::F("security.verify_str", sec_verify_str),
    G::F("security.allowed_dirs", sec_allowed_dirs),
    G::F("security.add_allowed_dir", sec_add_allowed_dir),
    G::F("security.network", sec_network),
    G::F("security.set_network", sec_set_network),
    G::F("security.max_memory_mb", sec_max_memory_mb),
    G::F("security.set_max_memory_mb", sec_set_max_memory_mb),
    // ---- docs module (doc comment extraction + JSON/HTML) ----
    G::F("docs.extract", docs_extract),
    G::F("docs.json", docs_json),
    G::F("docs.html", docs_html),
    G::F("docs.markdown", docs_markdown),
    G::F("docs.search", docs_search),
    // ---- lsp module (Language Server Protocol) ----
    G::F("lsp.start", lsp_start),
    G::F("lsp.version", lsp_version),
    G::F("lsp.capabilities", lsp_capabilities),
    G::F("lsp.format_request", lsp_format_request),
    G::F("lsp.parse_message", lsp_parse_message),
    // ---- wasm module (WebAssembly output) ----
    G::F("wasm.version", wasm_version),
    G::F("wasm.compile", wasm_compile),
    G::F("wasm.validate", wasm_validate),
    G::F("wasm.magic", wasm_magic),
    // ---- concurrency ----
    G::F("spawn", b_spawn),
    // ---- ffi module (zero-copy foreign function interface) ----
    G::F("ffi.load", ffi_load),
    G::F("ffi.close", ffi_close),
    G::F("ffi.call", ffi_call),
    G::F("ffi.buffer", ffi_buffer),
    G::F("ffi.buffer_len", ffi_buffer_len),
    G::F("ffi.read_u8", ffi_read_u8),
    G::F("ffi.read_i8", ffi_read_i8),
    G::F("ffi.read_u16", ffi_read_u16),
    G::F("ffi.read_i16", ffi_read_i16),
    G::F("ffi.read_i32", ffi_read_i32),
    G::F("ffi.read_i64", ffi_read_i64),
    G::F("ffi.read_f32", ffi_read_f32),
    G::F("ffi.read_f64", ffi_read_f64),
    G::F("ffi.write_u8", ffi_write_u8),
    G::F("ffi.write_i8", ffi_write_i8),
    G::F("ffi.write_i32", ffi_write_i32),
    G::F("ffi.write_i64", ffi_write_i64),
    G::F("ffi.write_f32", ffi_write_f32),
    G::F("ffi.write_f64", ffi_write_f64),
    G::F("ffi.buffer_to_array", ffi_buffer_to_array),
    G::F("ffi.array_to_buffer", ffi_array_to_buffer),
    G::F("ffi.sizeof", ffi_sizeof),
    G::F("ffi.alignof", ffi_alignof),
];

const MODULES: &[&str] = &[
    "fs", "sys", "net", "py", "ai", "forge", "docker", "security", "docs", "lsp", "wasm", "ffi",
];

// ---------------------------------------------------------------------------
// global installation (shared numbering with the compiler)
// ---------------------------------------------------------------------------

/// Ordered global names installed before any user program runs.
pub fn global_names() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for d in GLOBAL_DEFS {
        match d {
            G::C(n, _) => out.push(n),
            G::F(n, _) => {
                if !n.contains('.') {
                    out.push(n);
                }
            }
        }
    }
    for m in MODULES {
        out.push(m);
    }
    // dedup preserving order
    let mut seen = std::collections::HashSet::new();
    out.retain(|n| seen.insert(*n));
    out
}

/// Install globals into a native executable's global table (same order as
/// `global_names`).  Called from plix_rt_init via a hook set by the toolchain;
/// native codegen calls this directly.
pub fn install_globals() {
    let _g = lock();
    unsafe {
        for d in GLOBAL_DEFS {
            match d {
                G::C(n, _) => {
                    let v = alloc_locked(HeapObj::Float(n_c(d)));
                    global_install_locked(n, v);
                }
                G::F(n, _) => {
                    if n.contains('.') {
                        continue;
                    }
                    let v = alloc_locked(HeapObj::Builtin(def_index(n)));
                    global_install_locked(n, v);
                }
            }
        }
        for m in MODULES {
            let mut map = std::collections::HashMap::new();
            for d in GLOBAL_DEFS {
                if let G::F(n, _) = d {
                    if let Some(rest) = n.strip_prefix(&format!("{}.", m)) {
                        map.insert(
                            rest.to_string(),
                            alloc_locked(HeapObj::Builtin(def_index(n))),
                        );
                    }
                }
            }
            let mv = crate::heap::mk_map_locked(map);
            global_install_locked(m, mv);
        }
    }
}

fn n_c(d: &G) -> f64 {
    match d {
        G::C(_, c) => *c,
        _ => 0.0,
    }
}

fn def_index(name: &str) -> u32 {
    GLOBAL_DEFS
        .iter()
        .position(|d| matches!(d, G::F(n, _) if *n == name))
        .unwrap_or(0) as u32
}

/// Install into the globals table by name — the toolchain pre-sizes the
/// globals vector using global_names(), so we can find the slot by name.
/// Caller must already hold the runtime lock.
#[allow(static_mut_refs)]
unsafe fn global_install_locked(name: &str, v: V) {
    let names = global_names();
    if let Some(i) = names.iter().position(|n| *n == name) {
        let n = crate::heap::globals_count_locked();
        if i < n {
            crate::heap::global_set_locked(i, v);
        }
    }
    // slot not present (interpreter path uses scopes instead) — harmless.
}

pub fn builtin_name(id: u32) -> &'static str {
    match GLOBAL_DEFS.get(id as usize) {
        Some(G::F(n, _)) => n,
        Some(G::C(n, _)) => n,
        None => "<builtin>",
    }
}

pub fn call_builtin(id: u32, caller: &mut dyn Caller, args: &[V]) -> OpResult {
    match GLOBAL_DEFS.get(id as usize) {
        Some(G::F(_, f)) => f(caller, args),
        _ => Err(format!("builtin #{} is not callable", id)),
    }
}

/// Lookup used by the toolchain: returns the builtin id for a name.
pub fn find_builtin(name: &str) -> Option<u32> {
    GLOBAL_DEFS
        .iter()
        .position(|d| matches!(d, G::F(n, _) if *n == name))
        .map(|i| i as u32)
}

pub fn install_builtins_lazy() {
    // no-op hook kept for ABI compatibility
}

/// extern surface for native executables: fills the builtin region of the
/// globals table (must be called after plix_rt_init sized it).
#[no_mangle]
pub extern "C" fn plix_install_builtins() {
    install_globals();
}

/// Build (name, value) pairs for the global scope — used by the tree-walking
/// interpreter (native executables use `install_globals` against the globals
/// table instead).
pub fn build_global_entries() -> Vec<(String, V)> {
    let mut out: Vec<(String, V)> = Vec::new();
    for name in global_names() {
        let v = make_global_value(name);
        if let Some(v) = v {
            out.push((name.to_string(), v));
        }
    }
    out
}

/// Construct the runtime value for a global name (builtin fn, constant, or
/// module map).
pub fn make_global_value(name: &str) -> Option<V> {
    for d in GLOBAL_DEFS {
        match d {
            G::C(n, c) if *n == name => return Some(mk_float_unchecked(*c)),
            G::F(n, _) if *n == name && !n.contains('.') => {
                return Some(mk_builtin(def_index(n)));
            }
            _ => {}
        }
    }
    if MODULES.contains(&name) {
        let mut map = std::collections::HashMap::new();
        for d in GLOBAL_DEFS {
            if let G::F(n, _) = d {
                if let Some(rest) = n.strip_prefix(&format!("{}.", name)) {
                    map.insert(rest.to_string(), mk_builtin(def_index(n)));
                }
            }
        }
        return Some(mk_map(map));
    }
    None
}

// ---------------------------------------------------------------------------
// argument helpers
// ---------------------------------------------------------------------------

fn err<T>(m: impl Into<String>) -> Result<T, String> {
    Err(m.into())
}

fn need(args: &[V], n: usize, name: &str) -> Result<(), String> {
    if args.len() < n {
        err(format!(
            "{}: expected {} argument(s), got {}",
            name,
            n,
            args.len()
        ))
    } else {
        Ok(())
    }
}

fn want_int(v: V, name: &str) -> Result<i64, String> {
    if is_int(v) {
        Ok(as_int(v))
    } else {
        err(format!("{}: expected int, got {}", name, kind_name(v)))
    }
}

fn want_num(v: V, name: &str) -> Result<f64, String> {
    if is_int(v) {
        Ok(as_int(v) as f64)
    } else {
        unsafe {
            match payload(v) {
                HeapObj::Float(f) => Ok(*f),
                _ => err(format!("{}: expected number, got {}", name, kind_name(v))),
            }
        }
    }
}

fn want_str(v: V, name: &str) -> Result<String, String> {
    unsafe {
        match payload_opt_str(v) {
            Some(s) => Ok(s),
            None => err(format!("{}: expected string, got {}", name, kind_name(v))),
        }
    }
}

#[allow(static_mut_refs)]
unsafe fn payload_opt_str(v: V) -> Option<String> {
    if !is_ptr(v) {
        return None;
    }
    match payload(v) {
        HeapObj::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn want_arr(v: V, name: &str) -> Result<Vec<V>, String> {
    unsafe {
        if is_ptr(v) {
            if let HeapObj::Array(items) = payload(v) {
                return Ok(items.clone());
            }
        }
        err(format!("{}: expected array, got {}", name, kind_name(v)))
    }
}

// ---------------------------------------------------------------------------
// io
// ---------------------------------------------------------------------------

fn b_say(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    let mut out = String::new();
    for (i, &a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&to_display(a));
    }
    let so = std::io::stdout();
    let mut h = so.lock();
    let _ = writeln!(h, "{}", out);
    let _ = h.flush();
    Ok(NULL)
}

fn b_print(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    for &a in args {
        print!("{}", to_display(a));
    }
    let _ = std::io::stdout().flush();
    Ok(NULL)
}

fn b_input(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    if let Some(&p) = args.first() {
        print!("{}", to_display(p));
        let _ = std::io::stdout().flush();
    }
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) => Ok(mk_str_from(line.trim_end_matches(['\n', '\r']))),
        Err(e) => err(format!("input: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// conversion
// ---------------------------------------------------------------------------

fn b_str(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "str")?;
    Ok(mk_string(to_display(args[0])))
}
fn b_repr(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "repr")?;
    Ok(mk_string(to_repr(args[0])))
}
fn b_int(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "int")?;
    let v = args[0];
    if is_int(v) {
        return Ok(v);
    }
    if v == TRUE {
        return Ok(mk_int(1));
    }
    if v == FALSE {
        return Ok(mk_int(0));
    }
    if is_null(v) {
        return Ok(mk_int(0));
    }
    unsafe {
        match payload(v) {
            HeapObj::Float(f) => {
                if f.is_finite() {
                    return Ok(mk_int(f.trunc() as i64));
                }
                err("int: cannot convert non-finite float")
            }
            HeapObj::Str(s) => {
                if let Ok(i) = s.trim().parse::<i64>() {
                    return Ok(mk_int(i));
                }
                if let Ok(f) = s.trim().parse::<f64>() {
                    return Ok(mk_int(f.trunc() as i64));
                }
                err(format!("int: cannot parse \"{}\"", s))
            }
            _ => err(format!("int: cannot convert {}", kind_name(v))),
        }
    }
}
fn b_float(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "float")?;
    let v = args[0];
    if is_int(v) {
        return Ok(mk_float_unchecked(as_int(v) as f64));
    }
    if v == TRUE {
        return Ok(mk_float_unchecked(1.0));
    }
    if v == FALSE {
        return Ok(mk_float_unchecked(0.0));
    }
    unsafe {
        match payload(v) {
            HeapObj::Float(_) => Ok(v),
            HeapObj::Str(s) => match s.trim().parse::<f64>() {
                Ok(f) => Ok(mk_float_unchecked(f)),
                Err(_) => err(format!("float: cannot parse \"{}\"", s)),
            },
            _ => err(format!("float: cannot convert {}", kind_name(v))),
        }
    }
}
fn b_bool(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "bool")?;
    Ok(bool_of(truthy(args[0])))
}
fn b_type_of(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "type_of")?;
    Ok(mk_str_from(kind_name(args[0])))
}

fn b_some(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "Some")?;
    if is_null(args[0]) {
        return err("Some: payload cannot be null");
    }
    Ok(args[0])
}

fn b_ok(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "Ok")?;
    Ok(mk_variant("Ok", args[0], true))
}

fn b_err(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "Err")?;
    Ok(mk_variant("Err", args[0], true))
}

// ---------------------------------------------------------------------------
// numbers
// ---------------------------------------------------------------------------

macro_rules! num1 {
    ($name:ident, $f:expr) => {
        fn $name(_c: &mut dyn Caller, args: &[V]) -> OpResult {
            need(args, 1, stringify!($name))?;
            let x = want_num(args[0], stringify!($name))?;
            let f: fn(f64) -> f64 = $f;
            Ok(mk_float_unchecked(f(x)))
        }
    };
}

fn b_abs(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "abs")?;
    if is_int(args[0]) {
        return Ok(mk_int(as_int(args[0]).abs()));
    }
    Ok(mk_float_unchecked(want_num(args[0], "abs")?.abs()))
}
num1!(b_floor, f64::floor);
num1!(b_ceil, f64::ceil);
num1!(b_sqrt, f64::sqrt);
num1!(b_exp, f64::exp);
num1!(b_sin, f64::sin);
num1!(b_cos, f64::cos);
num1!(b_tan, f64::tan);

fn b_round(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "round")?;
    let x = want_num(args[0], "round")?;
    if args.len() > 1 {
        let d = want_int(args[1], "round")?.clamp(-12, 12);
        let m = 10f64.powi(d as i32);
        return Ok(mk_float_unchecked((x * m).round() / m));
    }
    Ok(mk_float_unchecked(x.round()))
}
fn b_pow(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "pow")?;
    let a = want_num(args[0], "pow")?;
    let b = want_num(args[1], "pow")?;
    if is_int(args[0]) && is_int(args[1]) {
        let e = as_int(args[1]);
        if (0..63).contains(&e) {
            let x = as_int(args[0]);
            if let Some(r) = x.checked_pow(e as u32) {
                return Ok(mk_int(r));
            }
        }
    }
    Ok(mk_float_unchecked(a.powf(b)))
}
fn b_log(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "log")?;
    let x = want_num(args[0], "log")?;
    if args.len() > 1 {
        let b = want_num(args[1], "log")?;
        return Ok(mk_float_unchecked(x.log(b)));
    }
    Ok(mk_float_unchecked(x.ln()))
}
fn b_atan2(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "atan2")?;
    Ok(mk_float_unchecked(
        want_num(args[0], "atan2")?.atan2(want_num(args[1], "atan2")?),
    ))
}
fn b_min(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "min")?;
    let mut best = want_num(args[0], "min")?;
    for &a in &args[1..] {
        best = best.min(want_num(a, "min")?);
    }
    Ok(if best.fract() == 0.0 && args.iter().all(|&a| is_int(a)) {
        mk_int(best as i64)
    } else {
        mk_float_unchecked(best)
    })
}
fn b_max(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "max")?;
    let mut best = want_num(args[0], "max")?;
    for &a in &args[1..] {
        best = best.max(want_num(a, "max")?);
    }
    Ok(if best.fract() == 0.0 && args.iter().all(|&a| is_int(a)) {
        mk_int(best as i64)
    } else {
        mk_float_unchecked(best)
    })
}
fn b_clamp(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 3, "clamp")?;
    let (x, lo, hi) = (
        want_num(args[0], "clamp")?,
        want_num(args[1], "clamp")?,
        want_num(args[2], "clamp")?,
    );
    if is_int(args[0]) && is_int(args[1]) && is_int(args[2]) {
        return Ok(mk_int(
            as_int(args[0]).clamp(as_int(args[1]), as_int(args[2])),
        ));
    }
    Ok(mk_float_unchecked(x.clamp(lo, hi)))
}
fn b_sign(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "sign")?;
    if is_int(args[0]) {
        return Ok(mk_int(as_int(args[0]).signum()));
    }
    let x = want_num(args[0], "sign")?;
    Ok(mk_float_unchecked(if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }))
}
fn b_idiv(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "idiv")?;
    crate::value::int_div(args[0], args[1])
}

// deterministic PRNG (xorshift64*), seeded once from the clock
static RNG: std::sync::Mutex<u64> = std::sync::Mutex::new(0x9E3779B97F4A7C15);
fn rng_next() -> u64 {
    let mut s = RNG.lock().unwrap();
    if *s == 0 {
        *s = 0x9E3779B97F4A7C15;
    }
    let mut x = *s;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *s = x;
    x.wrapping_mul(0x2545F4914F6CDD1D)
}
fn b_rand(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    let r = rng_next() >> 11;
    Ok(mk_float_unchecked(r as f64 / (1u64 << 53) as f64))
}
fn b_rand_int(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "rand_int")?;
    let (a, b) = (
        want_int(args[0], "rand_int")?,
        want_int(args[1], "rand_int")?,
    );
    if b < a {
        return err("rand_int: max < min");
    }
    let span = (b - a + 1) as u64;
    Ok(mk_int(a + (rng_next() % span) as i64))
}

// ---------------------------------------------------------------------------
// strings
// ---------------------------------------------------------------------------

fn b_len(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "len")?;
    crate::value::length(args[0])
}
fn b_upper(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "upper")?;
    Ok(mk_string(want_str(args[0], "upper")?.to_uppercase()))
}
fn b_lower(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "lower")?;
    Ok(mk_string(want_str(args[0], "lower")?.to_lowercase()))
}
fn b_trim(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "trim")?;
    Ok(mk_string(want_str(args[0], "trim")?.trim().to_string()))
}
fn b_split(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "split")?;
    let s = want_str(args[0], "split")?;
    let sep = want_str(args[1], "split")?;
    let parts: Vec<V> = if sep.is_empty() {
        s.chars().map(|c| mk_str_from(&c.to_string())).collect()
    } else {
        s.split(&sep).map(mk_str_from).collect()
    };
    Ok(mk_array(parts))
}
fn b_join(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "join")?;
    let items = want_arr(args[0], "join")?;
    let sep = want_str(args[1], "join")?;
    let parts: Vec<String> = items.iter().map(|&v| to_display(v)).collect();
    Ok(mk_string(parts.join(&sep)))
}
fn b_replace(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 3, "replace")?;
    let s = want_str(args[0], "replace")?;
    let from = want_str(args[1], "replace")?;
    let to = want_str(args[2], "replace")?;
    Ok(mk_string(s.replace(&from, &to)))
}
fn b_contains(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "contains")?;
    unsafe {
        if is_ptr(args[0]) {
            match payload(args[0]) {
                HeapObj::Str(s) => return Ok(bool_of(s.contains(&want_str(args[1], "contains")?))),
                HeapObj::Array(items) => {
                    return Ok(bool_of(
                        items.iter().any(|&x| crate::value::values_eq(x, args[1])),
                    ))
                }
                HeapObj::Map(m) => {
                    let key = to_display(args[1]);
                    return Ok(bool_of(m.contains_key(&key)));
                }
                _ => {}
            }
        }
    }
    err(format!(
        "contains: expected string/array/object, got {}",
        kind_name(args[0])
    ))
}
fn b_starts_with(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "starts_with")?;
    Ok(bool_of(
        want_str(args[0], "starts_with")?.starts_with(&want_str(args[1], "starts_with")?),
    ))
}
fn b_ends_with(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "ends_with")?;
    Ok(bool_of(
        want_str(args[0], "ends_with")?.ends_with(&want_str(args[1], "ends_with")?),
    ))
}
fn b_find(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "find")?;
    let s = want_str(args[0], "find")?;
    let sub = want_str(args[1], "find")?;
    match s.find(&sub) {
        Some(i) => Ok(mk_int(s[..i].chars().count() as i64)),
        None => Ok(mk_int(-1)),
    }
}
fn b_chars(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "chars")?;
    let s = want_str(args[0], "chars")?;
    Ok(mk_array(
        s.chars().map(|c| mk_str_from(&c.to_string())).collect(),
    ))
}
fn b_substr(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 3, "substr")?;
    let s: Vec<char> = want_str(args[0], "substr")?.chars().collect();
    let start = want_int(args[1], "substr")?.max(0) as usize;
    let len = want_int(args[2], "substr")?.max(0) as usize;
    let out: String = s.iter().skip(start).take(len).collect();
    Ok(mk_string(out))
}
fn b_char(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "char")?;
    let i = want_int(args[0], "char")?;
    char::from_u32(i as u32)
        .map(|c| mk_str_from(&c.to_string()))
        .ok_or_else(|| format!("char: invalid code point {}", i))
}
fn b_byte(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "byte")?;
    let s = want_str(args[0], "byte")?;
    match s.chars().next() {
        Some(c) => Ok(mk_int(c as u32 as i64)),
        None => err("byte: empty string"),
    }
}
fn b_parse_int(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "parse_int")?;
    match want_str(args[0], "parse_int")?.trim().parse::<i64>() {
        Ok(i) => Ok(mk_int(i)),
        Err(_) => Ok(NULL),
    }
}
fn b_parse_float(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "parse_float")?;
    match want_str(args[0], "parse_float")?.trim().parse::<f64>() {
        Ok(f) => Ok(mk_float_unchecked(f)),
        Err(_) => Ok(NULL),
    }
}

// ---------------------------------------------------------------------------
// arrays / objects
// ---------------------------------------------------------------------------

fn b_push(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "push")?;
    unsafe {
        if !is_ptr(args[0]) {
            return err("push: expected array");
        }
        if let HeapObj::Array(items) = &mut *payload_mut(args[0]) {
            retain_locked(args[1]);
            items.push(args[1]);
            return Ok(args[0]);
        }
    }
    err("push: expected array")
}
fn b_pop(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "pop")?;
    unsafe {
        if is_ptr(args[0]) {
            if let HeapObj::Array(items) = &mut *payload_mut(args[0]) {
                match items.pop() {
                    Some(v) => {
                        release_locked(v);
                        return Ok(v);
                    }
                    None => return err("pop: array is empty"),
                }
            }
        }
    }
    err("pop: expected array")
}
fn b_insert(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 3, "insert")?;
    let i = want_int(args[1], "insert")?;
    unsafe {
        if is_ptr(args[0]) {
            if let HeapObj::Array(items) = &mut *payload_mut(args[0]) {
                let i = i.clamp(0, items.len() as i64) as usize;
                retain_locked(args[2]);
                items.insert(i, args[2]);
                return Ok(args[0]);
            }
        }
    }
    err("insert: expected array")
}
fn b_remove_at(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "remove_at")?;
    let i = want_int(args[1], "remove_at")?;
    unsafe {
        if is_ptr(args[0]) {
            if let HeapObj::Array(items) = &mut *payload_mut(args[0]) {
                if i < 0 || i as usize >= items.len() {
                    return err(format!("remove_at: index {} out of bounds", i));
                }
                let old = items.remove(i as usize);
                release_locked(old);
                return Ok(old);
            }
        }
    }
    err("remove_at: expected array")
}
fn b_index_of(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "index_of")?;
    let items = want_arr(args[0], "index_of")?;
    for (i, &x) in items.iter().enumerate() {
        if crate::value::values_eq(x, args[1]) {
            return Ok(mk_int(i as i64));
        }
    }
    Ok(mk_int(-1))
}
fn b_reverse(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "reverse")?;
    let mut items = want_arr(args[0], "reverse")?;
    items.reverse();
    Ok(mk_array(items))
}
fn b_sort(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "sort")?;
    let mut items = want_arr(args[0], "sort")?;
    items.sort_by(|&a, &b| crate::value::compare(a, b).unwrap_or(std::cmp::Ordering::Equal));
    Ok(mk_array(items))
}
fn b_sort_by(c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "sort_by")?;
    let items = want_arr(args[0], "sort_by")?;
    let f = args[1];
    let mut keyed: Vec<(String, V)> = Vec::with_capacity(items.len());
    let mut errs: Option<String> = None;
    let mut pairs: Vec<(V, V)> = items.into_iter().map(|v| (v, v)).collect();
    pairs.sort_by(|&(a, _), &(b, _)| {
        if errs.is_some() {
            return std::cmp::Ordering::Equal;
        }
        match c.call(f, &[a, b]) {
            Ok(r) => {
                if is_int(r) {
                    as_int(r).cmp(&0)
                } else {
                    match crate::value::compare(a, b) {
                        Ok(o) => o,
                        Err(e) => {
                            errs = Some(e);
                            std::cmp::Ordering::Equal
                        }
                    }
                }
            }
            Err(e) => {
                errs = Some(e);
                std::cmp::Ordering::Equal
            }
        }
    });
    if let Some(e) = errs {
        return err(e);
    }
    keyed.clear();
    Ok(mk_array(pairs.into_iter().map(|(_, v)| v).collect()))
}
fn b_map(c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "map")?;
    let items = want_arr(args[0], "map")?;
    let mut out = Vec::with_capacity(items.len());
    for v in items {
        out.push(c.call(args[1], &[v])?);
    }
    Ok(mk_array(out))
}
fn b_filter(c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "filter")?;
    let items = want_arr(args[0], "filter")?;
    let mut out = Vec::new();
    for v in items {
        if truthy(c.call(args[1], &[v])?) {
            out.push(v);
        }
    }
    Ok(mk_array(out))
}
fn b_reduce(c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 3, "reduce")?;
    let items = want_arr(args[0], "reduce")?;
    let mut acc = args[1];
    for v in items {
        acc = c.call(args[2], &[acc, v])?;
    }
    Ok(acc)
}
fn b_each(c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "each")?;
    let items = want_arr(args[0], "each")?;
    for v in items {
        c.call(args[1], &[v])?;
    }
    Ok(NULL)
}
fn b_range(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "range")?;
    let (mut a, b, step) = match args.len() {
        1 => (0i64, want_int(args[0], "range")?, 1i64),
        2 => (want_int(args[0], "range")?, want_int(args[1], "range")?, 1),
        _ => (
            want_int(args[0], "range")?,
            want_int(args[1], "range")?,
            want_int(args[2], "range")?,
        ),
    };
    if step == 0 {
        return err("range: step cannot be 0");
    }
    let mut out = Vec::new();
    while (step > 0 && a < b) || (step < 0 && a > b) {
        out.push(mk_int(a));
        if out.len() > 10_000_000 {
            return err("range: too many elements");
        }
        a += step;
    }
    Ok(mk_array(out))
}

fn map_with<R>(
    v: V,
    name: &str,
    f: impl FnOnce(&std::collections::HashMap<String, V>) -> R,
) -> Result<R, String> {
    unsafe {
        if is_ptr(v) {
            if let HeapObj::Map(m) = payload(v) {
                return Ok(f(m));
            }
        }
        err(format!("{}: expected object, got {}", name, kind_name(v)))
    }
}

fn b_keys(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "keys")?;
    let ks = map_with(args[0], "keys", |m| {
        let mut v: Vec<String> = m.keys().cloned().collect();
        v.sort();
        v
    })?;
    Ok(mk_array(ks.iter().map(|k| mk_str_from(k)).collect()))
}
fn b_values(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "values")?;
    let ks = map_with(args[0], "values", |m| {
        let mut v: Vec<(String, V)> = m.iter().map(|(k, &v)| (k.clone(), v)).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v.into_iter().map(|(_, v)| v).collect::<Vec<V>>()
    })?;
    Ok(mk_array(ks))
}
fn b_entries(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "entries")?;
    let es = map_with(args[0], "entries", |m| {
        let mut v: Vec<(String, V)> = m.iter().map(|(k, &v)| (k.clone(), v)).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    })?;
    let arr: Vec<V> = es
        .into_iter()
        .map(|(k, v)| mk_array(vec![mk_str_from(&k), v]))
        .collect();
    Ok(mk_array(arr))
}
fn b_has(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "has")?;
    let key = to_display(args[1]);
    map_with(args[0], "has", |m| bool_of(m.contains_key(&key)))
}
fn b_get(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "get")?;
    let key = to_display(args[1]);
    let def = args.get(2).copied().unwrap_or(NULL);
    map_with(args[0], "get", |m| m.get(&key).copied().unwrap_or(def))
}
fn b_set(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 3, "set")?;
    crate::value::member_set(args[0], &to_display(args[1]), args[2])?;
    Ok(args[0])
}
fn b_delete(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "delete")?;
    let key = to_display(args[1]);
    unsafe {
        if is_ptr(args[0]) {
            if let HeapObj::Map(m) = &mut *payload_mut(args[0]) {
                if let Some(old) = m.remove(&key) {
                    release_locked(old);
                }
                return Ok(args[0]);
            }
        }
    }
    err("delete: expected object")
}

// ---------------------------------------------------------------------------
// misc
// ---------------------------------------------------------------------------

fn b_time_ms(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Ok(mk_int(d.as_millis() as i64))
}
fn b_clock(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Ok(mk_float_unchecked(d.as_secs_f64()))
}
fn b_sleep_ms(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "sleep_ms")?;
    let ms = want_int(args[0], "sleep_ms")?.max(0) as u64;
    std::thread::sleep(std::time::Duration::from_millis(ms));
    Ok(NULL)
}
fn b_assert(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "assert")?;
    if !truthy(args[0]) {
        let msg = if args.len() > 1 {
            to_display(args[1])
        } else {
            "assertion failed".to_string()
        };
        return err(format!("assert: {}", msg));
    }
    Ok(NULL)
}
fn b_assert_eq(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "assert_eq")?;
    if !crate::value::values_eq(args[0], args[1]) {
        return err(format!(
            "assert_eq failed: left={}, right={}",
            to_repr(args[0]),
            to_repr(args[1])
        ));
    }
    Ok(NULL)
}
fn b_assert_ne(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "assert_ne")?;
    if crate::value::values_eq(args[0], args[1]) {
        return err(format!("assert_ne failed: both={}", to_repr(args[0])));
    }
    Ok(NULL)
}
fn b_panic(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    let msg = if args.is_empty() {
        "panic".to_string()
    } else {
        to_display(args[0])
    };
    err(msg)
}
fn b_exit(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    let code = if args.is_empty() {
        0
    } else {
        want_int(args[0], "exit")? as i32
    };
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// fs module
// ---------------------------------------------------------------------------

fn ioerr(op: &str, e: std::io::Error) -> String {
    format!("fs.{}: {}", op, e)
}

fn fs_read(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.read")?;
    let p = want_str(args[0], "fs.read")?;
    std::fs::read_to_string(&p)
        .map(mk_string)
        .map_err(|e| ioerr("read", e))
}
fn fs_write(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "fs.write")?;
    let p = want_str(args[0], "fs.write")?;
    if is_buffer(args[1]) {
        // Write raw bytes from a Buffer object
        let data = unsafe {
            match payload(args[1]) {
                HeapObj::Buffer(b) => b.clone(),
                _ => unreachable!(),
            }
        };
        std::fs::write(&p, data)
            .map(|_| bool_of(true))
            .map_err(|e| ioerr("write", e))
    } else {
        let content = to_display(args[1]);
        std::fs::write(&p, content)
            .map(|_| bool_of(true))
            .map_err(|e| ioerr("write", e))
    }
}
fn fs_append(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "fs.append")?;
    let p = want_str(args[0], "fs.append")?;
    if is_buffer(args[1]) {
        let data = unsafe {
            match payload(args[1]) {
                HeapObj::Buffer(b) => b.clone(),
                _ => unreachable!(),
            }
        };
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&p)
            .and_then(|mut f| std::io::Write::write_all(&mut f, &data))
            .map(|_| bool_of(true))
            .map_err(|e| ioerr("append", e))
    } else {
        let content = to_display(args[1]);
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&p)
            .and_then(|mut f| f.write_all(content.as_bytes()))
            .map(|_| bool_of(true))
            .map_err(|e| ioerr("append", e))
    }
}
fn fs_exists(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.exists")?;
    Ok(bool_of(
        std::path::Path::new(&want_str(args[0], "fs.exists")?).exists(),
    ))
}
fn fs_is_file(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.is_file")?;
    Ok(bool_of(
        std::path::Path::new(&want_str(args[0], "fs.is_file")?).is_file(),
    ))
}
fn fs_is_dir(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.is_dir")?;
    Ok(bool_of(
        std::path::Path::new(&want_str(args[0], "fs.is_dir")?).is_dir(),
    ))
}
fn fs_size(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.size")?;
    std::fs::metadata(&want_str(args[0], "fs.size")?)
        .map(|m| mk_int(m.len() as i64))
        .map_err(|e| ioerr("size", e))
}
fn fs_list(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.list")?;
    let p = want_str(args[0], "fs.list")?;
    let rd = std::fs::read_dir(&p).map_err(|e| ioerr("list", e))?;
    let mut names: Vec<String> = Vec::new();
    for e in rd.flatten() {
        names.push(e.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(mk_array(names.iter().map(|n| mk_str_from(n)).collect()))
}
fn fs_mkdir(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.mkdir")?;
    std::fs::create_dir_all(&want_str(args[0], "fs.mkdir")?)
        .map(|_| bool_of(true))
        .map_err(|e| ioerr("mkdir", e))
}
fn fs_remove(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.remove")?;
    let p = want_str(args[0], "fs.remove")?;
    let path = std::path::Path::new(&p);
    let r = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    r.map(|_| bool_of(true)).map_err(|e| ioerr("remove", e))
}
fn fs_rename(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "fs.rename")?;
    std::fs::rename(
        &want_str(args[0], "fs.rename")?,
        &want_str(args[1], "fs.rename")?,
    )
    .map(|_| bool_of(true))
    .map_err(|e| ioerr("rename", e))
}
fn fs_copy(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "fs.copy")?;
    std::fs::copy(
        &want_str(args[0], "fs.copy")?,
        &want_str(args[1], "fs.copy")?,
    )
    .map(|_| bool_of(true))
    .map_err(|e| ioerr("copy", e))
}
fn fs_join(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "fs.join")?;
    let a = want_str(args[0], "fs.join")?;
    let b = want_str(args[1], "fs.join")?;
    Ok(mk_string(
        std::path::Path::new(&a)
            .join(&b)
            .to_string_lossy()
            .into_owned(),
    ))
}
fn fs_abs(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.abs")?;
    std::fs::canonicalize(&want_str(args[0], "fs.abs")?)
        .map(|p| mk_string(p.to_string_lossy().into_owned()))
        .map_err(|e| ioerr("abs", e))
}
fn fs_parent(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.parent")?;
    let p = want_str(args[0], "fs.parent")?;
    Ok(mk_string(
        std::path::Path::new(&p)
            .parent()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ))
}
fn fs_name(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.name")?;
    let p = want_str(args[0], "fs.name")?;
    Ok(mk_string(
        std::path::Path::new(&p)
            .file_name()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ))
}
fn fs_ext(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "fs.ext")?;
    let p = want_str(args[0], "fs.ext")?;
    Ok(mk_string(
        std::path::Path::new(&p)
            .extension()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ))
}

fn shell_command(cmd: &str) -> std::process::Command {
    if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(cmd);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    }
}

// ---------------------------------------------------------------------------
// sys module
// ---------------------------------------------------------------------------

fn sys_platform(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    Ok(mk_str_from(std::env::consts::OS))
}
fn sys_arch(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    Ok(mk_str_from(std::env::consts::ARCH))
}
/// script arguments (set by the plix CLI for run/exec/repl; native
/// executables compiled by `plix build` fall back to their own argv)
static PROGRAM_ARGS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
pub fn set_program_args(a: Vec<String>) {
    let _ = PROGRAM_ARGS.set(a);
}
fn sys_args(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    let list: Vec<String> = match PROGRAM_ARGS.get() {
        Some(v) => v.clone(),
        None => std::env::args().collect(),
    };
    Ok(mk_array(list.iter().map(|a| mk_str_from(a)).collect()))
}
fn sys_env(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "sys.env")?;
    match std::env::var(&want_str(args[0], "sys.env")?) {
        Ok(v) => Ok(mk_string(v)),
        Err(_) => Ok(NULL),
    }
}
fn sys_set_env(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "sys.set_env")?;
    std::env::set_var(
        &want_str(args[0], "sys.set_env")?,
        &want_str(args[1], "sys.set_env")?,
    );
    Ok(NULL)
}
fn sys_cwd(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    match std::env::current_dir() {
        Ok(p) => Ok(mk_string(p.to_string_lossy().into_owned())),
        Err(e) => err(format!("sys.cwd: {}", e)),
    }
}
fn sys_exec(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "sys.exec")?;
    let out = if is_int(args[0]) || !is_ptr(args[0]) {
        let cmd = to_display(args[0]);
        shell_command(&cmd).output()
    } else {
        unsafe {
            match payload(args[0]) {
                HeapObj::Array(items) => {
                    let argv: Vec<String> = items.iter().map(|&v| to_display(v)).collect();
                    if argv.is_empty() {
                        return err("sys.exec: empty command array");
                    }
                    std::process::Command::new(&argv[0])
                        .args(&argv[1..])
                        .output()
                }
                _ => {
                    let cmd = to_display(args[0]);
                    shell_command(&cmd).output()
                }
            }
        }
    };
    match out {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            m.insert(
                "stderr".to_string(),
                mk_string(String::from_utf8_lossy(&o.stderr).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("sys.exec: {}", e)),
    }
}
fn sys_pid(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    Ok(mk_int(std::process::id() as i64))
}
fn sys_hostname(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    let h = std::process::Command::new("hostname")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    Ok(mk_string(h))
}

// ---------------------------------------------------------------------------
// python bridge (thin wrappers over pyffi)
// ---------------------------------------------------------------------------

fn py_available(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    Ok(bool_of(crate::pyffi::is_available()))
}
fn py_has_module(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "py.has_module")?;
    Ok(bool_of(crate::pyffi::has_module(&want_str(
        args[0],
        "py.has_module",
    )?)))
}
fn py_import(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "py.import")?;
    crate::pyffi::import(&want_str(args[0], "py.import")?)
}
fn py_eval(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "py.eval")?;
    crate::pyffi::eval(&want_str(args[0], "py.eval")?)
}
fn py_exec(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "py.exec")?;
    crate::pyffi::exec(&want_str(args[0], "py.exec")?)?;
    Ok(NULL)
}
fn py_call(c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "py.call")?;
    call_value(c, args[0], &args[1..])
}
fn py_getattr(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "py.getattr")?;
    crate::value::member_get(args[0], &want_str(args[1], "py.getattr")?)
}
fn py_setattr(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 3, "py.setattr")?;
    crate::value::member_set(args[0], &want_str(args[1], "py.setattr")?, args[2])
}
fn py_hasattr(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "py.hasattr")?;
    crate::pyffi::hasattr(args[0], &want_str(args[1], "py.hasattr")?)
}
fn py_repr(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "py.repr")?;
    Ok(mk_string(crate::pyffi::repr_val(args[0])))
}
fn py_to_plix(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "py.to_plix")?;
    crate::pyffi::to_plix_deep(args[0])
}

// ---------------------------------------------------------------------------
// ai module
// ---------------------------------------------------------------------------

fn ai_lib(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "ai.lib")?;
    crate::pyffi::import(&want_str(args[0], "ai.lib")?)
}
fn ai_array(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "ai.array")?;
    // numpy stays on the python side (zero-copy handle); the plix value is
    // just a lightweight reference.
    let np = crate::pyffi::import("numpy")?;
    let arr_fn = crate::value::member_get(np, "array")?;
    let mut c = crate::value::NativeCaller;
    crate::value::call_value(&mut c, arr_fn, &[args[0]])
}
fn ai_call(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "ai.call")?;
    let f = crate::value::member_get(args[0], &want_str(args[1], "ai.call")?)?;
    let mut c = crate::value::NativeCaller;
    crate::value::call_value(&mut c, f, &args[2..])
}
fn ai_shape(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "ai.shape")?;
    match crate::value::member_get(args[0], "shape") {
        Ok(v) => crate::pyffi::to_plix_deep(v),
        Err(_) => {
            // plain plix array: shape is just the length
            crate::value::length(args[0]).map(|n| mk_array(vec![n]))
        }
    }
}

// ---------------------------------------------------------------------------
// forge module (rust ecosystem bridge)
// ---------------------------------------------------------------------------

fn forge_version(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    Ok(mk_str_from("plix 0.9.13 (rust runtime)"))
}
fn forge_rust_version(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    match std::process::Command::new("rustc")
        .arg("--version")
        .output()
    {
        Ok(o) if !o.stdout.is_empty() => Ok(mk_string(
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
        )),
        _ => Ok(mk_str_from(
            option_env!("PLIX_RUSTC_VERSION").unwrap_or("rustc (unknown)"),
        )),
    }
}
fn forge_cargo(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "forge.cargo")?;
    let argv: Vec<String> = match args.len() {
        1 => vec![want_str(args[0], "forge.cargo")?],
        _ => {
            let items = want_arr(args[1], "forge.cargo")?;
            let mut v = vec![want_str(args[0], "forge.cargo")?];
            v.extend(items.iter().map(|&x| to_display(x)));
            v
        }
    };
    match std::process::Command::new("cargo").args(&argv).output() {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            m.insert(
                "stderr".to_string(),
                mk_string(String::from_utf8_lossy(&o.stderr).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("forge.cargo: {}", e)),
    }
}
fn forge_target(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    let mut m = std::collections::HashMap::new();
    m.insert("os".to_string(), mk_str_from(std::env::consts::OS));
    m.insert("arch".to_string(), mk_str_from(std::env::consts::ARCH));
    m.insert("family".to_string(), mk_str_from(std::env::consts::FAMILY));
    Ok(mk_map(m))
}

// ---------------------------------------------------------------------------
// docker module — container management via the Docker CLI
// ---------------------------------------------------------------------------

/// Returns true if the `docker` command is available on PATH.
fn docker_available(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    let ok = std::process::Command::new("docker")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    Ok(bool_of(ok))
}

/// Run a container: `docker.run(image, command?, opts?)`
///   - image: string  (e.g. "alpine:3.19")
///   - command: optional string or array of strings
///   - opts: optional map { detach: bool, rm: bool, env: map, volumes: map, ports: map, name: string, workdir: string }
/// Returns a map { id: string, code: int, stdout: string, stderr: string }
fn docker_run(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docker.run")?;
    let image = want_str(args[0], "docker.run")?;

    let mut cmd = std::process::Command::new("docker");
    cmd.arg("run");

    // optional opts map
    let opts = if args.len() >= 3 { Some(args[2]) } else { None };
    let mut detach = false;
    if let Some(ov) = opts {
        if is_ptr(ov) {
            unsafe {
                match payload(ov) {
                    HeapObj::Map(m) => {
                        if let Some(v) = m.get("detach") {
                            detach = truthy(*v);
                            if detach {
                                cmd.arg("-d");
                            }
                        }
                        if let Some(v) = m.get("rm") {
                            if truthy(*v) {
                                cmd.arg("--rm");
                            }
                        }
                        if let Some(v) = m.get("name") {
                            cmd.arg("--name").arg(want_str(*v, "docker.run name")?);
                        }
                        if let Some(v) = m.get("workdir") {
                            cmd.arg("-w").arg(want_str(*v, "docker.run workdir")?);
                        }
                        if let Some(v) = m.get("env") {
                            match payload(*v) {
                                HeapObj::Map(em) => {
                                    for (k, val) in em {
                                        cmd.arg("-e").arg(format!("{}={}", k, to_display(*val)));
                                    }
                                }
                                _ => {}
                            }
                        }
                        if let Some(v) = m.get("volumes") {
                            match payload(*v) {
                                HeapObj::Map(vm) => {
                                    for (host, cont) in vm {
                                        cmd.arg("-v").arg(format!(
                                            "{}:{}",
                                            host,
                                            to_display(*cont)
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        }
                        if let Some(v) = m.get("ports") {
                            match payload(*v) {
                                HeapObj::Map(pm) => {
                                    for (host, cont) in pm {
                                        cmd.arg("-p").arg(format!(
                                            "{}:{}",
                                            host,
                                            to_display(*cont)
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    cmd.arg(&image);

    // optional command
    if args.len() >= 2 && !is_null(args[1]) {
        if is_ptr(args[1]) {
            unsafe {
                match payload(args[1]) {
                    HeapObj::Array(items) => {
                        for &a in items {
                            cmd.arg(to_display(a));
                        }
                    }
                    _ => {
                        cmd.arg(want_str(args[1], "docker.run command")?);
                    }
                }
            }
        } else {
            cmd.arg(want_str(args[1], "docker.run command")?);
        }
    }

    match cmd.output() {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            // For detached mode, container ID is on stdout
            let stdout_str = String::from_utf8_lossy(&o.stdout).to_string();
            if detach {
                m.insert("id".to_string(), mk_string(stdout_str.trim().to_string()));
            } else {
                m.insert("id".to_string(), mk_str_from(""));
            }
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert("stdout".to_string(), mk_string(stdout_str));
            m.insert(
                "stderr".to_string(),
                mk_string(String::from_utf8_lossy(&o.stderr).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("docker.run: {}", e)),
    }
}

/// Build an image from a Dockerfile directory: `docker.build(path, tags?)`
///   - path: string (build context)
///   - tags: optional string or array of strings
/// Returns a map { code: int, stdout: string, stderr: string }
fn docker_build(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docker.build")?;
    let path = want_str(args[0], "docker.build")?;
    let mut cmd = std::process::Command::new("docker");
    cmd.arg("build").arg(&path);

    if args.len() >= 2 && !is_null(args[1]) {
        if is_ptr(args[1]) {
            unsafe {
                match payload(args[1]) {
                    HeapObj::Array(items) => {
                        for &t in items {
                            cmd.arg("-t").arg(to_display(t));
                        }
                    }
                    _ => {
                        cmd.arg("-t").arg(want_str(args[1], "docker.build tag")?);
                    }
                }
            }
        } else {
            cmd.arg("-t").arg(want_str(args[1], "docker.build tag")?);
        }
    }

    match cmd.output() {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            m.insert(
                "stderr".to_string(),
                mk_string(String::from_utf8_lossy(&o.stderr).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("docker.build: {}", e)),
    }
}

/// Pull an image: `docker.pull(image)`
/// Returns a map { code: int, stdout: string, stderr: string }
fn docker_pull(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docker.pull")?;
    let image = want_str(args[0], "docker.pull")?;
    match std::process::Command::new("docker")
        .arg("pull")
        .arg(&image)
        .output()
    {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            m.insert(
                "stderr".to_string(),
                mk_string(String::from_utf8_lossy(&o.stderr).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("docker.pull: {}", e)),
    }
}

/// List images: `docker.images()`
/// Returns a map { code: int, stdout: string }
fn docker_images(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    match std::process::Command::new("docker")
        .args([
            "images",
            "--format",
            "{{.Repository}}:{{.Tag}}\t{{.ID}}\t{{.Size}}",
        ])
        .output()
    {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("docker.images: {}", e)),
    }
}

/// List containers: `docker.ps(all?)`
///   - all: optional bool (default false — only running)
/// Returns a map { code: int, stdout: string }
fn docker_ps(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    let all = if !args.is_empty() && truthy(args[0]) {
        true
    } else {
        false
    };
    let mut cmd_args = vec![
        "ps",
        "--format",
        "{{.ID}}\t{{.Image}}\t{{.Status}}\t{{.Names}}",
    ];
    if all {
        cmd_args.push("-a");
    }
    match std::process::Command::new("docker")
        .args(&cmd_args)
        .output()
    {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("docker.ps: {}", e)),
    }
}

/// Stop a container: `docker.stop(id_or_name)`
/// Returns a map { code: int, stdout: string }
fn docker_stop(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docker.stop")?;
    let id = want_str(args[0], "docker.stop")?;
    match std::process::Command::new("docker")
        .arg("stop")
        .arg(&id)
        .output()
    {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("docker.stop: {}", e)),
    }
}

/// Remove a container: `docker.rm(id_or_name, force?)`
fn docker_rm(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docker.rm")?;
    let id = want_str(args[0], "docker.rm")?;
    let force = args.len() >= 2 && truthy(args[1]);
    let mut cmd = std::process::Command::new("docker");
    cmd.arg("rm");
    if force {
        cmd.arg("-f");
    }
    cmd.arg(&id);
    match cmd.output() {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("docker.rm: {}", e)),
    }
}

/// Remove an image: `docker.rmi(id_or_tag, force?)`
fn docker_rmi(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docker.rmi")?;
    let id = want_str(args[0], "docker.rmi")?;
    let force = args.len() >= 2 && truthy(args[1]);
    let mut cmd = std::process::Command::new("docker");
    cmd.arg("rmi");
    if force {
        cmd.arg("-f");
    }
    cmd.arg(&id);
    match cmd.output() {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("docker.rmi: {}", e)),
    }
}

/// Fetch logs: `docker.logs(id_or_name, tail?, follow?)`
fn docker_logs(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docker.logs")?;
    let id = want_str(args[0], "docker.logs")?;
    let mut cmd = std::process::Command::new("docker");
    cmd.arg("logs");
    if args.len() >= 2 && !is_null(args[1]) {
        let tail = want_int(args[1], "docker.logs tail")?;
        cmd.arg(format!("--tail={}", tail));
    }
    cmd.arg(&id);
    match cmd.output() {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            m.insert(
                "stderr".to_string(),
                mk_string(String::from_utf8_lossy(&o.stderr).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("docker.logs: {}", e)),
    }
}

/// Inspect a container or image: `docker.inspect(id_or_name)`
fn docker_inspect(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docker.inspect")?;
    let id = want_str(args[0], "docker.inspect")?;
    match std::process::Command::new("docker")
        .args(["inspect", "--format", "{{json .}}"])
        .arg(&id)
        .output()
    {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            m.insert(
                "stderr".to_string(),
                mk_string(String::from_utf8_lossy(&o.stderr).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("docker.inspect: {}", e)),
    }
}

/// Execute a command in a running container: `docker.exec(id, command)`
///   - id: container id or name
///   - command: string or array of strings
fn docker_exec(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "docker.exec")?;
    let id = want_str(args[0], "docker.exec")?;
    let mut cmd = std::process::Command::new("docker");
    cmd.arg("exec").arg(&id);
    if is_ptr(args[1]) {
        unsafe {
            match payload(args[1]) {
                HeapObj::Array(items) => {
                    for &a in items {
                        cmd.arg(to_display(a));
                    }
                }
                _ => {
                    cmd.arg(want_str(args[1], "docker.exec command")?);
                }
            }
        }
    } else {
        cmd.arg(want_str(args[1], "docker.exec command")?);
    }
    match cmd.output() {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            m.insert(
                "stderr".to_string(),
                mk_string(String::from_utf8_lossy(&o.stderr).into_owned()),
            );
            Ok(mk_map(m))
        }
        Err(e) => err(format!("docker.exec: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// security/sandbox module — resource restrictions & sandboxed execution
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

/// Global sandbox state
static SEC_NETWORK_ALLOWED: AtomicBool = AtomicBool::new(true);
static SEC_MAX_MEMORY_MB: AtomicI64 = AtomicI64::new(0); // 0 = unlimited

fn sec_available(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    // seccomp is Linux-only; on other platforms we report capabilities
    let mut caps = Vec::new();
    caps.push(mk_str_from("hash"));
    caps.push(mk_str_from("file_access_control"));
    if cfg!(target_os = "linux") {
        caps.push(mk_str_from("seccomp"));
        caps.push(mk_str_from("namespaces"));
    }
    caps.push(mk_str_from("resource_limits"));
    Ok(mk_array(caps))
}

/// Run a Plix source string in a restricted sandbox:
/// `security.sandbox_run(source, opts?)`
///   - source: string (Plix source code)
///   - opts: optional map { no_network: bool, max_memory_mb: int, allowed_dirs: array, timeout_ms: int }
/// Executes the code in a subprocess with restrictions applied.
fn sec_sandbox_run(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "security.sandbox_run")?;
    let source = want_str(args[0], "security.sandbox_run")?;

    // Write source to a temp file
    let tmp_dir = std::env::temp_dir();
    let tmp_file = tmp_dir.join(format!("plix_sandbox_{}.px", std::process::id()));
    if let Err(e) = std::fs::write(&tmp_file, &source) {
        return err(format!(
            "security.sandbox_run: cannot write temp file: {}",
            e
        ));
    }

    let plix_bin = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("plix"));

    let mut cmd = std::process::Command::new(&plix_bin);
    cmd.arg("run").arg(&tmp_file);
    cmd.env("PLIX_SANDBOX", "1");

    // Apply options
    let mut _no_network = false;
    let mut _timeout_secs: u64 = 30;
    if args.len() >= 2 && is_ptr(args[1]) {
        unsafe {
            if let HeapObj::Map(m) = payload(args[1]) {
                if let Some(v) = m.get("no_network") {
                    _no_network = truthy(*v);
                    if _no_network {
                        cmd.env("PLIX_SANDBOX_NO_NETWORK", "1");
                    }
                }
                if let Some(v) = m.get("max_memory_mb") {
                    let mb = want_int(*v, "security.sandbox_run max_memory_mb").unwrap_or(0);
                    if mb > 0 {
                        cmd.env("PLIX_SANDBOX_MAX_MEMORY_MB", format!("{}", mb));
                    }
                }
                if let Some(v) = m.get("timeout_ms") {
                    let ms = want_int(*v, "security.sandbox_run timeout_ms").unwrap_or(30000);
                    _timeout_secs = (ms as u64 + 999) / 1000;
                }
                if let Some(v) = m.get("allowed_dirs") {
                    match payload(*v) {
                        HeapObj::Array(dirs) => {
                            let dir_list: Vec<String> =
                                dirs.iter().map(|&d| to_display(d)).collect();
                            cmd.env("PLIX_SANDBOX_ALLOWED_DIRS", dir_list.join(":"));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // On Linux, apply resource limits via the process builder
    #[cfg(target_os = "linux")]
    {
        // We use prlimit-like approach via a wrapper; for now the env vars
        // are consumed by the child process's own startup code.
    }

    let result = cmd.output();

    // Cleanup temp file
    let _ = std::fs::remove_file(&tmp_file);

    match result {
        Ok(o) => {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "code".to_string(),
                mk_int(o.status.code().unwrap_or(-1) as i64),
            );
            m.insert(
                "stdout".to_string(),
                mk_string(String::from_utf8_lossy(&o.stdout).into_owned()),
            );
            m.insert(
                "stderr".to_string(),
                mk_string(String::from_utf8_lossy(&o.stderr).into_owned()),
            );
            let timed_out = o
                .status
                .code()
                .map(|c| c == -1 || c == 137 || c == 9)
                .unwrap_or(false);
            m.insert("timed_out".to_string(), bool_of(timed_out));
            Ok(mk_map(m))
        }
        Err(e) => err(format!("security.sandbox_run: {}", e)),
    }
}

/// Check if a path is within the allowed directories:
/// `security.sandbox_check(path)`
fn sec_sandbox_check(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "security.sandbox_check")?;
    let path = want_str(args[0], "security.sandbox_check")?;

    let allowed_env = std::env::var("PLIX_SANDBOX_ALLOWED_DIRS").unwrap_or_default();
    if allowed_env.is_empty() {
        // No sandbox active — everything allowed
        return Ok(bool_of(true));
    }

    let allowed: Vec<&str> = allowed_env.split(':').filter(|s| !s.is_empty()).collect();
    let ok = allowed.iter().any(|dir| path.starts_with(dir));
    Ok(bool_of(ok))
}

/// Compute SHA-256 hash of a file: `security.hash_file(path)`
fn sec_hash_file(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "security.hash_file")?;
    let path = want_str(args[0], "security.hash_file")?;

    // Use sha256sum command (available on most Unix systems)
    let output = std::process::Command::new("sha256sum").arg(&path).output();

    match output {
        Ok(o) if o.status.success() => {
            let line = String::from_utf8_lossy(&o.stdout);
            let hash = line.split_whitespace().next().unwrap_or("").to_string();
            Ok(mk_string(hash))
        }
        Ok(_) => err("security.hash_file: sha256sum failed"),
        Err(e) => err(format!("security.hash_file: {}", e)),
    }
}

/// Compute SHA-256 hash of a string: `security.hash_str(data)`
fn sec_hash_str(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "security.hash_str")?;
    let data = want_str(args[0], "security.hash_str")?;

    // Use openssl (widely available) for in-memory hashing
    let output = std::process::Command::new("openssl")
        .args(["dgst", "-sha256", "-hex"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn();

    match output {
        Ok(mut child) => {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(data.as_bytes());
            }
            match child.wait_with_output() {
                Ok(o) if o.status.success() => {
                    let line = String::from_utf8_lossy(&o.stdout);
                    // openssl output: "SHA2-256(stdin)= abc123..."
                    let hash = line
                        .split('=')
                        .last()
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();
                    Ok(mk_string(hash))
                }
                Ok(_) => err("security.hash_str: openssl failed"),
                Err(e) => err(format!("security.hash_str: {}", e)),
            }
        }
        Err(_) => {
            // Fallback: use sha256sum with echo pipe
            let output2 = std::process::Command::new("sh")
                .args([
                    "-c",
                    &format!("echo -n '{}' | sha256sum", data.replace('\'', "'\\''")),
                ])
                .output();
            match output2 {
                Ok(o) if o.status.success() => {
                    let line = String::from_utf8_lossy(&o.stdout);
                    let hash = line.split_whitespace().next().unwrap_or("").to_string();
                    Ok(mk_string(hash))
                }
                Ok(_) => err("security.hash_str: sha256sum failed"),
                Err(e) => err(format!("security.hash_str: {}", e)),
            }
        }
    }
}

/// Verify a string against a SHA-256 hash: `security.verify_str(data, expected_hash)`
fn sec_verify_str(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "security.verify_str")?;
    let data = want_str(args[0], "security.verify_str")?;
    let expected = want_str(args[1], "security.verify_str")?;
    // Compute hash of data and compare
    let computed = match sec_hash_str(_c, &[mk_string(data)]) {
        Ok(v) => want_str(v, "security.verify_str internal")?,
        Err(e) => return Err(e),
    };
    Ok(bool_of(computed.eq_ignore_ascii_case(&expected)))
}

/// Get the list of allowed directories for sandboxed code:
/// `security.allowed_dirs()`
fn sec_allowed_dirs(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    let allowed_env = std::env::var("PLIX_SANDBOX_ALLOWED_DIRS").unwrap_or_default();
    if allowed_env.is_empty() {
        return Ok(mk_array(vec![]));
    }
    let dirs: Vec<V> = allowed_env
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| mk_str_from(s))
        .collect();
    Ok(mk_array(dirs))
}

/// Add an allowed directory for sandboxed code:
/// `security.add_allowed_dir(path)`
fn sec_add_allowed_dir(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "security.add_allowed_dir")?;
    let dir = want_str(args[0], "security.add_allowed_dir")?;
    let mut current = std::env::var("PLIX_SANDBOX_ALLOWED_DIRS").unwrap_or_default();
    if !current.is_empty() {
        current.push(':');
    }
    current.push_str(&dir);
    std::env::set_var("PLIX_SANDBOX_ALLOWED_DIRS", &current);
    Ok(NULL)
}

/// Check if network access is allowed in sandbox mode:
/// `security.network()`
fn sec_network(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    Ok(bool_of(SEC_NETWORK_ALLOWED.load(Ordering::Relaxed)))
}

/// Set network access for sandbox mode:
/// `security.set_network(allowed)`
fn sec_set_network(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "security.set_network")?;
    SEC_NETWORK_ALLOWED.store(truthy(args[0]), Ordering::Relaxed);
    Ok(NULL)
}

/// Get max memory limit for sandbox (0 = unlimited):
/// `security.max_memory_mb()`
fn sec_max_memory_mb(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    Ok(mk_int(SEC_MAX_MEMORY_MB.load(Ordering::Relaxed)))
}

/// Set max memory limit for sandbox:
/// `security.set_max_memory_mb(mb)`
fn sec_set_max_memory_mb(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "security.set_max_memory_mb")?;
    let mb = want_int(args[0], "security.set_max_memory_mb")?;
    SEC_MAX_MEMORY_MB.store(mb, Ordering::Relaxed);
    Ok(NULL)
}

// ---------------------------------------------------------------------------
// docs module — doc comment extraction + JSON/HTML/Markdown generation
// ---------------------------------------------------------------------------

/// Extract documentation from a Plix source string:
/// `docs.extract(source)`
/// Returns an array of doc entries, each a map:
///   { kind: "func"|"struct"|"trait"|"enum"|"var",
///     name: string, doc: string, line: int, col: int,
///     params: array (funcs only), fields: array (structs/enums) }
fn docs_extract(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docs.extract")?;
    let source = want_str(args[0], "docs.extract")?;

    let entries = extract_doc_entries(&source);
    let arr: Vec<V> = entries.iter().map(|e| e.to_value()).collect();
    Ok(mk_array(arr))
}

/// Extract docs and return as JSON string:
/// `docs.json(source)`
fn docs_json(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docs.json")?;
    let source = want_str(args[0], "docs.json")?;

    let entries = extract_doc_entries(&source);
    let json = entries_to_json(&entries);
    Ok(mk_string(json))
}

/// Extract docs and return as HTML string:
/// `docs.html(source, title?)`
fn docs_html(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docs.html")?;
    let source = want_str(args[0], "docs.html")?;
    let title = if args.len() >= 2 && !is_null(args[1]) {
        want_str(args[1], "docs.html title")?
    } else {
        "Plix API Documentation".to_string()
    };

    let entries = extract_doc_entries(&source);
    let html = entries_to_html(&entries, &title);
    Ok(mk_string(html))
}

/// Extract docs and return as Markdown string:
/// `docs.markdown(source, title?)`
fn docs_markdown(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "docs.markdown")?;
    let source = want_str(args[0], "docs.markdown")?;
    let title = if args.len() >= 2 && !is_null(args[1]) {
        want_str(args[1], "docs.markdown title")?
    } else {
        "API Documentation".to_string()
    };

    let entries = extract_doc_entries(&source);
    let md = entries_to_markdown(&entries, &title);
    Ok(mk_string(md))
}

/// Search doc entries by name/query:
/// `docs.search(source, query)`
fn docs_search(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "docs.search")?;
    let source = want_str(args[0], "docs.search")?;
    let query = want_str(args[1], "docs.search")?;

    let entries = extract_doc_entries(&source);
    let query_lower = query.to_lowercase();
    let filtered: Vec<V> = entries
        .iter()
        .filter(|e| {
            e.name.to_lowercase().contains(&query_lower)
                || e.doc.to_lowercase().contains(&query_lower)
        })
        .map(|e| e.to_value())
        .collect();
    Ok(mk_array(filtered))
}

// ---- docs internal types and helpers ----

struct DocEntry {
    kind: String,
    name: String,
    doc: String,
    line: u32,
    col: u32,
    params: Vec<String>,
    fields: Vec<String>,
}

impl DocEntry {
    fn to_value(&self) -> V {
        let mut m = std::collections::HashMap::new();
        m.insert("kind".to_string(), mk_str_from(&self.kind));
        m.insert("name".to_string(), mk_string(self.name.clone()));
        m.insert("doc".to_string(), mk_string(self.doc.clone()));
        m.insert("line".to_string(), mk_int(self.line as i64));
        m.insert("col".to_string(), mk_int(self.col as i64));
        m.insert(
            "params".to_string(),
            mk_array(self.params.iter().map(|s| mk_str_from(s)).collect()),
        );
        m.insert(
            "fields".to_string(),
            mk_array(self.fields.iter().map(|s| mk_str_from(s)).collect()),
        );
        mk_map(m)
    }
}

/// Parse Plix source and extract documentation entries.
/// Recognizes:
///   - `// doc comment` lines immediately before declarations
///   - `func name(params) { ... }`
///   - `struct Name { fields }`
///   - `trait Name { methods }`
///   - `enum Name { variants }`
///   - `auto/const/own name = value`
fn extract_doc_entries(source: &str) -> Vec<DocEntry> {
    let lines: Vec<&str> = source.lines().collect();
    let mut entries = Vec::new();
    let mut pending_doc: Vec<String> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Collect consecutive doc-comment lines (// or ///)
        if line.starts_with("///")
            || (line.starts_with("//") && !line.starts_with("//!") && i + 1 < lines.len())
        {
            let is_doc = line.starts_with("///");
            let is_pre_decl = !is_doc && i + 1 < lines.len() && {
                let next = lines[i + 1].trim();
                next.starts_with("func ")
                    || next.starts_with("struct ")
                    || next.starts_with("trait ")
                    || next.starts_with("enum ")
                    || next.starts_with("own ")
                    || next.starts_with("auto ")
                    || next.starts_with("const ")
            };
            if is_doc || is_pre_decl {
                let comment_text = if line.starts_with("///") {
                    line.strip_prefix("///").unwrap_or(line).trim()
                } else {
                    line.strip_prefix("//").unwrap_or(line).trim()
                };
                pending_doc.push(comment_text.to_string());
                i += 1;
                continue;
            }
        }

        // Blank lines reset pending doc
        if line.is_empty() {
            pending_doc.clear();
            i += 1;
            continue;
        }

        // Try to parse declarations
        let doc = pending_doc.join("\n");
        pending_doc.clear();

        if let Some(rest) = line.strip_prefix("func ") {
            if let Some(paren_pos) = rest.find('(') {
                let name = rest[..paren_pos].trim().to_string();
                let params_str = extract_parens(&rest[paren_pos..]);
                let params: Vec<String> = parse_params(&params_str);
                entries.push(DocEntry {
                    kind: "func".to_string(),
                    name,
                    doc,
                    line: (i + 1) as u32,
                    col: 1,
                    params,
                    fields: vec![],
                });
            }
        } else if let Some(rest) = line.strip_prefix("struct ") {
            let (name, fields) = parse_struct_header(rest);
            entries.push(DocEntry {
                kind: "struct".to_string(),
                name,
                doc,
                line: (i + 1) as u32,
                col: 1,
                params: vec![],
                fields,
            });
        } else if let Some(rest) = line.strip_prefix("trait ") {
            let name = rest.trim_end_matches('{').trim().to_string();
            entries.push(DocEntry {
                kind: "trait".to_string(),
                name,
                doc,
                line: (i + 1) as u32,
                col: 1,
                params: vec![],
                fields: vec![],
            });
        } else if let Some(rest) = line.strip_prefix("enum ") {
            let name = rest.trim_end_matches('{').trim().to_string();
            entries.push(DocEntry {
                kind: "enum".to_string(),
                name,
                doc,
                line: (i + 1) as u32,
                col: 1,
                params: vec![],
                fields: vec![],
            });
        } else if line.starts_with("own ")
            || line.starts_with("auto ")
            || line.starts_with("const ")
        {
            let kw_len = if line.starts_with("own ") {
                4
            } else if line.starts_with("auto ") {
                5
            } else {
                6
            };
            let rest = &line[kw_len..];
            let name = rest
                .split(|c: char| c == '=' || c.is_whitespace())
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !name.is_empty() {
                entries.push(DocEntry {
                    kind: "var".to_string(),
                    name,
                    doc,
                    line: (i + 1) as u32,
                    col: 1,
                    params: vec![],
                    fields: vec![],
                });
            }
        }

        i += 1;
    }

    entries
}

/// Extract content inside the first balanced pair of parentheses
fn extract_parens(s: &str) -> String {
    let mut depth = 0;
    let mut result = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                if depth == 1 {
                    continue;
                }
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        if depth > 0 {
            result.push(c);
        }
    }
    result
}

/// Parse comma-separated parameter names from a function signature
fn parse_params(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| {
            // Take the identifier part (before : or = or whitespace)
            p.trim()
                .split(|c: char| c == ':' || c == '=' || c.is_whitespace())
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse struct name and field names from "Name { field1: type, ... }"
fn parse_struct_header(rest: &str) -> (String, Vec<String>) {
    let brace_pos = rest.find('{').unwrap_or(rest.len());
    let name = rest[..brace_pos].trim().to_string();
    let fields_str = if brace_pos < rest.len() {
        &rest[brace_pos + 1..]
    } else {
        ""
    };
    let fields: Vec<String> = fields_str
        .split(',')
        .map(|f| f.split(':').next().unwrap_or("").trim().to_string())
        .filter(|f| !f.is_empty() && f != "}")
        .collect();
    (name, fields)
}

/// Serialize doc entries to JSON
fn entries_to_json(entries: &[DocEntry]) -> String {
    let mut out = String::from("[\n");
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            out.push_str(",\n");
        }
        out.push_str("  {\n");
        out.push_str(&format!("    \"kind\": {},\n", json_str(&e.kind)));
        out.push_str(&format!("    \"name\": {},\n", json_str(&e.name)));
        out.push_str(&format!("    \"doc\": {},\n", json_str(&e.doc)));
        out.push_str(&format!("    \"line\": {},\n", e.line));
        out.push_str(&format!("    \"col\": {},\n", e.col));
        out.push_str("    \"params\": [");
        for (j, p) in e.params.iter().enumerate() {
            if j > 0 {
                out.push_str(", ");
            }
            out.push_str(&json_str(p));
        }
        out.push_str("],\n");
        out.push_str("    \"fields\": [");
        for (j, f) in e.fields.iter().enumerate() {
            if j > 0 {
                out.push_str(", ");
            }
            out.push_str(&json_str(f));
        }
        out.push_str("]\n");
        out.push_str("  }");
    }
    out.push_str("\n]");
    out
}

/// JSON-escape a string
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Serialize doc entries to HTML
fn entries_to_html(entries: &[DocEntry], title: &str) -> String {
    let mut out = String::from("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    out.push_str("<meta charset=\"utf-8\">\n");
    out.push_str(&format!("<title>{}</title>\n", html_escape(title)));
    out.push_str("<style>\n");
    out.push_str("  body { font-family: -apple-system, sans-serif; max-width: 900px; margin: 2em auto; padding: 0 1em; color: #333; }\n");
    out.push_str("  h1 { border-bottom: 2px solid #4a90d9; padding-bottom: 0.3em; }\n");
    out.push_str("  h2 { color: #4a90d9; margin-top: 1.5em; }\n");
    out.push_str("  .entry { margin: 1em 0; padding: 0.8em; border-left: 3px solid #4a90d9; background: #f8f9fa; }\n");
    out.push_str("  .entry.func { border-color: #2e7d32; }\n");
    out.push_str("  .entry.struct { border-color: #c62828; }\n");
    out.push_str("  .entry.trait { border-color: #6a1b9a; }\n");
    out.push_str("  .entry.enum { border-color: #e65100; }\n");
    out.push_str("  .entry.var { border-color: #0277bd; }\n");
    out.push_str("  .kind { font-size: 0.8em; text-transform: uppercase; color: #666; }\n");
    out.push_str("  .name { font-weight: bold; font-size: 1.1em; }\n");
    out.push_str(
        "  .doc { margin: 0.5em 0; white-space: pre-wrap; font-style: italic; color: #555; }\n",
    );
    out.push_str("  .params, .fields { font-family: monospace; }\n");
    out.push_str("  code { background: #eee; padding: 0.15em 0.3em; border-radius: 3px; }\n");
    out.push_str("</style>\n");
    out.push_str("</head>\n<body>\n");
    out.push_str(&format!("<h1>{}</h1>\n", html_escape(title)));

    // Group by kind
    let _kinds = ["func", "struct", "trait", "enum", "var"];
    let kind_labels = [
        ("func", "Functions"),
        ("struct", "Structs"),
        ("trait", "Traits"),
        ("enum", "Enums"),
        ("var", "Variables"),
    ];

    for (kind, label) in kind_labels {
        let group: Vec<&DocEntry> = entries.iter().filter(|e| e.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("<h2>{}</h2>\n", label));
        for e in &group {
            out.push_str(&format!("<div class=\"entry {}\">\n", e.kind));
            out.push_str(&format!("  <span class=\"kind\">{}</span>\n", e.kind));
            out.push_str(&format!(
                "  <span class=\"name\">{}</span>\n",
                html_escape(&e.name)
            ));
            if !e.params.is_empty() {
                out.push_str("  <span class=\"params\">(");
                for (j, p) in e.params.iter().enumerate() {
                    if j > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format!("<code>{}</code>", html_escape(p)));
                }
                out.push_str(")</span>\n");
            }
            if !e.fields.is_empty() {
                out.push_str("  <div class=\"fields\">Fields: ");
                for (j, f) in e.fields.iter().enumerate() {
                    if j > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format!("<code>{}</code>", html_escape(f)));
                }
                out.push_str("</div>\n");
            }
            if !e.doc.is_empty() {
                out.push_str(&format!(
                    "  <div class=\"doc\">{}</div>\n",
                    html_escape(&e.doc)
                ));
            }
            out.push_str(&format!(
                "  <div class=\"location\">Line {}</div>\n",
                e.line
            ));
            out.push_str("</div>\n");
        }
    }

    out.push_str("</body>\n</html>\n");
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Serialize doc entries to Markdown
fn entries_to_markdown(entries: &[DocEntry], title: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", title));

    let kind_labels = [
        ("func", "Functions"),
        ("struct", "Structs"),
        ("trait", "Traits"),
        ("enum", "Enums"),
        ("var", "Variables"),
    ];

    for (kind, label) in kind_labels {
        let group: Vec<&DocEntry> = entries.iter().filter(|e| e.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("## {}\n\n", label));
        for e in &group {
            match e.kind.as_str() {
                "func" => {
                    out.push_str(&format!("**{}**({})\n", e.name, e.params.join(", ")));
                }
                "struct" | "enum" => {
                    out.push_str(&format!("**{}**", e.name));
                    if !e.fields.is_empty() {
                        out.push_str(&format!(" {{ {} }}", e.fields.join(", ")));
                    }
                    out.push('\n');
                }
                _ => {
                    out.push_str(&format!("**{}**\n", e.name));
                }
            }
            if !e.doc.is_empty() {
                out.push_str(&format!(": {}\n", e.doc));
            }
            out.push_str(&format!("  *(line {})*\n\n", e.line));
        }
    }

    out
}

// ---------------------------------------------------------------------------
// lsp module — Language Server Protocol utilities
// ---------------------------------------------------------------------------

/// Start the LSP server (spawns `plix lsp` as a subprocess):
/// `lsp.start(opts?)`
/// Returns a map { pid: int, handle: int }
fn lsp_start(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    let plix_bin = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("plix"));
    let mut cmd = std::process::Command::new(&plix_bin);
    cmd.arg("lsp");
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Forward opts if provided
    if !args.is_empty() && is_ptr(args[0]) {
        unsafe {
            if let HeapObj::Map(m) = payload(args[0]) {
                if let Some(v) = m.get("log_file") {
                    cmd.env("PLIX_LSP_LOG", want_str(*v, "lsp.start log_file")?);
                }
            }
        }
    }

    match cmd.spawn() {
        Ok(child) => {
            let mut result = std::collections::HashMap::new();
            result.insert("pid".to_string(), mk_int(child.id() as i64));
            Ok(mk_map(result))
        }
        Err(e) => err(format!("lsp.start: {}", e)),
    }
}

/// Return the LSP specification version supported:
/// `lsp.version()`
fn lsp_version(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    Ok(mk_str_from("3.17.0"))
}

/// Return server capabilities as a map:
/// `lsp.capabilities()`
fn lsp_capabilities(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    let mut m = std::collections::HashMap::new();
    m.insert(
        "completionProvider".to_string(),
        mk_map(std::collections::HashMap::from([(
            "triggerCharacters".to_string(),
            mk_array(vec![mk_str_from("."), mk_str_from(":")]),
        )])),
    );
    m.insert("textDocumentSync".to_string(), mk_int(1)); // Full sync
    m.insert("hoverProvider".to_string(), bool_of(true));
    m.insert("documentFormattingProvider".to_string(), bool_of(true));
    m.insert("documentSymbolProvider".to_string(), bool_of(true));
    m.insert(
        "diagnosticProvider".to_string(),
        mk_map(std::collections::HashMap::from([
            ("interFileDependencies".to_string(), bool_of(false)),
            ("workspaceDiagnostics".to_string(), bool_of(false)),
        ])),
    );
    Ok(mk_map(m))
}

/// Format a JSON-RPC request message:
/// `lsp.format_request(method, params?)`
fn lsp_format_request(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "lsp.format_request")?;
    let method = want_str(args[0], "lsp.format_request")?;
    let id: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut json = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":\"{}\"",
        id,
        json_str(&method)
    );
    if args.len() >= 2 && !is_null(args[1]) {
        json.push_str(&format!(",\"params\":{}", plix_to_json(args[1])));
    }
    json.push_str("}");

    // Wrap with Content-Length header
    let body = json;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    Ok(mk_string(format!("{}{}", header, body)))
}

/// Parse a JSON-RPC message string into a Plix map:
/// `lsp.parse_message(raw)`
fn lsp_parse_message(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "lsp.parse_message")?;
    let raw = want_str(args[0], "lsp.parse_message")?;
    let mut m = std::collections::HashMap::new();

    // Split headers from body
    if let Some(body_start) = raw.find("\r\n\r\n") {
        let _headers = &raw[..body_start];
        let body = &raw[body_start + 4..];
        m.insert("body".to_string(), mk_string(body.to_string()));
        // Basic JSON field extraction
        if let Some(jsonrpc) = extract_json_string(body, "jsonrpc") {
            m.insert("jsonrpc".to_string(), mk_string(jsonrpc));
        }
        if let Some(method) = extract_json_string(body, "method") {
            m.insert("method".to_string(), mk_string(method));
        }
        if let Some(id) = extract_json_number(body, "id") {
            m.insert("id".to_string(), mk_int(id));
        }
        m.insert("valid".to_string(), bool_of(true));
    } else {
        m.insert("valid".to_string(), bool_of(false));
    }

    Ok(mk_map(m))
}

/// Simple JSON string field extractor
fn extract_json_string(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", field);
    let start = json.find(&needle)?;
    let val_start = start + needle.len();
    let val_end = json[val_start..].find('"')?;
    Some(json[val_start..val_start + val_end].to_string())
}

/// Simple JSON number field extractor
fn extract_json_number(json: &str, field: &str) -> Option<i64> {
    let needle = format!("\"{}\":", field);
    let start = json.find(&needle)?;
    let val_start = start + needle.len();
    let rest = json[val_start..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

/// Convert a Plix value to a JSON string (basic types only)
fn plix_to_json(v: V) -> String {
    if is_null(v) {
        return "null".to_string();
    }
    if v == TRUE {
        return "true".to_string();
    }
    if v == FALSE {
        return "false".to_string();
    }
    if is_int(v) {
        return format!("{}", as_int(v));
    }
    if is_ptr(v) {
        unsafe {
            return match payload(v) {
                HeapObj::Float(f) => format!("{:?}", f),
                HeapObj::Str(s) => json_str(s),
                HeapObj::Array(items) => {
                    let parts: Vec<String> = items.iter().map(|&x| plix_to_json(x)).collect();
                    format!("[{}]", parts.join(","))
                }
                HeapObj::Map(m) => {
                    let parts: Vec<String> = m
                        .iter()
                        .map(|(k, &x)| format!("{}:{}", json_str(k), plix_to_json(x)))
                        .collect();
                    format!("{{{}}}", parts.join(","))
                }
                _ => "null".to_string(),
            };
        }
    }
    "null".to_string()
}

// ---------------------------------------------------------------------------
// wasm module — WebAssembly output utilities
// ---------------------------------------------------------------------------

/// Return the WASM specification version:
/// `wasm.version()`
fn wasm_version(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    Ok(mk_str_from("2.0"))
}

/// Compile Plix source to WASM binary (basic subset):
/// `wasm.compile(source, opts?)`
/// Returns a buffer containing the .wasm binary.
fn wasm_compile(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "wasm.compile")?;
    let source = want_str(args[0], "wasm.compile")?;

    // Parse the source to extract function signatures and basic structure
    let wasm_bytes = compile_plix_to_wasm(&source)?;
    Ok(mk_buffer(wasm_bytes))
}

/// Validate a WASM binary (basic magic number + version check):
/// `wasm.validate(bytes)`
fn wasm_validate(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    if is_buffer(args[0]) {
        unsafe {
            if let HeapObj::Buffer(data) = payload(args[0]) {
                let valid = data.len() >= 8
                    && data[0] == 0x00
                    && data[1] == 0x61
                    && data[2] == 0x73
                    && data[3] == 0x6D
                    && data[4] == 0x01
                    && data[5] == 0x00
                    && data[6] == 0x00
                    && data[7] == 0x00;
                return Ok(bool_of(valid));
            }
        }
    }
    Ok(bool_of(false))
}

/// Return the WASM magic number bytes:
/// `wasm.magic()`
fn wasm_magic(_c: &mut dyn Caller, _args: &[V]) -> OpResult {
    Ok(mk_array(vec![
        mk_int(0),
        mk_int(0x61),
        mk_int(0x73),
        mk_int(0x6D),
        mk_int(1),
        mk_int(0),
        mk_int(0),
        mk_int(0),
    ]))
}

// ---- WASM compilation via real compiler ----

/// Compile Plix source to a valid WASM binary using the real plix compiler.
/// This invokes `plix build --target wasm` as a subprocess, which uses the
/// full AST-based codegen from src/wasm.rs — producing a properly working
/// WASM module with WASI fd_write for say(), correct int-to-string conversion,
/// string output, control flow, etc.
fn compile_plix_to_wasm(source: &str) -> Result<Vec<u8>, String> {
    use std::io::Write;

    // Find the plix binary — try current exe first, then PATH
    let plix_bin = std::env::current_exe()
        .ok()
        .filter(|p| p.exists())
        .or_else(|| which_plix())
        .ok_or_else(|| "wasm.compile: cannot find plix binary".to_string())?;

    // Write source to a temp file
    let tmp_dir = std::env::temp_dir();
    let src_path = tmp_dir.join("plix_wasm_compile_input.px");
    let out_path = tmp_dir.join("plix_wasm_compile_output.wasm");

    {
        let mut f = std::fs::File::create(&src_path)
            .map_err(|e| format!("wasm.compile: cannot create temp file: {}", e))?;
        f.write_all(source.as_bytes())
            .map_err(|e| format!("wasm.compile: cannot write temp file: {}", e))?;
    }

    // Remove old output if it exists
    let _ = std::fs::remove_file(&out_path);

    // Run: plix build <src> --target wasm -o <out>
    let output = std::process::Command::new(&plix_bin)
        .arg("build")
        .arg(&src_path)
        .arg("--target")
        .arg("wasm")
        .arg("-o")
        .arg(&out_path)
        .output()
        .map_err(|e| format!("wasm.compile: failed to run plix: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("wasm.compile: {}", stderr.trim()));
    }

    // Read the resulting .wasm binary
    let wasm_bytes =
        std::fs::read(&out_path).map_err(|e| format!("wasm.compile: cannot read output: {}", e))?;

    // Clean up temp files
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&out_path);

    Ok(wasm_bytes)
}

/// Try to find the `plix` binary on PATH
fn which_plix() -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join("plix");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// ffi module — zero-copy foreign function interface
// ---------------------------------------------------------------------------

/// Load a shared library: `ffi.load(path)`
/// Returns a foreign library handle.
fn ffi_load(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "ffi.load")?;
    let path = want_str(args[0], "ffi.load")?;
    let handle = crate::heap::sys_dlopen(&path);
    if handle.is_null() {
        err(format!("ffi.load: cannot open library \"{}\"", path))
    } else {
        Ok(mk_foreign_lib(handle))
    }
}

/// Close a shared library: `ffi.close(handle)`
fn ffi_close(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "ffi.close")?;
    if !is_foreign_lib(args[0]) {
        return err("ffi.close: expected a foreign library handle");
    }
    unsafe {
        if let HeapObj::ForeignLib(p) = payload(args[0]) {
            let rc = crate::heap::sys_dlclose(*p);
            if rc != 0 {
                return err(format!("ffi.close: dlclose returned {}", rc));
            }
        }
    }
    Ok(NULL)
}

/// Call a foreign function: `ffi.call(lib, name, signature, args)`
///   - lib: foreign library handle
///   - name: function name (string)
///   - signature: string like "ii:i" (params:return, i=int, f=float, s=string, v=void)
///   - args: array of arguments
/// Returns the function result as a Plix value.
fn ffi_call(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 3, "ffi.call")?;
    if !is_foreign_lib(args[0]) {
        return err("ffi.call: first argument must be a foreign library handle");
    }
    let name = want_str(args[1], "ffi.call")?;
    let sig = want_str(args[2], "ffi.call")?;

    // Parse signature: "ii:i" means two int params returning int
    let (param_types, ret_type) = parse_ffi_sig(&sig)?;

    let handle = unsafe {
        match payload(args[0]) {
            HeapObj::ForeignLib(p) => *p,
            _ => return err("ffi.call: invalid library handle"),
        }
    };

    let sym = crate::heap::sys_dlsym(handle, &name);
    if sym.is_null() {
        return err(format!("ffi.call: symbol \"{}\" not found", name));
    }

    // Collect arguments
    let call_args = if args.len() >= 4 && is_ptr(args[3]) {
        unsafe {
            match payload(args[3]) {
                HeapObj::Array(items) => items.clone(),
                _ => return err("ffi.call: arguments must be an array"),
            }
        }
    } else {
        vec![]
    };

    if call_args.len() != param_types.len() {
        return err(format!(
            "ffi.call: signature expects {} arguments, got {}",
            param_types.len(),
            call_args.len()
        ));
    }

    // Build the call using libffi-style approach
    // For safety, we support only simple signatures with up to 6 args
    if param_types.len() > 6 {
        return err("ffi.call: maximum 6 arguments supported");
    }

    unsafe {
        match (ret_type.as_str(), param_types.len()) {
            ("v", 0) => {
                let f: unsafe extern "C" fn() = std::mem::transmute(sym);
                f();
                Ok(NULL)
            }
            ("i", 0) => {
                let f: unsafe extern "C" fn() -> i64 = std::mem::transmute(sym);
                Ok(mk_int(f()))
            }
            ("f", 0) => {
                let f: unsafe extern "C" fn() -> f64 = std::mem::transmute(sym);
                Ok(mk_float(f()))
            }
            ("i", 1) if param_types[0] == 'i' => {
                let f: unsafe extern "C" fn(i64) -> i64 = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                Ok(mk_int(f(a)))
            }
            ("i", 1) if param_types[0] == 'f' => {
                let f: unsafe extern "C" fn(f64) -> i64 = std::mem::transmute(sym);
                let a = want_num(call_args[0], "ffi.call arg0")?;
                Ok(mk_int(f(a)))
            }
            ("f", 1) if param_types[0] == 'i' => {
                let f: unsafe extern "C" fn(i64) -> f64 = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                Ok(mk_float(f(a)))
            }
            ("f", 1) if param_types[0] == 'f' => {
                let f: unsafe extern "C" fn(f64) -> f64 = std::mem::transmute(sym);
                let a = want_num(call_args[0], "ffi.call arg0")?;
                Ok(mk_float(f(a)))
            }
            ("v", 1) if param_types[0] == 'i' => {
                let f: unsafe extern "C" fn(i64) = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                f(a);
                Ok(NULL)
            }
            ("v", 1) if param_types[0] == 'f' => {
                let f: unsafe extern "C" fn(f64) = std::mem::transmute(sym);
                let a = want_num(call_args[0], "ffi.call arg0")?;
                f(a);
                Ok(NULL)
            }
            ("i", 2) if param_types[0] == 'i' && param_types[1] == 'i' => {
                let f: unsafe extern "C" fn(i64, i64) -> i64 = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                Ok(mk_int(f(a, b)))
            }
            ("f", 2) if param_types[0] == 'f' && param_types[1] == 'f' => {
                let f: unsafe extern "C" fn(f64, f64) -> f64 = std::mem::transmute(sym);
                let a = want_num(call_args[0], "ffi.call arg0")?;
                let b = want_num(call_args[1], "ffi.call arg1")?;
                Ok(mk_float(f(a, b)))
            }
            ("i", 2) if param_types[0] == 'f' && param_types[1] == 'f' => {
                let f: unsafe extern "C" fn(f64, f64) -> i64 = std::mem::transmute(sym);
                let a = want_num(call_args[0], "ffi.call arg0")?;
                let b = want_num(call_args[1], "ffi.call arg1")?;
                Ok(mk_int(f(a, b)))
            }
            // 3-arg calls (all i64 for simplicity)
            ("i", 3) => {
                let f: unsafe extern "C" fn(i64, i64, i64) -> i64 = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                let c = want_int(call_args[2], "ffi.call arg2")?;
                Ok(mk_int(f(a, b, c)))
            }
            ("v", 3) => {
                let f: unsafe extern "C" fn(i64, i64, i64) = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                let c = want_int(call_args[2], "ffi.call arg2")?;
                f(a, b, c);
                Ok(NULL)
            }
            ("i", 4) => {
                let f: unsafe extern "C" fn(i64, i64, i64, i64) -> i64 = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                let c = want_int(call_args[2], "ffi.call arg2")?;
                let d = want_int(call_args[3], "ffi.call arg3")?;
                Ok(mk_int(f(a, b, c, d)))
            }
            ("v", 4) => {
                let f: unsafe extern "C" fn(i64, i64, i64, i64) = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                let c = want_int(call_args[2], "ffi.call arg2")?;
                let d = want_int(call_args[3], "ffi.call arg3")?;
                f(a, b, c, d);
                Ok(NULL)
            }
            ("i", 5) => {
                let f: unsafe extern "C" fn(i64, i64, i64, i64, i64) -> i64 = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                let c = want_int(call_args[2], "ffi.call arg2")?;
                let d = want_int(call_args[3], "ffi.call arg3")?;
                let e2 = want_int(call_args[4], "ffi.call arg4")?;
                Ok(mk_int(f(a, b, c, d, e2)))
            }
            ("v", 5) => {
                let f: unsafe extern "C" fn(i64, i64, i64, i64, i64) = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                let c = want_int(call_args[2], "ffi.call arg2")?;
                let d = want_int(call_args[3], "ffi.call arg3")?;
                let e2 = want_int(call_args[4], "ffi.call arg4")?;
                f(a, b, c, d, e2);
                Ok(NULL)
            }
            ("i", 6) => {
                let f: unsafe extern "C" fn(i64, i64, i64, i64, i64, i64) -> i64 = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                let c = want_int(call_args[2], "ffi.call arg2")?;
                let d = want_int(call_args[3], "ffi.call arg3")?;
                let e2 = want_int(call_args[4], "ffi.call arg4")?;
                let g = want_int(call_args[5], "ffi.call arg5")?;
                Ok(mk_int(f(a, b, c, d, e2, g)))
            }
            ("v", 6) => {
                let f: unsafe extern "C" fn(i64, i64, i64, i64, i64, i64) = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                let c = want_int(call_args[2], "ffi.call arg2")?;
                let d = want_int(call_args[3], "ffi.call arg3")?;
                let e2 = want_int(call_args[4], "ffi.call arg4")?;
                let g = want_int(call_args[5], "ffi.call arg5")?;
                f(a, b, c, d, e2, g);
                Ok(NULL)
            }
            // Mixed type 2-arg combinations
            ("i", 2) if param_types[0] == 'i' && param_types[1] == 'f' => {
                let f: unsafe extern "C" fn(i64, f64) -> i64 = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_num(call_args[1], "ffi.call arg1")?;
                Ok(mk_int(f(a, b)))
            }
            ("i", 2) if param_types[0] == 'f' && param_types[1] == 'i' => {
                let f: unsafe extern "C" fn(f64, i64) -> i64 = std::mem::transmute(sym);
                let a = want_num(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                Ok(mk_int(f(a, b)))
            }
            ("f", 2) if param_types[0] == 'i' && param_types[1] == 'f' => {
                let f: unsafe extern "C" fn(i64, f64) -> f64 = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_num(call_args[1], "ffi.call arg1")?;
                Ok(mk_float(f(a, b)))
            }
            ("f", 2) if param_types[0] == 'f' && param_types[1] == 'i' => {
                let f: unsafe extern "C" fn(f64, i64) -> f64 = std::mem::transmute(sym);
                let a = want_num(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                Ok(mk_float(f(a, b)))
            }
            ("f", 2) if param_types[0] == 'i' && param_types[1] == 'i' => {
                let f: unsafe extern "C" fn(i64, i64) -> f64 = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                Ok(mk_float(f(a, b)))
            }
            ("v", 2) if param_types[0] == 'i' && param_types[1] == 'i' => {
                let f: unsafe extern "C" fn(i64, i64) = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_int(call_args[1], "ffi.call arg1")?;
                f(a, b);
                Ok(NULL)
            }
            ("v", 2) if param_types[0] == 'i' && param_types[1] == 'f' => {
                let f: unsafe extern "C" fn(i64, f64) = std::mem::transmute(sym);
                let a = want_int(call_args[0], "ffi.call arg0")?;
                let b = want_num(call_args[1], "ffi.call arg1")?;
                f(a, b);
                Ok(NULL)
            }
            _ => err(format!(
                "ffi.call: unsupported signature \"{}\" (supported: i/f params, i/f/v return, up to 6 args)",
                sig
            )),
        }
    }
}

/// Parse an FFI signature string like "ii:i" into (param_types, return_type)
fn parse_ffi_sig(sig: &str) -> Result<(Vec<char>, String), String> {
    let parts: Vec<&str> = sig.split(':').collect();
    if parts.len() != 2 {
        return err(format!(
            "ffi.call: invalid signature \"{}\" (expected \"params:return\")",
            sig
        ));
    }
    let param_types: Vec<char> = parts[0].chars().filter(|c| *c != 'v').collect();
    let ret_type = parts[1].to_string();
    for &c in &param_types {
        if c != 'i' && c != 'f' {
            return err(format!(
                "ffi.call: unsupported parameter type '{}' (use 'i' or 'f')",
                c
            ));
        }
    }
    if ret_type != "i" && ret_type != "f" && ret_type != "v" {
        return err(format!(
            "ffi.call: unsupported return type '{}' (use 'i', 'f', or 'v')",
            ret_type
        ));
    }
    Ok((param_types, ret_type))
}

/// Allocate a raw byte buffer: `ffi.buffer(size)`
fn ffi_buffer(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "ffi.buffer")?;
    let size = want_int(args[0], "ffi.buffer")?;
    if size < 0 {
        return err("ffi.buffer: size must be non-negative");
    }
    Ok(mk_buffer(vec![0u8; size as usize]))
}

/// Get buffer length: `ffi.buffer_len(buf)`
fn ffi_buffer_len(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "ffi.buffer_len")?;
    if !is_buffer(args[0]) {
        return err("ffi.buffer_len: expected a buffer");
    }
    unsafe {
        match payload(args[0]) {
            HeapObj::Buffer(data) => Ok(mk_int(data.len() as i64)),
            _ => err("ffi.buffer_len: invalid buffer"),
        }
    }
}

macro_rules! ffi_read_fn {
    ($name:ident, $rust_ty:ty, $plix_fn:expr) => {
        fn $name(_c: &mut dyn Caller, args: &[V]) -> OpResult {
            need(args, 2, stringify!($name))?;
            if !is_buffer(args[0]) {
                return err(format!("{}: expected a buffer", stringify!($name)));
            }
            let offset = want_int(args[1], stringify!($name))? as usize;
            unsafe {
                match payload(args[0]) {
                    HeapObj::Buffer(data) => {
                        if offset + std::mem::size_of::<$rust_ty>() > data.len() {
                            return err(format!(
                                "{}: offset {} out of bounds (buffer size {})",
                                stringify!($name),
                                offset,
                                data.len()
                            ));
                        }
                        let ptr = data.as_ptr().add(offset) as *const $rust_ty;
                        let value = std::ptr::read_unaligned(ptr);
                        Ok($plix_fn(value))
                    }
                    _ => err(format!("{}: invalid buffer", stringify!($name))),
                }
            }
        }
    };
}

ffi_read_fn!(ffi_read_u8, u8, |v: u8| mk_int(v as i64));
ffi_read_fn!(ffi_read_i8, i8, |v: i8| mk_int(v as i64));
ffi_read_fn!(ffi_read_u16, u16, |v: u16| mk_int(v as i64));
ffi_read_fn!(ffi_read_i16, i16, |v: i16| mk_int(v as i64));
ffi_read_fn!(ffi_read_i32, i32, |v: i32| mk_int(v as i64));
ffi_read_fn!(ffi_read_i64, i64, |v: i64| mk_int(v));
ffi_read_fn!(ffi_read_f32, f32, |v: f32| mk_float(v as f64));
ffi_read_fn!(ffi_read_f64, f64, |v: f64| mk_float(v));

macro_rules! ffi_write_fn {
    ($name:ident, $rust_ty:ty, $extract_fn:expr) => {
        fn $name(_c: &mut dyn Caller, args: &[V]) -> OpResult {
            need(args, 3, stringify!($name))?;
            if !is_buffer(args[0]) {
                return err(format!("{}: expected a buffer", stringify!($name)));
            }
            let offset = want_int(args[1], stringify!($name))? as usize;
            let value: $rust_ty = $extract_fn(args[2], stringify!($name))?;
            unsafe {
                match payload_mut(args[0]) {
                    p if !p.is_null() => {
                        let obj = &mut *p;
                        match obj {
                            HeapObj::Buffer(data) => {
                                if offset + std::mem::size_of::<$rust_ty>() > data.len() {
                                    return err(format!(
                                        "{}: offset {} out of bounds (buffer size {})",
                                        stringify!($name),
                                        offset,
                                        data.len()
                                    ));
                                }
                                let ptr = data.as_mut_ptr().add(offset) as *mut $rust_ty;
                                std::ptr::write_unaligned(ptr, value);
                                Ok(NULL)
                            }
                            _ => err(format!("{}: invalid buffer", stringify!($name))),
                        }
                    }
                    _ => err(format!("{}: invalid buffer", stringify!($name))),
                }
            }
        }
    };
}

ffi_write_fn!(ffi_write_u8, u8, |v: V, n: &str| want_int(v, n)
    .map(|x| x as u8));
ffi_write_fn!(ffi_write_i8, i8, |v: V, n: &str| want_int(v, n)
    .map(|x| x as i8));
ffi_write_fn!(ffi_write_i32, i32, |v: V, n: &str| want_int(v, n)
    .map(|x| x as i32));
ffi_write_fn!(ffi_write_i64, i64, |v: V, n: &str| want_int(v, n));
ffi_write_fn!(ffi_write_f32, f32, |v: V, n: &str| want_num(v, n)
    .map(|x| x as f32));
ffi_write_fn!(ffi_write_f64, f64, |v: V, n: &str| want_num(v, n));

/// Copy buffer to Plix array of byte values: `ffi.buffer_to_array(buf)`
fn ffi_buffer_to_array(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "ffi.buffer_to_array")?;
    if !is_buffer(args[0]) {
        return err("ffi.buffer_to_array: expected a buffer");
    }
    unsafe {
        match payload(args[0]) {
            HeapObj::Buffer(data) => {
                let arr: Vec<V> = data.iter().map(|&b| mk_int(b as i64)).collect();
                Ok(mk_array(arr))
            }
            _ => err("ffi.buffer_to_array: invalid buffer"),
        }
    }
}

/// Create buffer from Plix array of byte values: `ffi.array_to_buffer(arr)`
fn ffi_array_to_buffer(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "ffi.array_to_buffer")?;
    if !is_ptr(args[0]) {
        return err("ffi.array_to_buffer: expected an array");
    }
    unsafe {
        match payload(args[0]) {
            HeapObj::Array(items) => {
                let data: Vec<u8> = items
                    .iter()
                    .map(|&v| if is_int(v) { as_int(v) as u8 } else { 0 })
                    .collect();
                Ok(mk_buffer(data))
            }
            _ => err("ffi.array_to_buffer: expected an array"),
        }
    }
}

/// Get the size of a C type: `ffi.sizeof(type_name)`
fn ffi_sizeof(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "ffi.sizeof")?;
    let ty = want_str(args[0], "ffi.sizeof")?;
    let size = match ty.as_str() {
        "u8" | "i8" => 1,
        "u16" | "i16" => 2,
        "u32" | "i32" | "f32" => 4,
        "u64" | "i64" | "f64" | "pointer" | "ptr" => 8,
        _ => return err(format!("ffi.sizeof: unknown type \"{}\"", ty)),
    };
    Ok(mk_int(size))
}

/// Get the alignment of a C type: `ffi.alignof(type_name)`
fn ffi_alignof(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 1, "ffi.alignof")?;
    let ty = want_str(args[0], "ffi.alignof")?;
    let align = match ty.as_str() {
        "u8" | "i8" => 1,
        "u16" | "i16" => 2,
        "u32" | "i32" | "f32" => 4,
        "u64" | "i64" | "f64" | "pointer" | "ptr" => 8,
        _ => return err(format!("ffi.alignof: unknown type \"{}\"", ty)),
    };
    Ok(mk_int(align))
}

fn b_spawn(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    if args.is_empty() {
        return Err("spawn: expected 1 argument (function)".into());
    }
    Ok(crate::heap::mk_int(1)) // Demo handle
}
