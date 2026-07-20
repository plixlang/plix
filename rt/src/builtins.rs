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
];

const MODULES: &[&str] = &["fs", "sys", "net", "py", "ai", "forge"];

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
            return;
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
                return err("int: cannot convert non-finite float");
            }
            HeapObj::Str(s) => {
                if let Ok(i) = s.trim().parse::<i64>() {
                    return Ok(mk_int(i));
                }
                if let Ok(f) = s.trim().parse::<f64>() {
                    return Ok(mk_int(f.trunc() as i64));
                }
                return err(format!("int: cannot parse \"{}\"", s));
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
        if e >= 0 && e < 63 {
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
    let content = to_display(args[1]);
    std::fs::write(&p, content)
        .map(|_| bool_of(true))
        .map_err(|e| ioerr("write", e))
}
fn fs_append(_c: &mut dyn Caller, args: &[V]) -> OpResult {
    need(args, 2, "fs.append")?;
    let p = want_str(args[0], "fs.append")?;
    let content = to_display(args[1]);
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&p)
        .and_then(|mut f| f.write_all(content.as_bytes()))
        .map(|_| bool_of(true))
        .map_err(|e| ioerr("append", e))
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
    Ok(mk_str_from("plix 0.2.0 (rust runtime)"))
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
